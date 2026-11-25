//! Pure native `docker` / `docker compose` CLI reader and tolerant JSON / NDJSON parsers.
//!
//! Zero external HTTP/async dependencies: respects `DOCKER_HOST`, Docker Contexts, and rootless
//! Docker out of the box while keeping bounded timeouts and `setsid` isolation.

use super::model::{
    ComposeProject, ContainerHealth, ContainerInspect, ContainerItem, ContainerState,
    ContainerStats, ContainerTopRow, DiskUsageRow, DockerAvailability, DockerSnapshot, DockerTab,
    EngineInfo, EnvVar, HealthLogEntry, ImageHistoryRow, ImageInspect, ImageItem, MountInfo,
    NetworkEndpoint, NetworkItem, VolumeItem, is_anonymous_volume_name, is_sensitive_env_key,
    parse_percent, parse_size_bytes, short_id,
};
use crate::{
    error::{Error, Result},
    runtime::{Context, available},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
const DF_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_COMPOSE_FILE_BYTES: u64 = 256 * 1024;
const MAX_RAW_INSPECT_BYTES: usize = 128 * 1024;

#[derive(Debug)]
struct CliResult {
    success: bool,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// Runs a read-only external command with a hard wall-clock timeout and `LC_ALL=C`.
fn run_readonly(
    ctx: &Context,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<CliResult> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(&ctx.environment)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = ctx.environment.get("PATH") {
        cmd.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = crate::runtime::retry_executable_busy(ctx, || cmd.spawn())
        .map_err(|e| Error::new("error.command", format!("{program}: {e}")))?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = out_pipe.take() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = err_pipe.take() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            terminate_child(&mut child);
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };

    let stdout = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();
    Ok(CliResult {
        success: !timed_out && status.is_some_and(|s| s.success()),
        stdout,
        stderr,
        timed_out,
    })
}

fn terminate_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}

/// Detects whether Docker is available on this host and gathers basic engine info.
pub fn probe_availability(ctx: &Context) -> (DockerAvailability, Option<EngineInfo>) {
    if let Err(err) = ctx.platform() {
        return (DockerAvailability::NotApplicable(err.detail), None);
    }
    if !available("docker") {
        return (DockerAvailability::NotInstalled, None);
    }

    let info_res = match run_readonly(
        ctx,
        "docker",
        &["info", "--format", "{{json .}}"],
        PROBE_TIMEOUT,
    ) {
        Ok(res) => res,
        Err(err) => return (DockerAvailability::Unavailable(err.detail), None),
    };

    let (daemon_mirrors, daemon_ipv6, daemon_ipv6_cidr, daemon_json_error) =
        read_daemon_json(&ctx.paths.docker);
    let service_active = check_docker_service_active(ctx);

    if !info_res.success {
        let diag = if info_res.timed_out {
            "docker info timed out".to_owned()
        } else {
            first_meaningful_line(&info_res.stderr)
                .or_else(|| first_meaningful_line(&info_res.stdout))
                .unwrap_or_else(|| "docker info failed".to_owned())
        };
        let lower = diag.to_ascii_lowercase();
        let avail =
            if lower.contains("permission denied") || lower.contains("got permission denied") {
                DockerAvailability::PermissionDenied(diag)
            } else if lower.contains("cannot connect to the docker daemon")
                || lower.contains("is the docker daemon running")
                || lower.contains("connection refused")
                || lower.contains("no such file or directory")
                || service_active == Some(false)
            {
                DockerAvailability::DaemonStopped(diag)
            } else {
                DockerAvailability::Unavailable(diag)
            };
        let partial = EngineInfo {
            service_active,
            daemon_mirrors,
            daemon_ipv6,
            daemon_ipv6_cidr,
            daemon_json_error,
            ..EngineInfo::default()
        };
        return (avail, Some(partial));
    }

    let mut engine = parse_engine_info(&info_res.stdout);
    engine.service_active = service_active.or(Some(true));
    engine.daemon_mirrors = daemon_mirrors;
    engine.daemon_ipv6 = daemon_ipv6;
    engine.daemon_ipv6_cidr = daemon_ipv6_cidr;
    engine.daemon_json_error = daemon_json_error;

    if let Ok(ver) = run_readonly(
        ctx,
        "docker",
        &["version", "--format", "{{.Server.APIVersion}}"],
        PROBE_TIMEOUT,
    ) && ver.success
    {
        let api = ver.stdout.trim();
        if !api.is_empty() && api != "<no value>" {
            engine.api_version = api.to_owned();
        }
    }
    if let Ok(comp) = run_readonly(
        ctx,
        "docker",
        &["compose", "version", "--short"],
        PROBE_TIMEOUT,
    ) && comp.success
    {
        let v = comp.stdout.trim();
        if !v.is_empty() {
            engine.compose_version = Some(v.to_owned());
        }
    }
    if let Ok(c) = run_readonly(ctx, "docker", &["context", "show"], PROBE_TIMEOUT)
        && c.success
    {
        let name = c.stdout.trim();
        if !name.is_empty() {
            engine.context = name.to_owned();
        }
    }

    (DockerAvailability::Ready, Some(engine))
}

fn check_docker_service_active(ctx: &Context) -> Option<bool> {
    let res = run_readonly(
        ctx,
        "systemctl",
        &["is-active", "docker"],
        Duration::from_secs(2),
    )
    .ok()?;
    let trimmed = res.stdout.trim();
    if res.success || trimmed == "active" {
        Some(true)
    } else if matches!(trimmed, "inactive" | "failed" | "deactivating" | "dead") {
        Some(false)
    } else {
        None
    }
}

fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn read_daemon_json(path: &Path) -> (Vec<String>, Option<bool>, Option<String>, Option<String>) {
    match fs::read_to_string(path) {
        Ok(raw) => parse_daemon_json(&raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None, None, None),
        Err(e) => (Vec::new(), None, None, Some(e.to_string())),
    }
}

/// Parses `/etc/docker/daemon.json` into `(registry_mirrors, ipv6, ipv6_cidr, parse_error)`.
pub fn parse_daemon_json(raw: &str) -> (Vec<String>, Option<bool>, Option<String>, Option<String>) {
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(err) => return (Vec::new(), None, None, Some(err.to_string())),
    };
    let Some(obj) = value.as_object() else {
        return (
            Vec::new(),
            None,
            None,
            Some("daemon.json is not a JSON object".into()),
        );
    };
    let mirrors = obj
        .get("registry-mirrors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let ipv6 = obj.get("ipv6").and_then(Value::as_bool);
    let cidr = obj
        .get("fixed-cidr-v6")
        .or_else(|| obj.get("bip6"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    (mirrors, ipv6, cidr, None)
}

/// Refreshes the active tab (and cross-linked essentials) into a new snapshot, preserving
/// previously collected tab data and sparkline histories so switching tabs feels instantaneous.
pub fn fetch_snapshot(
    ctx: &Context,
    active_tab: DockerTab,
    prev: &DockerSnapshot,
) -> DockerSnapshot {
    let mut snap = prev.clone();
    let (avail, engine) = probe_availability(ctx);
    snap.availability = avail;
    snap.engine = engine;
    snap.updated = Some(Instant::now());
    snap.last_error = None;

    if !snap.availability.is_ready() {
        return snap;
    }

    // Always refresh containers on Containers, Compose, Images, Volumes, or initial load so
    // cross-references (`used_by`, compose container counts, sidebar running count) stay exact.
    if matches!(
        active_tab,
        DockerTab::Containers | DockerTab::Compose | DockerTab::Images | DockerTab::Volumes
    ) || snap.containers.is_empty()
    {
        match run_readonly(
            ctx,
            "docker",
            &["ps", "-a", "--no-trunc", "--format", "{{json .}}"],
            LIST_TIMEOUT,
        ) {
            Ok(res) if res.success => {
                snap.containers = parse_containers_ndjson(&res.stdout);
            }
            Ok(res) => {
                snap.last_error = first_meaningful_line(&res.stderr);
            }
            Err(err) => snap.last_error = Some(err.detail),
        }
    }

    // Compose projects (when on Compose tab or initial load)
    if active_tab == DockerTab::Compose || snap.compose.is_empty() {
        let compose_supported = snap
            .engine
            .as_ref()
            .and_then(|e| e.compose_version.as_ref())
            .is_some();
        if compose_supported {
            if let Ok(res) = run_readonly(
                ctx,
                "docker",
                &["compose", "ls", "-a", "--format", "json"],
                LIST_TIMEOUT,
            ) && res.success
            {
                snap.compose = parse_compose_json(&res.stdout);
            }
        } else {
            snap.compose.clear();
        }
    }

    if (active_tab == DockerTab::Images || snap.images.is_empty())
        && let Ok(res) = run_readonly(
            ctx,
            "docker",
            &["image", "ls", "-a", "--no-trunc", "--format", "{{json .}}"],
            LIST_TIMEOUT,
        )
        && res.success
    {
        snap.images = parse_images_ndjson(&res.stdout);
    }

    if active_tab == DockerTab::Volumes || snap.volumes.is_empty() {
        if let Ok(res) = run_readonly(
            ctx,
            "docker",
            &["volume", "ls", "--format", "{{json .}}"],
            LIST_TIMEOUT,
        ) && res.success
        {
            snap.volumes = parse_volumes_ndjson(&res.stdout);
        }
        if let Ok(dfv) = run_readonly(
            ctx,
            "docker",
            &["system", "df", "-v", "--format", "{{json .}}"],
            DF_TIMEOUT,
        ) && dfv.success
        {
            let vol_sizes = parse_system_df_verbose_volumes(&dfv.stdout);
            for vol in &mut snap.volumes {
                if let Some((size_text, size_bytes, _links)) = vol_sizes.get(&vol.name) {
                    vol.size_text = size_text.clone();
                    vol.size_bytes = Some(*size_bytes);
                }
            }
        }
    }

    if (active_tab == DockerTab::Networks || snap.networks.is_empty())
        && let Ok(res) = run_readonly(
            ctx,
            "docker",
            &["network", "ls", "--no-trunc", "--format", "{{json .}}"],
            LIST_TIMEOUT,
        )
        && res.success
    {
        let mut nets = parse_networks_ndjson(&res.stdout);
        // Enrich custom/bridge networks with subnet & endpoint containers via batch inspect
        if !nets.is_empty() {
            let ids: Vec<&str> = nets.iter().map(|n| n.id.as_str()).collect();
            let mut args = vec!["network", "inspect"];
            args.extend(ids);
            if let Ok(insp) = run_readonly(ctx, "docker", &args, LIST_TIMEOUT)
                && insp.success
            {
                enrich_networks_from_inspect(&mut nets, &insp.stdout);
            }
        }
        snap.networks = nets;
    }

    if (active_tab == DockerTab::Daemon || snap.disk_usage.is_empty())
        && let Ok(res) = run_readonly(
            ctx,
            "docker",
            &["system", "df", "--format", "{{json .}}"],
            DF_TIMEOUT,
        )
        && res.success
    {
        snap.disk_usage = parse_system_df_ndjson(&res.stdout);
    }

    cross_link_snapshot(&mut snap);
    snap
}

/// Single-shot batch stats query for all running containers.
pub fn fetch_stats(ctx: &Context) -> Result<BTreeMap<String, ContainerStats>> {
    let res = run_readonly(
        ctx,
        "docker",
        &[
            "stats",
            "--no-stream",
            "--no-trunc",
            "--format",
            "{{json .}}",
        ],
        LIST_TIMEOUT,
    )?;
    if !res.success {
        return Err(Error::new(
            "error.command",
            first_meaningful_line(&res.stderr).unwrap_or_else(|| "docker stats failed".into()),
        ));
    }
    Ok(parse_stats_ndjson(&res.stdout))
}

/// Fetches detailed `docker inspect` (and `docker top` when running) for one container.
pub fn fetch_container_inspect(ctx: &Context, id: &str, running: bool) -> Result<ContainerInspect> {
    let res = run_readonly(ctx, "docker", &["inspect", id], LIST_TIMEOUT)?;
    if !res.success {
        return Err(Error::new(
            "error.command",
            first_meaningful_line(&res.stderr).unwrap_or_else(|| "docker inspect failed".into()),
        ));
    }
    let mut inspect = parse_container_inspect(&res.stdout)?;
    if running
        && let Ok(top_res) = run_readonly(
            ctx,
            "docker",
            &["top", id, "-eo", "user,pid,pcpu,pmem,etime,args"],
            PROBE_TIMEOUT,
        )
        && top_res.success
    {
        inspect.top = parse_docker_top(&top_res.stdout);
    }
    Ok(inspect)
}

/// Fetches detailed `docker image inspect` + `docker image history` for one image.
pub fn fetch_image_inspect(ctx: &Context, id: &str) -> Result<ImageInspect> {
    let res = run_readonly(ctx, "docker", &["image", "inspect", id], LIST_TIMEOUT)?;
    if !res.success {
        return Err(Error::new(
            "error.command",
            first_meaningful_line(&res.stderr)
                .unwrap_or_else(|| "docker image inspect failed".into()),
        ));
    }
    let hist = run_readonly(
        ctx,
        "docker",
        &[
            "image",
            "history",
            "--no-trunc",
            "--format",
            "{{json .}}",
            id,
        ],
        LIST_TIMEOUT,
    )
    .ok()
    .filter(|r| r.success)
    .map(|r| r.stdout)
    .unwrap_or_default();
    parse_image_inspect(&res.stdout, &hist)
}

/// Reads a Compose YAML file safely with path checks, a strict 256 KiB cap, and control sanitization.
pub fn read_compose_file(path: &str) -> Result<String> {
    use std::io::Read;
    let p = Path::new(path);
    crate::storage::check_path(p, true)?;
    let file = fs::File::open(p)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::new("error.input", format!("not a file: {path}")));
    }
    if meta.len() > MAX_COMPOSE_FILE_BYTES {
        return Err(Error::new(
            "error.data",
            format!("compose file exceeds 256 KiB limit: {path}"),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_COMPOSE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_COMPOSE_FILE_BYTES {
        return Err(Error::new(
            "error.data",
            format!("compose file exceeds 256 KiB limit: {path}"),
        ));
    }
    let raw = String::from_utf8(bytes).map_err(|e| Error::new("error.data", e))?;
    Ok(crate::runtime::sanitize(&raw))
}

/// Cross-links containers with Compose projects, images, and volumes so reference badges and
/// counts remain consistent even when the Compose CLI plugin is absent.
pub fn cross_link_snapshot(snap: &mut DockerSnapshot) {
    // 1. Cross-link Images `used_by`
    for img in &mut snap.images {
        img.used_by.clear();
    }
    for c in &snap.containers {
        for img in &mut snap.images {
            let ref_name = img.display_ref();
            let repo_only = img.repository.as_str();
            let matches = c.image == ref_name
                || (!img.dangling && img.tag == "latest" && c.image == repo_only)
                || c.image == img.id
                || c.image == img.short_id
                || img.id.ends_with(&c.image);
            if matches && !img.used_by.contains(&c.name) {
                img.used_by.push(c.name.clone());
            }
        }
    }

    // 2. Cross-link Volumes `used_by`
    for vol in &mut snap.volumes {
        vol.used_by.clear();
    }
    for c in &snap.containers {
        for vol in &mut snap.volumes {
            let mounted = c.mounts.iter().any(|m| {
                m == &vol.name
                    || (!vol.mountpoint.is_empty() && m == &vol.mountpoint)
                    || (!vol.name.is_empty()
                        && m.ends_with(&format!("/volumes/{}/_data", vol.name)))
            });
            if mounted && !vol.used_by.contains(&c.name) {
                vol.used_by.push(c.name.clone());
            }
        }
    }

    // 3. Cross-link Compose projects with containers + synthesize missing projects from labels
    snap.compose.retain(|proj| !proj.synthesized_from_labels);
    let mut by_project: BTreeMap<String, Vec<&ContainerItem>> = BTreeMap::new();
    for c in &snap.containers {
        if let Some(proj) = &c.compose_project {
            by_project.entry(proj.clone()).or_default().push(c);
        }
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for proj in &mut snap.compose {
        seen.insert(proj.name.clone());
        if let Some(members) = by_project.get(&proj.name) {
            proj.total_count = members.len();
            proj.running_count = members.iter().filter(|c| c.state.is_running()).count();
            proj.container_ids = members.iter().map(|c| c.id.clone()).collect();
            if proj.working_dir.is_none() {
                proj.working_dir = members.iter().find_map(|c| c.compose_working_dir.clone());
            }
            if proj.config_files.is_empty()
                && let Some(cf) = members.iter().find_map(|c| c.compose_config_files.clone())
            {
                proj.config_files = split_config_files(&cf);
            }
        } else {
            proj.total_count = 0;
            proj.running_count = 0;
            proj.container_ids.clear();
        }
    }
    for (name, members) in by_project {
        if seen.contains(&name) {
            continue;
        }
        let running = members.iter().filter(|c| c.state.is_running()).count();
        let total = members.len();
        let status = if running == total && total > 0 {
            format!("running({running})")
        } else if running > 0 {
            format!("running({running}), exited({})", total - running)
        } else {
            format!("exited({total})")
        };
        let working_dir = members.iter().find_map(|c| c.compose_working_dir.clone());
        let config_files = members
            .iter()
            .find_map(|c| c.compose_config_files.clone())
            .map(|cf| split_config_files(&cf))
            .unwrap_or_default();
        snap.compose.push(ComposeProject {
            name,
            status,
            config_files,
            working_dir,
            running_count: running,
            total_count: total,
            container_ids: members.iter().map(|c| c.id.clone()).collect(),
            synthesized_from_labels: true,
        });
    }
    snap.compose.sort_by(|a, b| a.name.cmp(&b.name));
}

fn split_config_files(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// JSON / NDJSON Parsers (independently unit-testable without a Docker daemon)
// ---------------------------------------------------------------------------

pub fn parse_engine_info(json: &str) -> EngineInfo {
    let val: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return EngineInfo::default(),
    };
    let s = |key: &str| {
        val.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let n = |key: &str| val.get(key).and_then(Value::as_u64).unwrap_or(0) as usize;
    EngineInfo {
        server_version: s("ServerVersion"),
        api_version: String::new(),
        compose_version: None,
        context: "default".into(),
        os_type: s("OSType"),
        arch: s("Architecture"),
        kernel: s("KernelVersion"),
        storage_driver: s("Driver"),
        cgroup_driver: s("CgroupDriver"),
        cgroup_version: s("CgroupVersion"),
        root_dir: s("DockerRootDir"),
        containers_running: n("ContainersRunning"),
        containers_paused: n("ContainersPaused"),
        containers_stopped: n("ContainersStopped"),
        images_total: n("Images"),
        service_active: Some(true),
        daemon_mirrors: Vec::new(),
        daemon_ipv6: None,
        daemon_ipv6_cidr: None,
        daemon_json_error: None,
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawContainer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "Command")]
    command: String,
    #[serde(rename = "CreatedAt")]
    created_at: String,
    #[serde(rename = "RunningFor")]
    running_for: String,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Ports")]
    ports: String,
    #[serde(rename = "Networks")]
    networks: String,
    #[serde(rename = "Mounts")]
    mounts: String,
    #[serde(rename = "Size")]
    size: String,
    #[serde(rename = "Labels")]
    labels: String,
}

pub fn parse_containers_ndjson(raw: &str) -> Vec<ContainerItem> {
    let mut items = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<RawContainer>(line) else {
            continue;
        };
        if row.id.is_empty() {
            continue;
        }
        let labels = parse_csv_labels(&row.labels);
        let name = row
            .names
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches('/')
            .to_owned();
        let ports = row
            .ports
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let networks = row
            .networks
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let mounts: Vec<String> = row
            .mounts
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let mounts_count = mounts.len();
        let state = ContainerState::parse(&row.state);
        let health = ContainerHealth::from_status(&row.status);
        let size_bytes = parse_size_bytes(&row.size);
        let compose_project = labels.get("com.docker.compose.project").cloned();
        let compose_service = labels.get("com.docker.compose.service").cloned();
        let compose_working_dir = labels
            .get("com.docker.compose.project.working_dir")
            .cloned();
        let compose_config_files = labels
            .get("com.docker.compose.project.config_files")
            .cloned();
        items.push(ContainerItem {
            short_id: short_id(&row.id),
            id: row.id,
            name,
            image: row.image,
            command: row.command.trim_matches('"').to_owned(),
            created_at: row.created_at,
            running_for: row.running_for,
            status: row.status,
            state,
            health,
            ports,
            networks,
            mounts,
            mounts_count,
            size: row.size,
            size_bytes,
            compose_project,
            compose_service,
            compose_working_dir,
            compose_config_files,
            labels,
        });
    }
    items
}

fn parse_csv_labels(raw: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in raw.split(',') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                map.insert(k.to_owned(), v.trim().to_owned());
            }
        }
    }
    map
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawCompose {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "ConfigFiles")]
    config_files: String,
}

