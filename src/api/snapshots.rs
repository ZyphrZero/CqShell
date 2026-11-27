//! Read-only structured telemetry snapshots (host system, Docker, Mihomo/Clash) and
//! controlled Docker mutation execution for JSON CLI and MCP consumers.
//!
//! Security invariant: no raw secret, controller token, SOCKS5 password, subscription URL token,
//! or unredacted Docker environment variable is ever serialized here.

use super::{ExecutionPolicy, SCHEMA_VERSION, drain_logs, error_envelope, error_object, exit_code};
use crate::{
    dashboard::{self, ConnectionCounts, LocalSnapshot, PublicSnapshot, Reading, Usage},
    error::{Error, Result},
    ops::{
        clash::{
            self, State as ClashState,
            model::{
                ClashAvailability, ClashSnapshot, ConnectionItem, ProfileSummary, ProxyGroupItem,
                RuleItem,
            },
            monitor as clash_monitor, socks,
        },
        docker::{
            actions::{ConfirmPolicy, DockerOp},
            client as docker_client,
            model::{
                ComposeProject, ContainerHealth, ContainerInspect, ContainerItem, ContainerState,
                ContainerStats, DiskUsageRow, DockerAvailability, DockerSnapshot, DockerTab,
                EngineInfo, EnvVar, ImageInspect, ImageItem, NetworkItem, VolumeItem,
            },
        },
        self_update,
    },
    runtime::Context,
};
use serde_json::{Value, json};
use std::{thread, time::Instant};

const REDACTED: &str = "********";
const DEFAULT_CONNECTIONS_LIMIT: usize = 100;
const MAX_CONNECTIONS_LIMIT: usize = 500;
const DEFAULT_RULES_LIMIT: usize = 200;
const MAX_RULES_LIMIT: usize = 2000;

fn reading_json<T>(reading: &Reading<T>, map: impl FnOnce(&T) -> Value) -> Value {
    match reading {
        Reading::Loading => json!({ "status": "loading", "value": Value::Null }),
        Reading::NotQueried => json!({ "status": "not_queried", "value": Value::Null }),
        Reading::Value(v) => json!({ "status": "value", "value": map(v) }),
        Reading::Unavailable(diag) => json!({
            "status": "unavailable",
            "value": Value::Null,
            "error": diag,
        }),
        Reading::NotApplicable => json!({ "status": "not_applicable", "value": Value::Null }),
    }
}

fn usage_json(usage: &Usage) -> Value {
    json!({
        "used_bytes": usage.used,
        "total_bytes": usage.total,
        "percent": (usage.percent() * 10.0).round() / 10.0,
    })
}

fn connection_counts_json(counts: &ConnectionCounts) -> Value {
    json!({
        "tcp": counts.tcp,
        "tcp_time_wait": counts.tcp_time_wait,
        "udp": counts.udp,
        "total": counts.total(),
    })
}

fn clash_state_json(state: &ClashState) -> Value {
    match state {
        ClashState::NotInstalled => json!({ "status": "not_installed", "pid": Value::Null }),
        ClashState::Running(pid) => json!({ "status": "running", "pid": pid }),
        ClashState::Stopped => json!({ "status": "stopped", "pid": Value::Null }),
        ClashState::Unknown => json!({ "status": "unknown", "pid": Value::Null }),
    }
}

fn sample_local(ctx: &Context, sample_cpu: bool) -> LocalSnapshot {
    let probe = ctx.silent();
    let mut sampler = dashboard::Sampler::new();
    let first = sampler.sample(&probe);
    if !sample_cpu {
        return first;
    }
    let wait = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(std::time::Duration::from_millis(220));
    thread::sleep(wait);
    sampler.sample(&probe)
}

fn local_snapshot_to_json(local: &LocalSnapshot) -> Value {
    json!({
        "host": reading_json(&local.host, |v| json!(v)),
        "os": reading_json(&local.os, |v| json!(v)),
        "kernel": reading_json(&local.kernel, |v| json!(v)),
        "arch": local.arch,
        "cpu": reading_json(&local.cpu, |v| json!(v)),
        "cpu_count": reading_json(&local.cpu_count, |v| json!(v)),
        "cpu_usage_percent": reading_json(&local.cpu_usage, |v| json!((*v as f64 * 10.0).round() / 10.0)),
        "memory": reading_json(&local.memory, usage_json),
        "effective_memory_kb": reading_json(&local.effective_memory_kb, |v| json!(v)),
        "swap": reading_json(&local.swap, usage_json),
        "disk": {
            "mount": local.disk_name,
            "usage": reading_json(&local.disk, usage_json),
        },
        "uptime_secs": reading_json(&local.uptime, |v| json!(v)),
        "timezone": reading_json(&local.timezone, |v| json!(v)),
        "terminal_proxy": reading_json(&local.terminal_proxy, |v| json!(v)),
        "network": {
            "congestion": reading_json(&local.congestion, |v| json!(v)),
            "qdisc": reading_json(&local.qdisc, |v| json!(v)),
            "bbr_available": reading_json(&local.bbr_available, |v| json!(v)),
            "bbr_version": reading_json(&local.bbr_version, |v| json!(v)),
            "tcp_configured": reading_json(&local.tcp_configured, |v| json!(v)),
            "tcp_rmem_max_bytes": reading_json(&local.tcp_rmem_max, |v| json!(v)),
            "connections": reading_json(&local.connections, connection_counts_json),
        },
        "mihomo": {
            "state": reading_json(&local.mihomo_state, clash_state_json),
            "version": reading_json(&local.mihomo_version, |v| json!(v)),
        },
    })
}

fn public_snapshot_to_json(public: &PublicSnapshot) -> Value {
    json!({
        "ipv4": reading_json(&public.ipv4, |v| json!(v)),
        "ipv6": reading_json(&public.ipv6, |v| json!(v)),
        "isp": reading_json(&public.isp, |v| json!(v)),
    })
}

/// Captures the structured host status snapshot (optionally querying public IP/ISP).
pub fn system_status_json(ctx: &Context, include_public_ip: bool, sample_cpu: bool) -> Value {
    let local = sample_local(ctx, sample_cpu);
    let public = if include_public_ip {
        let probe = ctx.silent();
        Some(dashboard::query_public(&probe))
    } else {
        None
    };
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "cqs_version": env!("CARGO_PKG_VERSION"),
        "locale": ctx.i18n.locale(),
        "privileges": {
            "is_root": ctx.is_root(),
            "platform_supported": ctx.platform().is_ok(),
            "platform": std::env::consts::OS,
        },
        "system": local_snapshot_to_json(&local),
        "public_network": public.as_ref().map(public_snapshot_to_json),
    })
}

