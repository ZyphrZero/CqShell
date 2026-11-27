//! MCP Tool definitions and dispatchers backed by `crate::api`.

use super::{ERR_INVALID_PARAMS, RpcError};
use crate::{
    actions,
    api::{
        self, ActionFilter, ExecutionPolicy, error_envelope,
        snapshots::{
            check_update_json, docker_inspect_json, docker_logs_json, docker_snapshot_json,
            execute_docker_op_json, mihomo_logs_json, mihomo_snapshot_json, system_status_json,
        },
    },
    runtime::Context,
};
use serde_json::{Value, json};

pub struct ToolOutput {
    pub structured: Value,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(structured: Value) -> Self {
        Self {
            structured,
            is_error: false,
        }
    }

    fn err(structured: Value) -> Self {
        Self {
            structured,
            is_error: true,
        }
    }

    fn from_execution(payload: Value) -> Self {
        let status = payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        let is_error = !payload.get("ok").and_then(Value::as_bool).unwrap_or(false)
            && status != "confirmation_required";
        Self {
            structured: payload,
            is_error,
        }
    }
}

/// Returns the MCP tool descriptors exposed to clients under `policy`.
pub fn list_tools(policy: &ExecutionPolicy) -> Vec<Value> {
    let mut list = vec![
        json!({
            "name": "cqs_system_status",
            "title": "System Status Snapshot",
            "description": "Read host OS, kernel, CPU, memory, cgroup memory limit, swap, disk, uptime, timezone, BBR/qdisc/TCP state, Mihomo summary, and process privileges. Never contacts public networks unless include_public_ip=true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_public_ip": {
                        "type": "boolean",
                        "description": "Query public IPv4, IPv6, and ISP over HTTPS (default false).",
                        "default": false
                    },
                    "sample_cpu": {
                        "type": "boolean",
                        "description": "Wait ~220ms for a second sysinfo sample to compute live CPU usage percentage (default false).",
                        "default": false
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_list_actions",
            "title": "List CqShell Actions",
            "description": "Browse or search CqShell's structured action catalog (system, network, kernel, ssh, docker, tools, ai, terminal, tests, app, clash) with risk tiers and policy availability.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "group": {
                        "type": "string",
                        "description": "Filter by group ID.",
                        "enum": actions::GROUPS
                    },
                    "query": {
                        "type": "string",
                        "description": "Case-insensitive keyword filter matching action ID, group, label, or description."
                    },
                    "mutates": {
                        "type": "boolean",
                        "description": "Filter by whether the action mutates host or user state."
                    },
                    "root": {
                        "type": "boolean",
                        "description": "Filter by whether the action requires root privileges."
                    },
                    "non_interactive_only": {
                        "type": "boolean",
                        "description": "When true, omit actions that require an interactive TTY.",
                        "default": false
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_describe_action",
            "title": "Describe Action Schema",
            "description": "Inspect full metadata, risk tier, defaults, dynamic parameter sources, and JSON Schema for one action ID before calling cqs_run_action.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action_id": {
                        "type": "string",
                        "description": "Stable CqShell action ID (e.g. 'bbr.enable', 'clash.proxy.select')."
                    }
                },
                "required": ["action_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_list_action_choices",
            "title": "List Dynamic Action Choices",
            "description": "Read live host choices for an action's listed parameters (saved Clash subscriptions, active Mihomo proxy groups and member nodes, bootable or removable kernels).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action_id": {
                        "type": "string",
                        "description": "Action ID whose parameter choices to enumerate (e.g. 'clash.proxy.select', 'kernel.boot')."
                    }
                },
                "required": ["action_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_run_action",
            "title": if policy.read_only { "Run Read-Only Action" } else { "Run or Preview CqShell Action" },
            "description": if policy.read_only {
                "Execute a read-only CqShell action (mutates=false) and return captured logs and status. Mutating actions are blocked by --read-only."
            } else {
                "Preview (dry_run=true) or execute one non-interactive CqShell action. Read-only actions run immediately; mutating actions return a confirmation_required preview unless confirm=true is explicitly passed. High-risk/danger actions require --allow-danger."
            },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action_id": {
                        "type": "string",
                        "description": "Stable CqShell action ID."
                    },
                    "parameters": {
                        "type": "object",
                        "description": "Key-value map of action parameters (strings, numbers, or booleans; omitted keys use action defaults).",
                        "additionalProperties": {
                            "type": ["string", "number", "boolean", "null"]
                        }
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "Explicitly confirm mutating action execution (default false; omitted or false returns confirmation_required preview for mutating actions).",
                        "default": false
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "Validate parameters and run preflight checks without executing the action (default false).",
                        "default": false
                    }
                },
                "required": ["action_id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": policy.read_only,
                "destructiveHint": !policy.read_only,
                "idempotentHint": false,
                "openWorldHint": true
            }
        }),
        json!({
            "name": "cqs_run_batch",
            "title": if policy.read_only { "Run Read-Only Action Batch" } else { "Run or Preview Action Batch" },
            "description": if policy.read_only {
                "Run an ordered list of read-only CqShell actions and return per-step timings and logs. Mutating actions are blocked by --read-only."
            } else {
                "Preview or execute an ordered list of CqShell actions. Validates all IDs and parameters up front, enforces policy, and requires confirm=true if any job mutates state."
            },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "jobs": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Ordered list of action jobs.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "action_id": { "type": "string" },
                                "parameters": {
                                    "type": "object",
                                    "additionalProperties": {
                                        "type": ["string", "number", "boolean", "null"]
                                    }
                                }
                            },
                            "required": ["action_id"],
                            "additionalProperties": false
                        }
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "Explicitly confirm execution when any job mutates state (default false).",
                        "default": false
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "Validate and preflight all jobs without executing them (default false).",
                        "default": false
                    }
                },
                "required": ["jobs"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": policy.read_only,
                "destructiveHint": !policy.read_only,
                "idempotentHint": false,
                "openWorldHint": true
            }
        }),
        json!({
            "name": "cqs_docker_snapshot",
            "title": "Docker Read-Only Snapshot",
            "description": "Read structured Docker Engine status, containers, Compose projects, images, volumes, networks, daemon.json mirrors/IPv6, disk usage, and optional container stats.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "section": {
                        "type": "string",
                        "description": "Subset to query (default 'all').",
                        "enum": ["all", "containers", "compose", "images", "volumes", "networks", "daemon"],
                        "default": "all"
                    },
                    "include_stats": {
                        "type": "boolean",
                        "description": "Also query one-shot `docker stats --no-stream` for running containers (default false).",
                        "default": false
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_docker_inspect",
            "title": "Inspect Docker Container or Image",
            "description": "Inspect one Docker container (ports, networks, mounts, health, top processes, redacted env) or image (tags, layers, redacted env). Sensitive environment variables are always masked.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["container", "image"],
                        "description": "Resource type to inspect."
                    },
                    "id": {
                        "type": "string",
                        "description": "Container ID/name or image ID/reference."
                    },
                    "running": {
                        "type": "boolean",
                        "description": "For containers: include `docker top` process list (default false).",
                        "default": false
                    }
                },
                "required": ["kind", "id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_docker_logs",
            "title": "Read Docker Container Logs",
            "description": "Fetch a bounded tail of ANSI-sanitized logs for one Docker container (no live follow stream).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Container ID or name."
                    },
                    "tail": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1000,
                        "default": 100,
                        "description": "Number of recent log lines to return (1..=1000, default 100)."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_mihomo_snapshot",
            "title": "Mihomo / Clash Snapshot",
            "description": "Read structured, redacted Mihomo runtime state: core status, mode, ports, TUN, panel, redacted SOCKS5 config, profiles with quota, policy groups and node delays, active connections, and optional rules/providers. Never exposes secrets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_rules": {
                        "type": "boolean",
                        "default": false,
                        "description": "Also fetch routing rules from the controller (default false)."
                    },
                    "include_providers": {
                        "type": "boolean",
                        "default": false,
                        "description": "Also fetch external proxy and rule providers (default false)."
                    },
                    "connections_limit": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 500,
                        "default": 100,
                        "description": "Maximum active connections to include (default 100, max 500)."
                    },
                    "rules_limit": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 2000,
                        "default": 200,
                        "description": "Maximum routing rules to include when include_rules=true (default 200, max 2000)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_mihomo_logs",
            "title": "Read Redacted Mihomo Logs",
            "description": "Read the last N lines of Mihomo logs with secrets and tokens redacted (1..=500, default 100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lines": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "default": 100,
                        "description": "Number of recent log lines to return (default 100, max 500)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "cqs_check_update",
            "title": "Check CqShell Release Update",
            "description": "Check GitHub (ZyphrZero/CqShell) for the latest CqShell release without modifying local files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "version": {
                        "type": "string",
                        "description": "Optional specific release tag to inspect (e.g. 'v0.2.1')."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        }),
    ];

    if !policy.read_only
        && (policy.allow_groups.is_empty() || policy.allow_groups.iter().any(|g| g == "docker"))
    {
        list.push(json!({
            "name": "cqs_docker_operate",
            "title": "Execute Controlled Docker Operation",
            "description": "Preview (dry_run=true) or execute a typed Docker mutation using validated argument arrays (container start/stop/restart/pause/unpause/remove, compose up/start/stop/restart/pull/down, image pull/remove, volume remove, network create/remove, prune, service start/stop/restart). Requires confirm=true to execute; danger operations (volume removal, bulk >=3 removals, volume prune) also require --allow-danger.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": [
                            "container_start", "container_stop", "container_restart",
                            "container_pause", "container_unpause", "container_remove",
                            "compose_up", "compose_start", "compose_stop",
                            "compose_restart", "compose_pull", "compose_down",
                            "image_pull", "image_remove",
                            "volume_remove",
                            "network_create", "network_remove",
                            "prune",
                            "service_start", "service_stop", "service_restart"
                        ]
                    },
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target container, image, or network IDs/names."
                    },
                    "names": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Target volume names (for volume_remove)."
                    },
                    "name": {
                        "type": "string",
                        "description": "Network name (for network_create)."
                    },
                    "subnet": {
                        "type": "string",
                        "description": "Optional CIDR subnet (for network_create)."
                    },
                    "ipv6": {
                        "type": "boolean",
                        "description": "Enable IPv6 on created network (for network_create)."
                    },
                    "reference": {
                        "type": "string",
                        "description": "Image reference (for image_pull)."
                    },
                    "project": {
                        "type": "string",
                        "description": "Compose project name."
                    },
                    "config_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Compose YAML paths."
                    },
                    "force": { "type": "boolean", "default": false },
                    "volumes": { "type": "boolean", "default": false },
                    "containers": { "type": "boolean", "default": false },
                    "images": { "type": "boolean", "default": false },
                    "all_unused_images": { "type": "boolean", "default": false },
                    "networks": { "type": "boolean", "default": false },
                    "build_cache": { "type": "boolean", "default": false },
                    "confirm": {
                        "type": "boolean",
                        "default": false,
                        "description": "Explicitly confirm execution (default false returns confirmation_required with exact cli_preview)."
                    },
                    "dry_run": {
                        "type": "boolean",
                        "default": false,
                        "description": "Return cli_preview and risk classification without executing."
                    }
                },
                "required": ["operation"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": true
            }
        }));
    }

    list
}