/// Parses `docker compose ls -a --format json` across both JSON array and NDJSON CLI versions.
pub fn parse_compose_json(raw: &str) -> Vec<ComposeProject> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let rows: Vec<RawCompose> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).unwrap_or_default()
    } else {
        trimmed
            .lines()
            .filter_map(|l| serde_json::from_str::<RawCompose>(l.trim()).ok())
            .collect()
    };
    rows.into_iter()
        .filter(|r| !r.name.is_empty())
        .map(|r| {
            let config_files = split_config_files(&r.config_files);
            let working_dir = config_files
                .first()
                .and_then(|p| Path::new(p).parent())
                .map(|p| p.display().to_string());
            let (running_count, total_count) = parse_compose_status_counts(&r.status);
            ComposeProject {
                name: r.name,
                status: r.status,
                config_files,
                working_dir,
                running_count,
                total_count,
                container_ids: Vec::new(),
                synthesized_from_labels: false,
            }
        })
        .collect()
}

fn parse_compose_status_counts(status: &str) -> (usize, usize) {
    let mut running = 0usize;
    let mut total = 0usize;
    for part in status.split(',') {
        let part = part.trim().to_ascii_lowercase();
        if let Some((state, rest)) = part.split_once('(')
            && let Some(num_str) = rest.strip_suffix(')')
            && let Ok(n) = num_str.trim().parse::<usize>()
        {
            total += n;
            if state.trim() == "running" {
                running += n;
            }
        }
    }
    (running, total)
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawImage {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Repository")]
    repository: String,
    #[serde(rename = "Tag")]
    tag: String,
    #[serde(rename = "CreatedSince")]
    created_since: String,
    #[serde(rename = "CreatedAt")]
    created_at: String,
    #[serde(rename = "Size")]
    size: String,
}

pub fn parse_images_ndjson(raw: &str) -> Vec<ImageItem> {
    let mut items = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<RawImage>(line) else {
            continue;
        };
        if row.id.is_empty() {
            continue;
        }
        let dangling = row.repository == "<none>" || row.tag == "<none>";
        let size_bytes = parse_size_bytes(&row.size).unwrap_or(0);
        items.push(ImageItem {
            short_id: short_id(&row.id),
            id: row.id,
            repository: row.repository,
            tag: row.tag,
            created_since: row.created_since,
            created_at: row.created_at,
            size_text: row.size,
            size_bytes,
            dangling,
            used_by: Vec::new(),
        });
    }
    items
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawVolume {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Driver")]
    driver: String,
    #[serde(rename = "Scope")]
    scope: String,
    #[serde(rename = "Mountpoint")]
    mountpoint: String,
    #[serde(rename = "Labels")]
    labels: String,
}

pub fn parse_volumes_ndjson(raw: &str) -> Vec<VolumeItem> {
    let mut items = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<RawVolume>(line) else {
            continue;
        };
        if row.name.is_empty() {
            continue;
        }
        let anonymous = is_anonymous_volume_name(&row.name);
        items.push(VolumeItem {
            name: row.name,
            driver: row.driver,
            scope: row.scope,
            mountpoint: row.mountpoint,
            size_text: "-".into(),
            size_bytes: None,
            anonymous,
            used_by: Vec::new(),
            labels: parse_csv_labels(&row.labels),
        });
    }
    items
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawNetwork {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Driver")]
    driver: String,
    #[serde(rename = "Scope")]
    scope: String,
    #[serde(rename = "IPv6")]
    ipv6: String,
    #[serde(rename = "Internal")]
    internal: String,
    #[serde(rename = "Labels")]
    labels: String,
}

pub fn parse_networks_ndjson(raw: &str) -> Vec<NetworkItem> {
    let mut items = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<RawNetwork>(line) else {
            continue;
        };
        if row.id.is_empty() && row.name.is_empty() {
            continue;
        }
        let builtin = matches!(row.name.as_str(), "bridge" | "host" | "none");
        items.push(NetworkItem {
            short_id: short_id(&row.id),
            id: row.id,
            name: row.name,
            driver: row.driver,
            scope: row.scope,
            ipv6: row.ipv6.eq_ignore_ascii_case("true"),
            internal: row.internal.eq_ignore_ascii_case("true"),
            builtin,
            subnet: String::new(),
            gateway: String::new(),
            ipv6_subnet: String::new(),
            containers: Vec::new(),
            labels: parse_csv_labels(&row.labels),
        });
    }
    items
}