/// Formats host status as concise human-readable text for `cqs status`.
pub fn system_status_text(ctx: &Context, include_public_ip: bool, sample_cpu: bool) -> String {
    let local = sample_local(ctx, sample_cpu);
    let fmt_str = |r: &Reading<String>| -> String {
        match r {
            Reading::Value(v) => v.clone(),
            Reading::Loading => ctx.i18n.text("dash.loading").to_owned(),
            Reading::NotQueried => ctx.i18n.text("dash.not_queried").to_owned(),
            Reading::Unavailable(e) => format!("{} ({e})", ctx.i18n.text("dash.unavailable")),
            Reading::NotApplicable => ctx.i18n.text("dash.na").to_owned(),
        }
    };
    let fmt_usage = |r: &Reading<Usage>| -> String {
        match r {
            Reading::Value(u) => format!(
                "{:.1}% ({} / {})",
                u.percent(),
                clash::model::format_bytes(u.used),
                clash::model::format_bytes(u.total),
            ),
            Reading::Unavailable(e) => format!("{} ({e})", ctx.i18n.text("dash.unavailable")),
            _ => ctx.i18n.text("dash.na").to_owned(),
        }
    };
    let mut lines = vec![
        format!("CqShell {}", env!("CARGO_PKG_VERSION")),
        format!("{}: {}", ctx.i18n.text("dash.host"), fmt_str(&local.host)),
        format!("{}: {}", ctx.i18n.text("dash.os"), fmt_str(&local.os)),
        format!(
            "{}: {} ({})",
            ctx.i18n.text("dash.kernel_status"),
            fmt_str(&local.kernel),
            local.arch
        ),
        format!("{}: {}", ctx.i18n.text("dash.cpu"), fmt_str(&local.cpu)),
        format!(
            "{}: {}",
            ctx.i18n.text("dash.memory"),
            fmt_usage(&local.memory)
        ),
        format!(
            "{}: {} ({})",
            ctx.i18n.text("dash.disk"),
            fmt_usage(&local.disk),
            local.disk_name
        ),
        format!(
            "{}: {}",
            ctx.i18n.text("dash.timezone"),
            fmt_str(&local.timezone)
        ),
        format!(
            "{}: {} / {}",
            ctx.i18n.text("dash.network"),
            fmt_str(&local.congestion),
            fmt_str(&local.qdisc)
        ),
        format!(
            "{}: {}",
            ctx.i18n.text("dash.connections"),
            match &local.connections {
                Reading::Value(c) => format!(
                    "TCP {} / UDP {} (TIME_WAIT {})",
                    c.tcp, c.udp, c.tcp_time_wait
                ),
                Reading::Unavailable(e) => {
                    format!("{} ({e})", ctx.i18n.text("dash.unavailable"))
                }
                _ => ctx.i18n.text("dash.na").to_owned(),
            }
        ),
    ];
    if include_public_ip {
        let probe = ctx.silent();
        let pub_snap = dashboard::query_public(&probe);
        lines.push(format!(
            "{}: {}",
            ctx.i18n.text("dash.ipv4"),
            fmt_str(&pub_snap.ipv4)
        ));
        lines.push(format!(
            "{}: {}",
            ctx.i18n.text("dash.ipv6"),
            fmt_str(&pub_snap.ipv6)
        ));
        lines.push(format!(
            "{}: {}",
            ctx.i18n.text("dash.isp"),
            fmt_str(&pub_snap.isp)
        ));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Docker structured snapshots, redacted inspection, logs & mutations
// ---------------------------------------------------------------------------

fn docker_availability_json(avail: &DockerAvailability) -> Value {
    match avail {
        DockerAvailability::Loading => json!({ "status": "loading", "detail": Value::Null }),
        DockerAvailability::NotApplicable(d) => json!({ "status": "not_applicable", "detail": d }),
        DockerAvailability::NotInstalled => {
            json!({ "status": "not_installed", "detail": Value::Null })
        }
        DockerAvailability::DaemonStopped(d) => json!({ "status": "daemon_stopped", "detail": d }),
        DockerAvailability::PermissionDenied(d) => {
            json!({ "status": "permission_denied", "detail": d })
        }
        DockerAvailability::Unavailable(d) => json!({ "status": "unavailable", "detail": d }),
        DockerAvailability::Ready => json!({ "status": "ready", "detail": Value::Null }),
    }
}

fn engine_info_json(engine: &EngineInfo) -> Value {
    json!({
        "server_version": engine.server_version,
        "api_version": engine.api_version,
        "compose_version": engine.compose_version,
        "context": engine.context,
        "os_type": engine.os_type,
        "arch": engine.arch,
        "kernel": engine.kernel,
        "storage_driver": engine.storage_driver,
        "cgroup_driver": engine.cgroup_driver,
        "cgroup_version": engine.cgroup_version,
        "root_dir": engine.root_dir,
        "containers_running": engine.containers_running,
        "containers_paused": engine.containers_paused,
        "containers_stopped": engine.containers_stopped,
        "images_total": engine.images_total,
        "service_active": engine.service_active,
        "daemon_mirrors": engine.daemon_mirrors,
        "daemon_ipv6": engine.daemon_ipv6,
        "daemon_ipv6_cidr": engine.daemon_ipv6_cidr,
        "daemon_json_error": engine.daemon_json_error,
    })
}

fn container_state_str(state: ContainerState) -> &'static str {
    match state {
        ContainerState::Running => "running",
        ContainerState::Restarting => "restarting",
        ContainerState::Paused => "paused",
        ContainerState::Created => "created",
        ContainerState::Removing => "removing",
        ContainerState::Exited => "exited",
        ContainerState::Dead => "dead",
        ContainerState::Unknown => "unknown",
    }
}

fn container_health_str(health: ContainerHealth) -> &'static str {
    match health {
        ContainerHealth::None => "none",
        ContainerHealth::Healthy => "healthy",
        ContainerHealth::Unhealthy => "unhealthy",
        ContainerHealth::Starting => "starting",
    }
}

fn container_item_json(item: &ContainerItem) -> Value {
    json!({
        "id": item.id,
        "short_id": item.short_id,
        "name": item.name,
        "image": item.image,
        "command": item.command,
        "created_at": item.created_at,
        "running_for": item.running_for,
        "status": item.status,
        "state": container_state_str(item.state),
        "health": container_health_str(item.health),
        "ports": item.ports,
        "networks": item.networks,
        "mounts_count": item.mounts_count,
        "size": item.size,
        "size_bytes": item.size_bytes,
        "compose_project": item.compose_project,
        "compose_service": item.compose_service,
        "compose_working_dir": item.compose_working_dir,
        "compose_config_files": item.compose_config_files,
        "labels": item.labels,
    })
}