fn opt_bool(args: &Value, key: &str, default: bool) -> std::result::Result<bool, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(RpcError::new(
            ERR_INVALID_PARAMS,
            format!("parameter '{key}' must be a boolean"),
        )),
    }
}

fn opt_str<'a>(args: &'a Value, key: &str) -> std::result::Result<Option<&'a str>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        _ => Err(RpcError::new(
            ERR_INVALID_PARAMS,
            format!("parameter '{key}' must be a string"),
        )),
    }
}

fn req_str<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, RpcError> {
    opt_str(args, key)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RpcError::new(
                ERR_INVALID_PARAMS,
                format!("missing required string '{key}'"),
            )
        })
}

fn opt_usize(args: &Value, key: &str) -> std::result::Result<Option<usize>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .map(Some)
            .ok_or_else(|| {
                RpcError::new(
                    ERR_INVALID_PARAMS,
                    format!("parameter '{key}' must be a non-negative integer"),
                )
            }),
        _ => Err(RpcError::new(
            ERR_INVALID_PARAMS,
            format!("parameter '{key}' must be an integer"),
        )),
    }
}

/// Executes one MCP tool call and maps the result to structured content + `isError`.
pub fn call_tool(
    ctx: &Context,
    policy: &ExecutionPolicy,
    name: &str,
    args: &Value,
) -> std::result::Result<ToolOutput, RpcError> {
    match name {
        "cqs_system_status" => {
            let include_public_ip = opt_bool(args, "include_public_ip", false)?;
            let sample_cpu = opt_bool(args, "sample_cpu", false)?;
            Ok(ToolOutput::ok(system_status_json(
                ctx,
                include_public_ip,
                sample_cpu,
            )))
        }
        "cqs_list_actions" => {
            let filter = ActionFilter {
                group: opt_str(args, "group")?.map(str::to_owned),
                query: opt_str(args, "query")?.map(str::to_owned),
                mutates: match args.get("mutates") {
                    None | Some(Value::Null) => None,
                    Some(Value::Bool(b)) => Some(*b),
                    _ => {
                        return Err(RpcError::new(
                            ERR_INVALID_PARAMS,
                            "parameter 'mutates' must be a boolean",
                        ));
                    }
                },
                root: match args.get("root") {
                    None | Some(Value::Null) => None,
                    Some(Value::Bool(b)) => Some(*b),
                    _ => {
                        return Err(RpcError::new(
                            ERR_INVALID_PARAMS,
                            "parameter 'root' must be a boolean",
                        ));
                    }
                },
                non_interactive_only: opt_bool(args, "non_interactive_only", false)?,
            };
            Ok(ToolOutput::ok(api::list_actions_json(
                ctx,
                &filter,
                Some(policy),
            )))
        }
        "cqs_describe_action" => {
            let action_id = req_str(args, "action_id")?;
            match actions::find_ref(action_id) {
                Ok(spec) => Ok(ToolOutput::ok(api::action_detail_json(ctx, spec))),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_list_action_choices" => {
            let action_id = req_str(args, "action_id")?;
            match api::action_choices_json(ctx, action_id) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_run_action" => {
            let action_id = req_str(args, "action_id")?;
            let confirm = opt_bool(args, "confirm", false)?;
            let dry_run = opt_bool(args, "dry_run", false)?;
            let spec = match actions::find(action_id) {
                Ok(s) => s,
                Err(err) => return Ok(ToolOutput::err(error_envelope(ctx, &err))),
            };
            let raw_params = args.get("parameters").unwrap_or(&Value::Null);
            let supplied = match api::parameters_from_json(&spec, raw_params) {
                Ok(p) => p,
                Err(err) => return Ok(ToolOutput::err(error_envelope(ctx, &err))),
            };
            let (payload, _) =
                api::execute_action_json(ctx, policy, action_id, &supplied, confirm, dry_run);
            Ok(ToolOutput::from_execution(payload))
        }
        "cqs_run_batch" => {
            let confirm = opt_bool(args, "confirm", false)?;
            let dry_run = opt_bool(args, "dry_run", false)?;
            let Some(arr) = args.get("jobs").and_then(Value::as_array) else {
                return Err(RpcError::new(
                    ERR_INVALID_PARAMS,
                    "parameter 'jobs' must be a non-empty array",
                ));
            };
            let mut jobs = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(obj) = item.as_object() else {
                    return Err(RpcError::new(
                        ERR_INVALID_PARAMS,
                        "each element of 'jobs' must be an object",
                    ));
                };
                let id = obj
                    .get("action_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        RpcError::new(
                            ERR_INVALID_PARAMS,
                            "each job requires non-empty 'action_id'",
                        )
                    })?;
                let spec = match actions::find(id) {
                    Ok(s) => s,
                    Err(err) => return Ok(ToolOutput::err(error_envelope(ctx, &err))),
                };
                let raw_params = obj.get("parameters").unwrap_or(&Value::Null);
                let supplied = match api::parameters_from_json(&spec, raw_params) {
                    Ok(p) => p,
                    Err(err) => return Ok(ToolOutput::err(error_envelope(ctx, &err))),
                };
                jobs.push((spec, supplied));
            }
            let (payload, _) = api::execute_batch_json(ctx, policy, &jobs, confirm, dry_run);
            Ok(ToolOutput::from_execution(payload))
        }
        "cqs_docker_snapshot" => {
            let section = opt_str(args, "section")?.unwrap_or("all");
            let include_stats = opt_bool(args, "include_stats", false)?;
            match docker_snapshot_json(ctx, section, include_stats) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_docker_inspect" => {
            let kind = req_str(args, "kind")?;
            let id = req_str(args, "id")?;
            let running = opt_bool(args, "running", false)?;
            match docker_inspect_json(ctx, kind, id, running) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_docker_logs" => {
            let id = req_str(args, "id")?;
            let tail = opt_usize(args, "tail")?.unwrap_or(100);
            match docker_logs_json(ctx, id, tail) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_docker_operate" => {
            let confirm = opt_bool(args, "confirm", false)?;
            let dry_run = opt_bool(args, "dry_run", false)?;
            let (payload, _) = execute_docker_op_json(ctx, policy, args, confirm, dry_run);
            Ok(ToolOutput::from_execution(payload))
        }
        "cqs_mihomo_snapshot" => {
            let include_rules = opt_bool(args, "include_rules", false)?;
            let include_providers = opt_bool(args, "include_providers", false)?;
            let connections_limit = opt_usize(args, "connections_limit")?;
            let rules_limit = opt_usize(args, "rules_limit")?;
            Ok(ToolOutput::ok(mihomo_snapshot_json(
                ctx,
                include_rules,
                include_providers,
                connections_limit,
                rules_limit,
            )))
        }
        "cqs_mihomo_logs" => {
            let lines = opt_usize(args, "lines")?.unwrap_or(100);
            match mihomo_logs_json(ctx, lines) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        "cqs_check_update" => {
            let version = opt_str(args, "version")?;
            match check_update_json(ctx, version) {
                Ok(val) => Ok(ToolOutput::ok(val)),
                Err(err) => Ok(ToolOutput::err(error_envelope(ctx, &err))),
            }
        }
        other => Err(RpcError::new(
            ERR_INVALID_PARAMS,
            format!("unknown tool: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tools_respects_read_only_and_allow_groups_policy() {
        let default_tools = list_tools(&ExecutionPolicy::mcp_default());
        assert_eq!(default_tools.len(), 13);
        assert!(
            default_tools
                .iter()
                .any(|t| t["name"] == "cqs_docker_operate")
        );

        let ro_tools = list_tools(&ExecutionPolicy {
            read_only: true,
            ..ExecutionPolicy::mcp_default()
        });
        assert_eq!(ro_tools.len(), 12);
        assert!(!ro_tools.iter().any(|t| t["name"] == "cqs_docker_operate"));

        let net_only = list_tools(&ExecutionPolicy {
            allow_groups: vec!["network".into()],
            ..ExecutionPolicy::mcp_default()
        });
        assert_eq!(net_only.len(), 12);
        assert!(!net_only.iter().any(|t| t["name"] == "cqs_docker_operate"));

        let docker_allowed = list_tools(&ExecutionPolicy {
            allow_groups: vec!["docker".into()],
            ..ExecutionPolicy::mcp_default()
        });
        assert_eq!(docker_allowed.len(), 13);
        assert!(
            docker_allowed
                .iter()
                .any(|t| t["name"] == "cqs_docker_operate")
        );
    }

    #[test]
    fn call_tool_covers_all_13_tools_and_parameter_validation_errors() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let policy = ExecutionPolicy::mcp_default();

        // 1. cqs_system_status
        let st = call_tool(&ctx, &policy, "cqs_system_status", &json!({})).unwrap();
        assert!(!st.is_error);
        assert_eq!(st.structured["ok"], true);
        assert_eq!(
            call_tool(
                &ctx,
                &policy,
                "cqs_system_status",
                &json!({"sample_cpu": "yes"})
            )
            .err()
            .unwrap()
            .code,
            ERR_INVALID_PARAMS
        );

        // 2. cqs_list_actions
        let list = call_tool(
            &ctx,
            &policy,
            "cqs_list_actions",
            &json!({
                "group": "network",
                "query": "bbr",
                "mutates": false,
                "root": false,
                "non_interactive_only": true
            }),
        )
        .unwrap();
        assert!(!list.is_error);
        assert_eq!(list.structured["total"], 1);
        for bad_arg in [
            json!({"mutates": "no"}),
            json!({"root": 1}),
            json!({"non_interactive_only": "true"}),
        ] {
            assert_eq!(
                call_tool(&ctx, &policy, "cqs_list_actions", &bad_arg)
                    .err()
                    .unwrap()
                    .code,
                ERR_INVALID_PARAMS
            );
        }

        // 3. cqs_describe_action
        let desc = call_tool(
            &ctx,
            &policy,
            "cqs_describe_action",
            &json!({"action_id": "bbr.enable"}),
        )
        .unwrap();
        assert!(!desc.is_error);
        assert_eq!(desc.structured["action"]["id"], "bbr.enable");
        let bad_desc = call_tool(
            &ctx,
            &policy,
            "cqs_describe_action",
            &json!({"action_id": "nope.id"}),
        )
        .unwrap();
        assert!(bad_desc.is_error);
        assert_eq!(
            call_tool(&ctx, &policy, "cqs_describe_action", &json!({}))
                .err()
                .unwrap()
                .code,
            ERR_INVALID_PARAMS
        );

        // 4. cqs_list_action_choices
        let ch = call_tool(
            &ctx,
            &policy,
            "cqs_list_action_choices",
            &json!({"action_id": "bbr.enable"}),
        )
        .unwrap();
        assert!(!ch.is_error);
        assert_eq!(ch.structured["action_id"], "bbr.enable");
        let bad_ch = call_tool(
            &ctx,
            &policy,
            "cqs_list_action_choices",
            &json!({"action_id": "nope.id"}),
        )
        .unwrap();
        assert!(bad_ch.is_error);

        // 5. cqs_run_action
        let run_ok = call_tool(
            &ctx,
            &policy,
            "cqs_run_action",
            &json!({"action_id": "terminal.preview", "parameters": {"style": "minimal"}}),
        )
        .unwrap();
        assert!(!run_ok.is_error);
        assert_eq!(run_ok.structured["status"], "completed");
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_run_action",
                &json!({"action_id": "nope.id"})
            )
            .unwrap()
            .is_error
        );
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_run_action",
                &json!({"action_id": "terminal.preview", "parameters": ["bad"]})
            )
            .unwrap()
            .is_error
        );

        // 6. cqs_run_batch
        let batch_ok = call_tool(
            &ctx,
            &policy,
            "cqs_run_batch",
            &json!({"jobs": [{"action_id": "terminal.preview"}]}),
        )
        .unwrap();
        assert!(!batch_ok.is_error);
        for bad_batch in [
            json!({"jobs": "not-array"}),
            json!({"jobs": ["not-object"]}),
            json!({"jobs": [{}]}),
        ] {
            assert_eq!(
                call_tool(&ctx, &policy, "cqs_run_batch", &bad_batch)
                    .err()
                    .unwrap()
                    .code,
                ERR_INVALID_PARAMS
            );
        }
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_run_batch",
                &json!({"jobs": [{"action_id": "nope.id"}]})
            )
            .unwrap()
            .is_error
        );
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_run_batch",
                &json!({"jobs": [{"action_id": "terminal.preview", "parameters": 123}]})
            )
            .unwrap()
            .is_error
        );

        // 7. cqs_docker_snapshot
        let d_snap = call_tool(
            &ctx,
            &policy,
            "cqs_docker_snapshot",
            &json!({"section": "containers", "include_stats": false}),
        )
        .unwrap();
        assert!(!d_snap.is_error);
        assert_eq!(d_snap.structured["section"], "containers");
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_docker_snapshot",
                &json!({"section": "invalid"})
            )
            .unwrap()
            .is_error
        );

        // 8. cqs_docker_inspect
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_docker_inspect",
                &json!({"kind": "volume", "id": "v1"})
            )
            .unwrap()
            .is_error
        );
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_docker_inspect",
                &json!({"kind": "container", "id": "-flag"})
            )
            .unwrap()
            .is_error
        );
        assert_eq!(
            call_tool(
                &ctx,
                &policy,
                "cqs_docker_inspect",
                &json!({"kind": "container"})
            )
            .err()
            .unwrap()
            .code,
            ERR_INVALID_PARAMS
        );

        // 9. cqs_docker_logs
        assert!(
            call_tool(
                &ctx,
                &policy,
                "cqs_docker_logs",
                &json!({"id": "-flag", "tail": 10})
            )
            .unwrap()
            .is_error
        );
        for bad_tail in [
            json!({"id": "web", "tail": -1}),
            json!({"id": "web", "tail": "10"}),
        ] {
            assert_eq!(
                call_tool(&ctx, &policy, "cqs_docker_logs", &bad_tail)
                    .err()
                    .unwrap()
                    .code,
                ERR_INVALID_PARAMS
            );
        }

        // 10. cqs_docker_operate
        let d_op = call_tool(
            &ctx,
            &policy,
            "cqs_docker_operate",
            &json!({
                "operation": "compose_up",
                "project": "demo",
                "config_files": ["/srv/compose.yaml"],
                "dry_run": true
            }),
        )
        .unwrap();
        assert!(!d_op.is_error);
        assert_eq!(d_op.structured["status"], "dry_run");

        // 11. cqs_mihomo_snapshot
        let m_snap = call_tool(
            &ctx,
            &policy,
            "cqs_mihomo_snapshot",
            &json!({"include_rules": false, "connections_limit": 20, "rules_limit": 50}),
        )
        .unwrap();
        assert!(!m_snap.is_error);
        assert_eq!(m_snap.structured["ok"], true);
        assert_eq!(
            call_tool(
                &ctx,
                &policy,
                "cqs_mihomo_snapshot",
                &json!({"connections_limit": -1})
            )
            .err()
            .unwrap()
            .code,
            ERR_INVALID_PARAMS
        );

        // 12. cqs_mihomo_logs (missing log yields empty list; existing log redacts secrets)
        let empty_logs =
            call_tool(&ctx, &policy, "cqs_mihomo_logs", &json!({"lines": 10})).unwrap();
        assert!(!empty_logs.is_error);
        assert_eq!(empty_logs.structured["count"], 0);
        assert_eq!(
            call_tool(&ctx, &policy, "cqs_mihomo_logs", &json!({"lines": "bad"}))
                .err()
                .unwrap()
                .code,
            ERR_INVALID_PARAMS
        );
        crate::storage::atomic_write(
            &ctx.paths.clash_mixin,
            b"secret: unit-secret-token-77\n",
            0o600,
        )
        .unwrap();
        crate::storage::atomic_write(
            &ctx.paths.clash_log,
            b"level=info msg=\"boot\"\nlevel=info secret=unit-secret-token-77\n",
            0o600,
        )
        .unwrap();
        let m_logs = call_tool(&ctx, &policy, "cqs_mihomo_logs", &json!({"lines": 5})).unwrap();
        assert!(!m_logs.is_error);
        assert_eq!(m_logs.structured["count"], 2);
        assert!(
            !serde_json::to_string(&m_logs.structured["lines"])
                .unwrap()
                .contains("unit-secret-token-77")
        );

        // 13. cqs_check_update (offline semver rejection + parameter type check)
        let bad_ver = call_tool(
            &ctx,
            &policy,
            "cqs_check_update",
            &json!({"version": "not-a-semver"}),
        )
        .unwrap();
        assert!(bad_ver.is_error);
        assert_eq!(bad_ver.structured["error"]["key"], "error.release");
        assert_eq!(
            call_tool(&ctx, &policy, "cqs_check_update", &json!({"version": 123}))
                .err()
                .unwrap()
                .code,
            ERR_INVALID_PARAMS
        );

        // Unknown tool
        assert_eq!(
            call_tool(&ctx, &policy, "cqs_unknown", &json!({}))
                .err()
                .unwrap()
                .code,
            ERR_INVALID_PARAMS
        );
    }
}