pub fn enrich_networks_from_inspect(nets: &mut [NetworkItem], inspect_json: &str) {
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(inspect_json) else {
        return;
    };
    for entry in arr {
        let id = entry.get("Id").and_then(Value::as_str).unwrap_or_default();
        let name = entry
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(net) = nets
            .iter_mut()
            .find(|n| (!id.is_empty() && n.id == id) || n.name == name)
        else {
            continue;
        };
        if let Some(en) = entry.get("EnableIPv6").and_then(Value::as_bool) {
            net.ipv6 = en;
        }
        if let Some(configs) = entry.pointer("/IPAM/Config").and_then(Value::as_array) {
            for cfg in configs {
                let subnet = cfg
                    .get("Subnet")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let gw = cfg
                    .get("Gateway")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if subnet.contains(':') {
                    if net.ipv6_subnet.is_empty() {
                        net.ipv6_subnet = subnet.to_owned();
                    }
                } else {
                    if net.subnet.is_empty() {
                        net.subnet = subnet.to_owned();
                    }
                    if net.gateway.is_empty() {
                        net.gateway = gw.to_owned();
                    }
                }
            }
        }
        net.containers.clear();
        if let Some(map) = entry.get("Containers").and_then(Value::as_object) {
            for (_cid, cval) in map {
                let cname = cval
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let ip4 = cval
                    .get("IPv4Address")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ip6 = cval
                    .get("IPv6Address")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let addr = match (ip4.is_empty(), ip6.is_empty()) {
                    (false, false) => format!("{ip4}, {ip6}"),
                    (false, true) => ip4.to_owned(),
                    (true, false) => ip6.to_owned(),
                    (true, true) => "-".to_owned(),
                };
                if !cname.is_empty() {
                    net.containers.push((cname, addr));
                }
            }
            net.containers.sort();
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawStats {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "CPUPerc")]
    cpu_perc: String,
    #[serde(rename = "MemUsage")]
    mem_usage: String,
    #[serde(rename = "MemPerc")]
    mem_perc: String,
    #[serde(rename = "NetIO")]
    net_io: String,
    #[serde(rename = "BlockIO")]
    block_io: String,
    #[serde(rename = "PIDs")]
    pids: String,
}

pub fn parse_stats_ndjson(raw: &str) -> BTreeMap<String, ContainerStats> {
    let mut map = BTreeMap::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<RawStats>(line) else {
            continue;
        };
        if row.id.is_empty() {
            continue;
        }
        let (mem_used_bytes, mem_limit_bytes) = match row.mem_usage.split_once('/') {
            Some((used, limit)) => (
                parse_size_bytes(used).unwrap_or(0),
                parse_size_bytes(limit).unwrap_or(0),
            ),
            None => (parse_size_bytes(&row.mem_usage).unwrap_or(0), 0),
        };
        let stats = ContainerStats {
            container_id: row.id.clone(),
            cpu_percent: parse_percent(&row.cpu_perc).unwrap_or(0.0),
            mem_used_bytes,
            mem_limit_bytes,
            mem_percent: parse_percent(&row.mem_perc).unwrap_or(0.0),
            mem_usage_text: row.mem_usage,
            net_io_text: row.net_io,
            block_io_text: row.block_io,
            pids: row.pids.trim().parse().unwrap_or(0),
        };
        map.insert(row.id, stats);
    }
    map
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawDiskUsage {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "TotalCount")]
    total_count: String,
    #[serde(rename = "Active")]
    active_count: String,
    #[serde(rename = "Size")]
    size: String,
    #[serde(rename = "Reclaimable")]
    reclaimable: String,
}