fn compose_project_json(item: &ComposeProject) -> Value {
    json!({
        "name": item.name,
        "status": item.status,
        "config_files": item.config_files,
        "working_dir": item.working_dir,
        "running_count": item.running_count,
        "total_count": item.total_count,
        "container_ids": item.container_ids,
        "synthesized_from_labels": item.synthesized_from_labels,
    })
}

fn image_item_json(item: &ImageItem) -> Value {
    json!({
        "id": item.id,
        "short_id": item.short_id,
        "repository": item.repository,
        "tag": item.tag,
        "display_ref": item.display_ref(),
        "created_since": item.created_since,
        "created_at": item.created_at,
        "size_text": item.size_text,
        "size_bytes": item.size_bytes,
        "dangling": item.dangling,
        "used_by": item.used_by,
    })
}

fn volume_item_json(item: &VolumeItem) -> Value {
    json!({
        "name": item.name,
        "driver": item.driver,
        "scope": item.scope,
        "mountpoint": item.mountpoint,
        "size_text": item.size_text,
        "size_bytes": item.size_bytes,
        "anonymous": item.anonymous,
        "used_by": item.used_by,
        "labels": item.labels,
    })
}

fn network_item_json(item: &NetworkItem) -> Value {
    json!({
        "id": item.id,
        "short_id": item.short_id,
        "name": item.name,
        "driver": item.driver,
        "scope": item.scope,
        "ipv6": item.ipv6,
        "internal": item.internal,
        "builtin": item.builtin,
        "subnet": item.subnet,
        "gateway": item.gateway,
        "ipv6_subnet": item.ipv6_subnet,
        "containers": item.containers.iter().map(|(name, ip)| json!({"name": name, "ip": ip})).collect::<Vec<_>>(),
        "labels": item.labels,
    })
}

fn disk_usage_json(row: &DiskUsageRow) -> Value {
    json!({
        "kind": row.kind,
        "total_count": row.total_count,
        "active_count": row.active_count,
        "size_text": row.size_text,
        "size_bytes": row.size_bytes,
        "reclaimable_text": row.reclaimable_text,
        "reclaimable_bytes": row.reclaimable_bytes,
    })
}

fn stats_json(stats: &ContainerStats) -> Value {
    json!({
        "container_id": stats.container_id,
        "cpu_percent": stats.cpu_percent,
        "mem_used_bytes": stats.mem_used_bytes,
        "mem_limit_bytes": stats.mem_limit_bytes,
        "mem_percent": stats.mem_percent,
        "mem_usage_text": stats.mem_usage_text,
        "net_io_text": stats.net_io_text,
        "block_io_text": stats.block_io_text,
        "pids": stats.pids,
    })
}

/// Redacts sensitive environment variables unconditionally before emitting JSON.
pub(crate) fn redacted_env_json(env: &[EnvVar]) -> Vec<Value> {
    env.iter()
        .map(|entry| {
            json!({
                "key": entry.key,
                "value": if entry.sensitive { REDACTED } else { entry.value.as_str() },
                "sensitive": entry.sensitive,
                "redacted": entry.sensitive,
            })
        })
        .collect()
}

pub(crate) fn serialize_container_inspect(inspect: &ContainerInspect) -> Value {
    // Deliberately omits `raw_json` so sensitive env vars can never leak unredacted.
    json!({
        "id": inspect.id,
        "name": inspect.name,
        "image": inspect.image,
        "platform": inspect.platform,
        "restart_policy": inspect.restart_policy,
        "restart_count": inspect.restart_count,
        "created": inspect.created,
        "started_at": inspect.started_at,
        "finished_at": inspect.finished_at,
        "exit_code": inspect.exit_code,
        "entrypoint": inspect.entrypoint,
        "cmd": inspect.cmd,
        "working_dir": inspect.working_dir,
        "user": inspect.user,
        "ports": inspect.ports,
        "networks": inspect.networks.iter().map(|n| json!({
            "name": n.name,
            "ipv4": n.ipv4,
            "ipv6": n.ipv6,
            "mac": n.mac,
            "gateway": n.gateway,
        })).collect::<Vec<_>>(),
        "mounts": inspect.mounts.iter().map(|m| json!({
            "kind": m.kind,
            "source": m.source,
            "destination": m.destination,
            "mode": m.mode,
            "rw": m.rw,
        })).collect::<Vec<_>>(),
        "env": redacted_env_json(&inspect.env),
        "labels": inspect.labels,
        "health_status": inspect.health_status,
        "health_logs": inspect.health_logs.iter().map(|h| json!({
            "exit_code": h.exit_code,
            "output": h.output,
        })).collect::<Vec<_>>(),
        "top": inspect.top.iter().map(|r| json!({
            "uid": r.uid,
            "pid": r.pid,
            "cpu": r.cpu,
            "mem": r.mem,
            "time": r.time,
            "command": r.command,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn serialize_image_inspect(inspect: &ImageInspect) -> Value {
    // Deliberately omits `raw_json` so sensitive env vars can never leak unredacted.
    json!({
        "id": inspect.id,
        "repo_tags": inspect.repo_tags,
        "os": inspect.os,
        "arch": inspect.arch,
        "size_bytes": inspect.size_bytes,
        "exposed_ports": inspect.exposed_ports,
        "entrypoint": inspect.entrypoint,
        "cmd": inspect.cmd,
        "working_dir": inspect.working_dir,
        "env": redacted_env_json(&inspect.env),
        "labels": inspect.labels,
        "history": inspect.history.iter().map(|h| json!({
            "id": h.id,
            "created_since": h.created_since,
            "created_by": h.created_by,
            "size_text": h.size_text,
            "size_bytes": h.size_bytes,
        })).collect::<Vec<_>>(),
    })
}

/// Queries a structured read-only Docker snapshot. `section` may be `"all"`, `"containers"`,
/// `"compose"`, `"images"`, `"volumes"`, `"networks"`, or `"daemon"`.
pub fn docker_snapshot_json(ctx: &Context, section: &str, include_stats: bool) -> Result<Value> {
    let probe = ctx.silent();
    let (tab, force, mut seed) = match section {
        "all" => (DockerTab::Daemon, true, DockerSnapshot::default()),
        "containers" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Compose);
            s.mark_fetched(DockerTab::Images);
            s.mark_fetched(DockerTab::Volumes);
            s.mark_fetched(DockerTab::Networks);
            s.df_updated = Some(Instant::now());
            (DockerTab::Containers, false, s)
        }
        "compose" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Images);
            s.mark_fetched(DockerTab::Volumes);
            s.mark_fetched(DockerTab::Networks);
            s.df_updated = Some(Instant::now());
            (DockerTab::Compose, false, s)
        }
        "images" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Compose);
            s.mark_fetched(DockerTab::Volumes);
            s.mark_fetched(DockerTab::Networks);
            s.df_updated = Some(Instant::now());
            (DockerTab::Images, false, s)
        }
        "volumes" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Compose);
            s.mark_fetched(DockerTab::Images);
            s.mark_fetched(DockerTab::Networks);
            s.df_updated = Some(Instant::now());
            (DockerTab::Volumes, false, s)
        }
        "networks" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Containers);
            s.mark_fetched(DockerTab::Compose);
            s.mark_fetched(DockerTab::Images);
            s.mark_fetched(DockerTab::Volumes);
            s.df_updated = Some(Instant::now());
            (DockerTab::Networks, false, s)
        }
        "daemon" => {
            let mut s = DockerSnapshot::default();
            s.mark_fetched(DockerTab::Containers);
            s.mark_fetched(DockerTab::Compose);
            s.mark_fetched(DockerTab::Images);
            s.mark_fetched(DockerTab::Volumes);
            s.mark_fetched(DockerTab::Networks);
            (DockerTab::Daemon, false, s)
        }
        other => {
            return Err(Error::new(
                "error.input",
                format!(
                    "invalid docker section '{other}'; expected all|containers|compose|images|volumes|networks|daemon"
                ),
            ));
        }
    };
    seed.engine_checked = None;
    let snap = docker_client::fetch_snapshot_with_options(&probe, tab, &seed, force);

    let stats_value = if include_stats && snap.availability.is_ready() {
        match docker_client::fetch_stats(&probe) {
            Ok(map) => Value::Array(map.values().map(stats_json).collect()),
            Err(_) => Value::Array(Vec::new()),
        }
    } else {
        Value::Null
    };

    let mut body = serde_json::Map::new();
    body.insert("schema_version".into(), json!(SCHEMA_VERSION));
    body.insert("ok".into(), json!(true));
    body.insert("section".into(), json!(section));
    body.insert(
        "availability".into(),
        docker_availability_json(&snap.availability),
    );
    body.insert(
        "engine".into(),
        snap.engine
            .as_ref()
            .map(engine_info_json)
            .unwrap_or(Value::Null),
    );
    if matches!(section, "all" | "containers") {
        body.insert(
            "containers".into(),
            Value::Array(snap.containers.iter().map(container_item_json).collect()),
        );
    }
    if matches!(section, "all" | "compose") {
        body.insert(
            "compose".into(),
            Value::Array(snap.compose.iter().map(compose_project_json).collect()),
        );
    }
    if matches!(section, "all" | "images") {
        body.insert(
            "images".into(),
            Value::Array(snap.images.iter().map(image_item_json).collect()),
        );
    }
    if matches!(section, "all" | "volumes") {
        body.insert(
            "volumes".into(),
            Value::Array(snap.volumes.iter().map(volume_item_json).collect()),
        );
    }
    if matches!(section, "all" | "networks") {
        body.insert(
            "networks".into(),
            Value::Array(snap.networks.iter().map(network_item_json).collect()),
        );
    }
    if matches!(section, "all" | "daemon") {
        body.insert(
            "disk_usage".into(),
            Value::Array(snap.disk_usage.iter().map(disk_usage_json).collect()),
        );
    }
    if include_stats {
        body.insert("stats".into(), stats_value);
    }
    if let Some(err) = snap.last_error {
        body.insert("last_error".into(), json!(err));
    }
    Ok(Value::Object(body))
}

/// Inspects a single Docker container or image with unconditional secret redaction.
pub fn docker_inspect_json(ctx: &Context, kind: &str, id: &str, running: bool) -> Result<Value> {
    if id.is_empty() || id.starts_with('-') || id.chars().any(char::is_control) {
        return Err(Error::new("error.input", format!("invalid id: {id}")));
    }
    if !matches!(kind, "container" | "image") {
        return Err(Error::new(
            "error.input",
            format!("invalid inspect kind '{kind}'; expected container|image"),
        ));
    }
    let probe = ctx.silent();
    probe.platform()?;
    match kind {
        "container" => {
            let inspect = docker_client::fetch_container_inspect(&probe, id, running)?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "kind": "container",
                "inspect": serialize_container_inspect(&inspect),
            }))
        }
        "image" => {
            let inspect = docker_client::fetch_image_inspect(&probe, id)?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "kind": "image",
                "inspect": serialize_image_inspect(&inspect),
            }))
        }
        _ => unreachable!(),
    }
}

/// Returns bounded, ANSI-sanitized container logs.
pub fn docker_logs_json(ctx: &Context, id: &str, tail: usize) -> Result<Value> {
    let probe = ctx.silent();
    let lines = docker_client::fetch_container_logs(&probe, id, tail)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "container_id": id,
        "tail": tail.clamp(1, 1000),
        "count": lines.len(),
        "lines": lines,
    }))
}

fn str_list(args: &Value, key: &str) -> Result<Vec<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let s = item.as_str().ok_or_else(|| {
                    Error::new("error.input", format!("{key} must contain strings"))
                })?;
                out.push(s.to_owned());
            }
            Ok(out)
        }
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        _ => Err(Error::new(
            "error.input",
            format!("{key} must be an array of strings"),
        )),
    }
}

fn req_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::new("error.required", key))
}