pub fn parse_system_df_ndjson(raw: &str) -> Vec<DiskUsageRow> {
    let mut rows = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(r) = serde_json::from_str::<RawDiskUsage>(line) else {
            continue;
        };
        if r.kind.is_empty() {
            continue;
        }
        rows.push(DiskUsageRow {
            kind: r.kind,
            total_count: r.total_count.trim().parse().unwrap_or(0),
            active_count: r.active_count.trim().parse().unwrap_or(0),
            size_bytes: parse_size_bytes(&r.size).unwrap_or(0),
            size_text: r.size,
            reclaimable_bytes: parse_size_bytes(&r.reclaimable).unwrap_or(0),
            reclaimable_text: r.reclaimable,
        });
    }
    rows
}

/// Extracts volume name -> `(size_text, size_bytes, links)` from `docker system df -v --format '{{json .}}'`.
pub fn parse_system_df_verbose_volumes(raw: &str) -> BTreeMap<String, (String, u64, usize)> {
    let mut out = BTreeMap::new();
    let Ok(val) = serde_json::from_str::<Value>(raw.trim()) else {
        return out;
    };
    let Some(vols) = val.get("Volumes").and_then(Value::as_array) else {
        return out;
    };
    for v in vols {
        let Some(name) = v.get("Name").and_then(Value::as_str) else {
            continue;
        };
        let size_text = v
            .get("Size")
            .and_then(Value::as_str)
            .unwrap_or("0B")
            .to_owned();
        let size_bytes = parse_size_bytes(&size_text).unwrap_or(0);
        let links = v
            .get("Links")
            .and_then(|x| {
                x.as_u64()
                    .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0) as usize;
        out.insert(name.to_owned(), (size_text, size_bytes, links));
    }
    out
}

pub fn parse_container_inspect(raw: &str) -> Result<ContainerInspect> {
    let val: Value = serde_json::from_str(raw)?;
    let obj = val.as_array().and_then(|arr| arr.first()).unwrap_or(&val);
    let str_at = |ptr: &str| {
        obj.pointer(ptr)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let str_vec = |ptr: &str| -> Vec<String> {
        obj.pointer(ptr)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut ports = Vec::new();
    if let Some(map) = obj
        .pointer("/NetworkSettings/Ports")
        .and_then(Value::as_object)
    {
        for (container_port, bindings) in map {
            if let Some(arr) = bindings.as_array() {
                for b in arr {
                    let host_ip = b.get("HostIp").and_then(Value::as_str).unwrap_or("0.0.0.0");
                    let host_port = b.get("HostPort").and_then(Value::as_str).unwrap_or("");
                    if !host_port.is_empty() {
                        ports.push(format!("{host_ip}:{host_port}->{container_port}"));
                    }
                }
            } else {
                ports.push(container_port.clone());
            }
        }
    }
    ports.sort();

    let mut networks = Vec::new();
    if let Some(map) = obj
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for (name, net) in map {
            networks.push(NetworkEndpoint {
                name: name.clone(),
                ipv4: net
                    .get("IPAddress")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                ipv6: net
                    .get("GlobalIPv6Address")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                mac: net
                    .get("MacAddress")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                gateway: net
                    .get("Gateway")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
    }

    let mut mounts = Vec::new();
    if let Some(arr) = obj.get("Mounts").and_then(Value::as_array) {
        for m in arr {
            let kind = m
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or("bind")
                .to_owned();
            let source = m
                .get("Name")
                .or_else(|| m.get("Source"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let destination = m
                .get("Destination")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let rw = m.get("RW").and_then(Value::as_bool).unwrap_or(true);
            let mode = m
                .get("Mode")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| if rw { "rw".into() } else { "ro".into() });
            mounts.push(MountInfo {
                kind,
                source,
                destination,
                mode,
                rw,
            });
        }
    }

    let env = str_vec("/Config/Env")
        .into_iter()
        .map(|entry| {
            let (k, v) = entry
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .unwrap_or((entry, String::new()));
            let sensitive = is_sensitive_env_key(&k);
            EnvVar {
                key: k,
                value: v,
                sensitive,
            }
        })
        .collect();

    let mut labels = BTreeMap::new();
    if let Some(map) = obj.pointer("/Config/Labels").and_then(Value::as_object) {
        for (k, v) in map {
            if let Some(vs) = v.as_str() {
                labels.insert(k.clone(), vs.to_owned());
            }
        }
    }

    let health_status = obj
        .pointer("/State/Health/Status")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut health_logs = Vec::new();
    if let Some(arr) = obj.pointer("/State/Health/Log").and_then(Value::as_array) {
        for item in arr.iter().rev().take(5) {
            health_logs.push(HealthLogEntry {
                exit_code: item.get("ExitCode").and_then(Value::as_i64).unwrap_or(0),
                output: item
                    .get("Output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
            });
        }
    }

    let pretty = serde_json::to_string_pretty(obj).unwrap_or_default();
    let raw_json = cap_utf8(&pretty, MAX_RAW_INSPECT_BYTES);

    Ok(ContainerInspect {
        id: str_at("/Id"),
        name: str_at("/Name").trim_start_matches('/').to_owned(),
        image: str_at("/Config/Image"),
        platform: str_at("/Platform"),
        restart_policy: str_at("/HostConfig/RestartPolicy/Name"),
        restart_count: obj.get("RestartCount").and_then(Value::as_u64).unwrap_or(0),
        created: str_at("/Created"),
        started_at: str_at("/State/StartedAt"),
        finished_at: str_at("/State/FinishedAt"),
        exit_code: obj
            .pointer("/State/ExitCode")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        entrypoint: str_vec("/Config/Entrypoint"),
        cmd: str_vec("/Config/Cmd"),
        working_dir: str_at("/Config/WorkingDir"),
        user: str_at("/Config/User"),
        ports,
        networks,
        mounts,
        env,
        labels,
        health_status,
        health_logs,
        top: Vec::new(),
        raw_json,
    })
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawImageHistory {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "CreatedSince")]
    created_since: String,
    #[serde(rename = "CreatedBy")]
    created_by: String,
    #[serde(rename = "Size")]
    size: String,
}

pub fn parse_image_inspect(raw: &str, history_ndjson: &str) -> Result<ImageInspect> {
    let val: Value = serde_json::from_str(raw)?;
    let obj = val.as_array().and_then(|arr| arr.first()).unwrap_or(&val);
    let str_at = |ptr: &str| {
        obj.pointer(ptr)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let str_vec = |ptr: &str| -> Vec<String> {
        obj.pointer(ptr)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    let exposed_ports = obj
        .pointer("/Config/ExposedPorts")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let env = str_vec("/Config/Env")
        .into_iter()
        .map(|entry| {
            let (k, v) = entry
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .unwrap_or((entry, String::new()));
            let sensitive = is_sensitive_env_key(&k);
            EnvVar {
                key: k,
                value: v,
                sensitive,
            }
        })
        .collect();

    let mut labels = BTreeMap::new();
    if let Some(map) = obj.pointer("/Config/Labels").and_then(Value::as_object) {
        for (k, v) in map {
            if let Some(vs) = v.as_str() {
                labels.insert(k.clone(), vs.to_owned());
            }
        }
    }

    let history = history_ndjson
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<RawImageHistory>(l).ok())
        .map(|h| ImageHistoryRow {
            id: short_id(&h.id),
            created_since: h.created_since,
            created_by: h.created_by,
            size_bytes: parse_size_bytes(&h.size).unwrap_or(0),
            size_text: h.size,
        })
        .collect();

    let pretty = serde_json::to_string_pretty(obj).unwrap_or_default();
    Ok(ImageInspect {
        id: str_at("/Id"),
        repo_tags: str_vec("/RepoTags"),
        os: str_at("/Os"),
        arch: str_at("/Architecture"),
        size_bytes: obj.get("Size").and_then(Value::as_u64).unwrap_or(0),
        exposed_ports,
        entrypoint: str_vec("/Config/Entrypoint"),
        cmd: str_vec("/Config/Cmd"),
        working_dir: str_at("/Config/WorkingDir"),
        env,
        labels,
        history,
        raw_json: cap_utf8(&pretty, MAX_RAW_INSPECT_BYTES),
    })
}

pub fn parse_docker_top(raw: &str) -> Vec<ContainerTopRow> {
    fn next_field(rest: &mut &str) -> String {
        let s = rest.trim_start();
        match s.find(char::is_whitespace) {
            Some(pos) => {
                *rest = &s[pos..];
                s[..pos].to_owned()
            }
            None => {
                *rest = "";
                s.to_owned()
            }
        }
    }

    let mut rows = Vec::new();
    for line in raw.lines().skip(1) {
        let mut rest = line.trim();
        if rest.is_empty() {
            continue;
        }
        let uid = next_field(&mut rest);
        let pid = next_field(&mut rest);
        let cpu = next_field(&mut rest);
        let mem = next_field(&mut rest);
        let time = next_field(&mut rest);
        let command = rest.trim_start().to_owned();
        if !pid.is_empty() {
            rows.push(ContainerTopRow {
                uid,
                pid,
                cpu,
                mem,
                time,
                command,
            });
        }
    }
    rows
}

fn cap_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (truncated)", &s[..end])
}