fn opt_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Builds a validated [`DockerOp`] from a structured JSON argument object.
pub fn parse_docker_op(args: &Value) -> Result<DockerOp> {
    let operation = req_str(args, "operation")?;
    let op = match operation.as_str() {
        "container_start" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerStart {
                names: ids.clone(),
                ids,
            }
        }
        "container_stop" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerStop {
                names: ids.clone(),
                ids,
            }
        }
        "container_restart" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerRestart {
                names: ids.clone(),
                ids,
            }
        }
        "container_pause" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerPause {
                names: ids.clone(),
                ids,
            }
        }
        "container_unpause" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerUnpause {
                names: ids.clone(),
                ids,
            }
        }
        "container_remove" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ContainerRemove {
                names: ids.clone(),
                ids,
                force: opt_bool(args, "force"),
            }
        }
        "compose_up" => DockerOp::ComposeUp {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
        },
        "compose_start" => DockerOp::ComposeStart {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
        },
        "compose_stop" => DockerOp::ComposeStop {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
        },
        "compose_restart" => DockerOp::ComposeRestart {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
        },
        "compose_pull" => DockerOp::ComposePull {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
        },
        "compose_down" => DockerOp::ComposeDown {
            project: req_str(args, "project")?,
            config_files: str_list(args, "config_files")?,
            volumes: opt_bool(args, "volumes"),
        },
        "image_pull" => DockerOp::ImagePull {
            reference: req_str(args, "reference")?,
        },
        "image_remove" => {
            let ids = str_list(args, "ids")?;
            DockerOp::ImageRemove {
                labels: ids.clone(),
                ids,
                force: opt_bool(args, "force"),
            }
        }
        "volume_remove" => DockerOp::VolumeRemove {
            names: str_list(args, "names")?,
        },
        "network_create" => DockerOp::NetworkCreate {
            name: req_str(args, "name")?,
            subnet: args
                .get("subnet")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            ipv6: opt_bool(args, "ipv6"),
        },
        "network_remove" => {
            let ids = str_list(args, "ids")?;
            DockerOp::NetworkRemove {
                names: ids.clone(),
                ids,
            }
        }
        "prune" => DockerOp::Prune {
            containers: opt_bool(args, "containers"),
            images: opt_bool(args, "images"),
            all_unused_images: opt_bool(args, "all_unused_images"),
            volumes: opt_bool(args, "volumes"),
            networks: opt_bool(args, "networks"),
            build_cache: opt_bool(args, "build_cache"),
        },
        "service_start" => DockerOp::ServiceStart,
        "service_stop" => DockerOp::ServiceStop,
        "service_restart" => DockerOp::ServiceRestart,
        other => {
            return Err(Error::new(
                "error.input",
                format!("unknown docker operation: {other}"),
            ));
        }
    };
    op.validate()?;
    Ok(op)
}

fn docker_risk_str(policy: ConfirmPolicy) -> &'static str {
    match policy {
        ConfirmPolicy::None => "low",
        ConfirmPolicy::YesNo => "standard",
        ConfirmPolicy::Danger => "danger",
    }
}

/// Previews or executes a typed [`DockerOp`] under `policy` and returns `(json, exit_code)`.
pub fn execute_docker_op_json(
    ctx: &Context,
    policy: &ExecutionPolicy,
    args: &Value,
    confirm: bool,
    dry_run: bool,
) -> (Value, i32) {
    let op = match parse_docker_op(args) {
        Ok(op) => op,
        Err(err) => {
            let code = exit_code(&err);
            return (error_envelope(ctx, &err), code);
        }
    };

    let risk = docker_risk_str(op.confirm_policy());
    let summary = json!({
        "title": ctx.i18n.text(op.title_key()),
        "target": op.target_summary(),
        "cli_preview": op.cli_preview(),
        "risk": risk,
        "requires_root": op.requires_root(),
    });

    if let Err(err) = policy.check_docker_op(&op) {
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "policy_denied",
                "operation": summary,
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    if dry_run {
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "dry_run",
                "operation": summary,
                "requires_confirmation": true,
            }),
            0,
        );
    }

    if !confirm {
        let err = Error::new("error.confirm", "confirm=true");
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "confirmation_required",
                "operation": summary,
                "requires_confirmation": true,
                "confirmation": {
                    "risk": risk,
                    "hint": "Re-run with confirm=true after reviewing cli_preview and target.",
                },
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    let (cap_ctx, rx) = ctx.captured(true);
    let started = Instant::now();
    let result = op.execute(&cap_ctx);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    drop(cap_ctx);
    let captured = drain_logs(rx);

    match result {
        Ok(()) => (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "completed",
                "operation": summary,
                "elapsed_ms": elapsed_ms,
                "logs": captured.lines,
                "logs_total": captured.total,
                "logs_truncated": captured.truncated,
            }),
            0,
        ),
        Err(err) => {
            let code = exit_code(&err);
            let status = if err.key == "error.cancelled" {
                "cancelled"
            } else {
                "failed"
            };
            (
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "status": status,
                    "operation": summary,
                    "elapsed_ms": elapsed_ms,
                    "logs": captured.lines,
                    "logs_total": captured.total,
                    "logs_truncated": captured.truncated,
                    "error": error_object(ctx, &err),
                }),
                code,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Mihomo / Clash redacted read-only snapshots & logs
// ---------------------------------------------------------------------------

fn clash_availability_json(avail: &ClashAvailability) -> Value {
    match avail {
        ClashAvailability::Loading => json!({ "status": "loading", "detail": Value::Null }),
        ClashAvailability::NotInstalled => {
            json!({ "status": "not_installed", "detail": Value::Null })
        }
        ClashAvailability::Stopped => json!({ "status": "stopped", "detail": Value::Null }),
        ClashAvailability::ControllerUnconfigured => {
            json!({ "status": "controller_unconfigured", "detail": Value::Null })
        }
        ClashAvailability::ControllerUnreachable(d) => {
            json!({ "status": "controller_unreachable", "detail": d })
        }
        ClashAvailability::UnknownProcess => {
            json!({ "status": "unknown_process", "detail": Value::Null })
        }
        ClashAvailability::Ready => json!({ "status": "ready", "detail": Value::Null }),
    }
}

fn profile_summary_json(p: &ProfileSummary) -> Value {
    json!({
        "name": p.name,
        "host": p.host,
        "is_local": p.is_local,
        "active": p.active,
        "updated": p.updated,
        "updated_epoch": p.updated_epoch,
        "userinfo": p.userinfo,
        "file_bytes": p.file_bytes,
    })
}

fn proxy_group_json(g: &ProxyGroupItem) -> Value {
    json!({
        "name": g.name,
        "kind": g.kind.label(),
        "now": g.now,
        "resolved_chain": g.resolved_chain,
        "fixed": g.fixed,
        "hidden": g.hidden,
        "member_count": g.members.len(),
        "members": g.members.iter().map(|m| json!({
            "name": m.name,
            "kind": m.kind,
            "delay_ms": m.delay_ms,
            "tested": m.tested,
            "is_group": m.is_group,
            "now": m.now,
            "udp": m.udp,
            "alive": m.alive,
        })).collect::<Vec<_>>(),
    })
}

fn connection_json(c: &ConnectionItem) -> Value {
    json!({
        "id": c.id,
        "network": c.network,
        "type": c.conn_type,
        "source": c.source,
        "destination": c.destination,
        "host": c.host,
        "process": c.process,
        "rule": c.rule,
        "rule_payload": c.rule_payload,
        "chains": c.chains,
        "exit_node": c.exit_node,
        "upload_bytes": c.upload,
        "download_bytes": c.download,
        "start": c.start,
    })
}

fn rule_json(r: &RuleItem) -> Value {
    json!({
        "index": r.index,
        "kind": r.kind,
        "payload": r.payload,
        "proxy": r.proxy,
    })
}

pub(crate) fn serialize_mihomo_snapshot(
    snap: &ClashSnapshot,
    socks_cfg: Option<&socks::SocksConfig>,
    connections_limit: usize,
    rules_limit: usize,
) -> Value {
    let conn_cap = connections_limit.min(MAX_CONNECTIONS_LIMIT);
    let rule_cap = rules_limit.min(MAX_RULES_LIMIT);
    let conns: Vec<Value> = snap
        .connections
        .iter()
        .take(conn_cap)
        .map(connection_json)
        .collect();
    let rules: Vec<Value> = snap.rules.iter().take(rule_cap).map(rule_json).collect();

    let socks_json = socks_cfg.map(|cfg| {
        // Deliberately never exposes plaintext password; uses redacted SocksConfig only.
        json!({
            "enabled": cfg.enabled,
            "port": cfg.port,
            "listen": cfg.listen,
            "udp": cfg.udp,
            "outbound": cfg.outbound,
            "has_auth": cfg.has_auth,
            "username": cfg.username,
        })
    });

    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "availability": clash_availability_json(&snap.availability),
        "summary": {
            "version": snap.summary.version,
            "mode": snap.summary.mode,
            "mixed_port": snap.summary.mixed_port,
            "http_port": snap.summary.http_port,
            "socks_port": snap.summary.socks_port,
            "allow_lan": snap.summary.allow_lan,
            "tun_enabled": snap.summary.tun_enabled,
            "tun_stack": snap.summary.tun_stack,
            "controller_url": snap.summary.controller_url,
            "controller_has_secret": snap.summary.controller_has_secret,
            "uptime_secs": snap.summary.uptime_secs,
            "pid": snap.summary.pid,
            "socks5_listen": snap.summary.socks5_listen,
            "socks5_auth": snap.summary.socks5_auth,
            "panel_installed": snap.summary.panel_installed,
            "panel_name": snap.summary.panel_name,
            "panel_version": snap.summary.panel_version,
            "panel_url": snap.summary.panel_url,
            "panel_bind": snap.summary.panel_bind,
            "panel_loopback": snap.summary.panel_loopback,
            "active_profile": snap.summary.active_profile,
        },
        "socks": socks_json,
        "traffic": {
            "upload_total_bytes": snap.upload_total,
            "download_total_bytes": snap.download_total,
            "memory_bytes": snap.memory_bytes,
            "active_connections_total": snap.connections.len(),
            "tcp_connections": snap.tcp_count(),
            "udp_connections": snap.udp_count(),
        },
        "profiles": snap.profiles.iter().map(profile_summary_json).collect::<Vec<_>>(),
        "groups": snap.groups.iter().map(proxy_group_json).collect::<Vec<_>>(),
        "connections": conns,
        "connections_truncated": snap.connections.len() > conn_cap,
        "rules_fetched": snap.fetched_rules,
        "rules_total": snap.rules.len(),
        "rules": rules,
        "rules_truncated": snap.rules.len() > rule_cap,
        "providers_fetched": snap.fetched_providers,
        "proxy_providers": snap.proxy_providers.iter().map(|p| json!({
            "name": p.name,
            "vehicle_type": p.vehicle_type,
            "node_count": p.node_count,
            "updated_at": p.updated_at,
            "userinfo": p.userinfo,
        })).collect::<Vec<_>>(),
        "rule_providers": snap.rule_providers.iter().map(|p| json!({
            "name": p.name,
            "behavior": p.behavior,
            "vehicle_type": p.vehicle_type,
            "rule_count": p.rule_count,
            "updated_at": p.updated_at,
        })).collect::<Vec<_>>(),
        "last_error": snap.last_error,
    })
}

/// Captures a synchronous, redacted Mihomo snapshot.
pub fn mihomo_snapshot_json(
    ctx: &Context,
    include_rules: bool,
    include_providers: bool,
    connections_limit: Option<usize>,
    rules_limit: Option<usize>,
) -> Value {
    let probe = ctx.silent();
    let snap = clash_monitor::fetch_snapshot(&probe, include_rules, include_providers);
    let socks_cfg = socks::inspect(&probe).ok();
    serialize_mihomo_snapshot(
        &snap,
        socks_cfg.as_ref(),
        connections_limit.unwrap_or(DEFAULT_CONNECTIONS_LIMIT),
        rules_limit.unwrap_or(DEFAULT_RULES_LIMIT),
    )
}

/// Returns bounded, redacted Mihomo log lines.
pub fn mihomo_logs_json(ctx: &Context, lines: usize) -> Result<Value> {
    let bounded = lines.clamp(1, 500);
    let probe = ctx.silent();
    let entries = clash::read_redacted_lines(&probe, bounded)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "limit": bounded,
        "count": entries.len(),
        "lines": entries,
    }))
}

/// Checks for the latest CqShell release without modifying local files.
pub fn check_update_json(ctx: &Context, version: Option<&str>) -> Result<Value> {
    let probe = ctx.silent();
    let info = self_update::check(&probe, version, true)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "release": info,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::docker::model::{ContainerInspect, EnvVar, ImageInspect};

    #[test]
    fn docker_inspect_serializers_redact_sensitive_env_and_omit_raw_json() {
        let secret = "super-secret-token-value-12345";
        let container = ContainerInspect {
            id: "abc123456789".into(),
            name: "app".into(),
            image: "example.org/app:latest".into(),
            platform: "linux".into(),
            restart_policy: "unless-stopped".into(),
            restart_count: 0,
            created: "2026-01-01T00:00:00Z".into(),
            started_at: "2026-01-01T00:00:01Z".into(),
            finished_at: String::new(),
            exit_code: 0,
            entrypoint: vec!["/app".into()],
            cmd: vec![],
            working_dir: "/".into(),
            user: "1000".into(),
            ports: vec![],
            networks: vec![],
            mounts: vec![],
            env: vec![
                EnvVar {
                    key: "APP_ENV".into(),
                    value: "production".into(),
                    sensitive: false,
                },
                EnvVar {
                    key: "API_SECRET_KEY".into(),
                    value: secret.into(),
                    sensitive: true,
                },
            ],
            labels: Default::default(),
            health_status: None,
            health_logs: vec![],
            top: vec![],
            raw_json: format!(r#"{{"Env":["API_SECRET_KEY={secret}"]}}"#),
        };
        let encoded = serialize_container_inspect(&container);
        let text = serde_json::to_string(&encoded).unwrap();
        assert!(!text.contains(secret), "secret leaked: {text}");
        assert!(encoded.get("raw_json").is_none());
        assert_eq!(encoded["env"][0]["value"], "production");
        assert_eq!(encoded["env"][1]["value"], REDACTED);
        assert_eq!(encoded["env"][1]["redacted"], true);

        let image = ImageInspect {
            id: "sha256:1234".into(),
            repo_tags: vec!["example.org/app:latest".into()],
            os: "linux".into(),
            arch: "amd64".into(),
            size_bytes: 1024,
            exposed_ports: vec![],
            entrypoint: vec![],
            cmd: vec![],
            working_dir: "/".into(),
            env: vec![EnvVar {
                key: "DB_PASSWORD".into(),
                value: secret.into(),
                sensitive: true,
            }],
            labels: Default::default(),
            history: vec![],
            raw_json: format!(r#"{{"Env":["DB_PASSWORD={secret}"]}}"#),
        };
        let img_encoded = serialize_image_inspect(&image);
        let img_text = serde_json::to_string(&img_encoded).unwrap();
        assert!(
            !img_text.contains(secret),
            "image secret leaked: {img_text}"
        );
        assert!(img_encoded.get("raw_json").is_none());
        assert_eq!(img_encoded["env"][0]["value"], REDACTED);
    }

    #[test]
    fn headless_captured_context_disables_interactive_prompts_and_kernel_reboot() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let (cap, rx) = ctx.captured(false);
        assert!(!cap.can_prompt());
        assert_eq!(
            cap.confirm("confirm.action", "test").unwrap_err().key,
            "error.confirm"
        );
        // Even with yes=true, headless captured context must never invoke `reboot`.
        let (cap_yes, rx_yes) = ctx.captured(true);
        assert!(!cap_yes.can_prompt());
        crate::ops::kernel::reboot_after_install(&cap_yes, "6.12.0-test", "6.1.0-test").unwrap();
        drop(cap_yes);
        let logs = drain_logs(rx_yes);
        assert!(
            logs.lines
                .iter()
                .any(|line| line.contains("sudo reboot") || line.contains("reboot")),
            "{:?}",
            logs.lines
        );
        drop(cap);
        drop(rx);
    }

    #[test]
    fn mihomo_snapshot_serializer_redacts_secrets_and_caps_collections() {
        let mut snap = ClashSnapshot {
            availability: ClashAvailability::Ready,
            ..Default::default()
        };
        snap.summary.controller_url = Some("http://127.0.0.1:9090".into());
        snap.summary.controller_has_secret = true;
        snap.summary.socks5_auth = true;
        let socks_cfg = socks::SocksConfig {
            enabled: true,
            port: 10808,
            listen: "127.0.0.1".into(),
            udp: true,
            outbound: "direct".into(),
            has_auth: true,
            username: Some("cqs".into()),
        };
        let encoded = serialize_mihomo_snapshot(&snap, Some(&socks_cfg), 10, 20);
        assert_eq!(encoded["ok"], true);
        assert_eq!(encoded["summary"]["controller_has_secret"], true);
        assert_eq!(encoded["traffic"]["active_connections_total"], 0);
        assert_eq!(encoded["traffic"]["tcp_connections"], 0);
        assert_eq!(encoded["traffic"]["udp_connections"], 0);
        assert_eq!(encoded["socks"]["has_auth"], true);
        assert!(encoded["socks"].get("password").is_none());
        assert!(encoded["summary"].get("secret").is_none());
    }

    #[test]
    fn parse_docker_op_covers_all_21_operations_and_validation_errors() {
        let cases: &[(&str, Value, &str, ConfirmPolicy, bool)] = &[
            (
                "container_start",
                json!({"operation": "container_start", "ids": ["web-01"]}),
                "docker start web-01",
                ConfirmPolicy::None,
                false,
            ),
            (
                "container_stop",
                json!({"operation": "container_stop", "ids": "web-01"}),
                "docker stop web-01",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "container_restart",
                json!({"operation": "container_restart", "ids": ["web-01"]}),
                "docker restart web-01",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "container_pause",
                json!({"operation": "container_pause", "ids": ["web-01"]}),
                "docker pause web-01",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "container_unpause",
                json!({"operation": "container_unpause", "ids": ["web-01"]}),
                "docker unpause web-01",
                ConfirmPolicy::None,
                false,
            ),
            (
                "container_remove",
                json!({"operation": "container_remove", "ids": ["web-01"], "force": true}),
                "docker rm -f web-01",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "compose_up",
                json!({"operation": "compose_up", "project": "demo", "config_files": ["/srv/compose.yaml"]}),
                "docker compose -p demo -f /srv/compose.yaml up -d",
                ConfirmPolicy::None,
                false,
            ),
            (
                "compose_start",
                json!({"operation": "compose_start", "project": "demo"}),
                "docker compose -p demo start",
                ConfirmPolicy::None,
                false,
            ),
            (
                "compose_stop",
                json!({"operation": "compose_stop", "project": "demo"}),
                "docker compose -p demo stop",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "compose_restart",
                json!({"operation": "compose_restart", "project": "demo"}),
                "docker compose -p demo restart",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "compose_pull",
                json!({"operation": "compose_pull", "project": "demo", "config_files": ["/srv/compose.yaml"]}),
                "docker compose -p demo -f /srv/compose.yaml pull",
                ConfirmPolicy::None,
                false,
            ),
            (
                "compose_down",
                json!({"operation": "compose_down", "project": "demo", "volumes": true}),
                "docker compose -p demo down --volumes",
                ConfirmPolicy::Danger,
                false,
            ),
            (
                "image_pull",
                json!({"operation": "image_pull", "reference": "example.org/app:1.0"}),
                "docker pull example.org/app:1.0",
                ConfirmPolicy::None,
                false,
            ),
            (
                "image_remove",
                json!({"operation": "image_remove", "ids": ["sha256:abcdef"], "force": true}),
                "docker image rm -f sha256:abcdef",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "volume_remove",
                json!({"operation": "volume_remove", "names": ["pg-data"]}),
                "docker volume rm pg-data",
                ConfirmPolicy::Danger,
                false,
            ),
            (
                "network_create",
                json!({"operation": "network_create", "name": "app-net", "subnet": "192.0.2.0/24", "ipv6": true}),
                "docker network create --subnet 192.0.2.0/24 --ipv6 app-net",
                ConfirmPolicy::None,
                false,
            ),
            (
                "network_remove",
                json!({"operation": "network_remove", "ids": ["app-net"]}),
                "docker network rm app-net",
                ConfirmPolicy::YesNo,
                false,
            ),
            (
                "prune",
                json!({"operation": "prune", "containers": true, "images": true, "all_unused_images": true, "volumes": true, "networks": true, "build_cache": true}),
                "docker container prune -f && docker image prune -f -a && docker volume prune -f && docker network prune -f && docker builder prune -f",
                ConfirmPolicy::Danger,
                false,
            ),
            (
                "service_start",
                json!({"operation": "service_start"}),
                "systemctl start docker",
                ConfirmPolicy::None,
                true,
            ),
            (
                "service_stop",
                json!({"operation": "service_stop"}),
                "systemctl stop docker",
                ConfirmPolicy::YesNo,
                true,
            ),
            (
                "service_restart",
                json!({"operation": "service_restart"}),
                "systemctl restart docker",
                ConfirmPolicy::YesNo,
                true,
            ),
        ];
        assert_eq!(cases.len(), 21);
        for (name, args, expected_preview, expected_policy, expected_root) in cases {
            let op = parse_docker_op(args).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(op.cli_preview(), *expected_preview, "{name}");
            assert_eq!(op.confirm_policy(), *expected_policy, "{name}");
            assert_eq!(op.requires_root(), *expected_root, "{name}");
        }

        // Validation and input-type error paths
        for bad in [
            json!({}),
            json!({"operation": "unknown_op"}),
            json!({"operation": "container_start", "ids": []}),
            json!({"operation": "container_start", "ids": [123]}),
            json!({"operation": "container_start", "ids": {"a": 1}}),
            json!({"operation": "container_start", "ids": ["-flag"]}),
            json!({"operation": "prune"}),
        ] {
            assert!(parse_docker_op(&bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn docker_snapshot_inspect_logs_and_operate_guardrails_work_offline() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());

        // All 7 valid snapshot sections plus invalid section rejection
        for section in [
            "all",
            "containers",
            "compose",
            "images",
            "volumes",
            "networks",
            "daemon",
        ] {
            let snap = docker_snapshot_json(&ctx, section, false).unwrap();
            assert_eq!(snap["ok"], true);
            assert_eq!(snap["section"], section);
        }
        assert_eq!(
            docker_snapshot_json(&ctx, "bogus", false).unwrap_err().key,
            "error.input"
        );

        // Input validation on inspect & logs
        assert_eq!(
            docker_inspect_json(&ctx, "container", "-bad", false)
                .unwrap_err()
                .key,
            "error.input"
        );
        assert_eq!(
            docker_inspect_json(&ctx, "container", "bad\nid", false)
                .unwrap_err()
                .key,
            "error.input"
        );
        assert_eq!(
            docker_inspect_json(&ctx, "volume", "vol1", false)
                .unwrap_err()
                .key,
            "error.input"
        );
        assert_eq!(
            docker_logs_json(&ctx, "-bad", 20).unwrap_err().key,
            "error.input"
        );

        // execute_docker_op_json: invalid op, dry_run, confirmation_required, policy_denied
        let policy = ExecutionPolicy::mcp_default();
        let (invalid, code) = execute_docker_op_json(&ctx, &policy, &json!({}), false, false);
        assert_eq!(code, 2);
        assert_eq!(invalid["error"]["key"], "error.required");

        let (dry, code) = execute_docker_op_json(
            &ctx,
            &policy,
            &json!({"operation": "service_restart"}),
            false,
            true,
        );
        assert_eq!(code, 0);
        assert_eq!(dry["status"], "dry_run");
        assert_eq!(dry["operation"]["requires_root"], true);

        let (unconf, code) = execute_docker_op_json(
            &ctx,
            &policy,
            &json!({"operation": "network_create", "name": "demo-net"}),
            false,
            false,
        );
        assert_eq!(code, 1);
        assert_eq!(unconf["status"], "confirmation_required");

        let (denied, code) = execute_docker_op_json(
            &ctx,
            &policy,
            &json!({"operation": "prune", "volumes": true}),
            true,
            false,
        );
        assert_eq!(code, 1);
        assert_eq!(denied["status"], "policy_denied");
    }

    #[test]
    fn mihomo_logs_redaction_and_snapshot_truncation_and_status_text_work_offline() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        crate::storage::atomic_write(
            &ctx.paths.clash_mixin,
            b"secret: demo-controller-secret-42\n",
            0o600,
        )
        .unwrap();
        crate::storage::atomic_write(
            &ctx.paths.clash_log,
            b"level=info msg=\"ready\"\nlevel=info secret=demo-controller-secret-42 token=https://example.invalid/sub?token=abc123\n",
            0o600,
        )
        .unwrap();

        let logs = mihomo_logs_json(&ctx, 10).unwrap();
        assert_eq!(logs["ok"], true);
        assert_eq!(logs["count"], 2);
        let joined = serde_json::to_string(&logs["lines"]).unwrap();
        assert!(!joined.contains("demo-controller-secret-42"), "{joined}");
        assert!(!joined.contains("abc123"), "{joined}");

        // Snapshot TCP/UDP counts & collection caps
        let mut snap = ClashSnapshot {
            availability: ClashAvailability::Ready,
            fetched_rules: true,
            fetched_providers: true,
            ..Default::default()
        };
        for idx in 0..3 {
            snap.connections.push(ConnectionItem {
                id: format!("c-{idx}"),
                network: if idx == 2 { "udp".into() } else { "tcp".into() },
                conn_type: "Socks5".into(),
                source: "192.0.2.10:12345".into(),
                destination: "198.51.100.20:443".into(),
                host: "example.com".into(),
                process: "curl".into(),
                rule: "MATCH".into(),
                rule_payload: String::new(),
                chains: vec!["DIRECT".into()],
                exit_node: "DIRECT".into(),
                upload: 100,
                download: 200,
                start: "2026-01-01T00:00:00Z".into(),
                up_speed: 0,
                down_speed: 0,
                closed: false,
            });
            snap.rules.push(RuleItem {
                index: idx,
                kind: "DOMAIN".into(),
                payload: "example.com".into(),
                proxy: "DIRECT".into(),
            });
        }
        let encoded = serialize_mihomo_snapshot(&snap, None, 2, 1);
        assert_eq!(encoded["traffic"]["active_connections_total"], 3);
        assert_eq!(encoded["traffic"]["tcp_connections"], 2);
        assert_eq!(encoded["traffic"]["udp_connections"], 1);
        assert_eq!(encoded["connections"].as_array().unwrap().len(), 2);
        assert_eq!(encoded["connections_truncated"], true);
        assert_eq!(encoded["rules"].as_array().unwrap().len(), 1);
        assert_eq!(encoded["rules_truncated"], true);

        // System status JSON & text rendering
        let st_json = system_status_json(&ctx, false, false);
        assert_eq!(st_json["ok"], true);
        assert!(st_json["system"]["network"].get("connections").is_some());
        let st_text = system_status_text(&ctx, false, false);
        assert!(st_text.contains("CqShell"));
        assert!(st_text.contains(ctx.i18n.text("dash.connections")));
    }
}
