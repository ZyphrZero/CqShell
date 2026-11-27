//! Read-only MCP Resources, Resource Templates, and reusable Ops Prompt templates.

use super::{ERR_INTERNAL, ERR_INVALID_PARAMS, RpcError};
use crate::{
    actions,
    api::{
        self, ActionFilter, ExecutionPolicy,
        snapshots::{docker_snapshot_json, mihomo_snapshot_json, system_status_json},
    },
    runtime::Context,
};
use serde_json::{Value, json};

/// Static read-only MCP resources exposed by CqShell.
pub fn list_resources() -> Vec<Value> {
    vec![
        json!({
            "uri": "cqs://status",
            "name": "system-status",
            "title": "Host System Status",
            "description": "Live read-only host OS, kernel, CPU, memory, swap, disk, BBR/TCP, and Mihomo summary (does not contact public networks).",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "cqs://actions",
            "name": "action-catalog",
            "title": "Action Catalog",
            "description": "Complete list of CqShell actions with group, risk tier, permission requirements, and policy availability.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "cqs://policy",
            "name": "server-policy",
            "title": "MCP Access Policy & Runtime Context",
            "description": "Active MCP security boundaries (read_only, allow_danger, allow_groups, deny_actions), privileges, and safety guarantees.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "cqs://docker/snapshot",
            "name": "docker-snapshot",
            "title": "Docker Read-Only Snapshot",
            "description": "Docker engine info, containers, Compose projects, images, volumes, networks, and disk usage.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "cqs://mihomo/snapshot",
            "name": "mihomo-snapshot",
            "title": "Mihomo / Clash Redacted Snapshot",
            "description": "Redacted Mihomo runtime state, profiles, policy groups, and active connections.",
            "mimeType": "application/json"
        }),
    ]
}

/// Parameterized MCP resource URI templates.
pub fn list_resource_templates() -> Vec<Value> {
    vec![
        json!({
            "uriTemplate": "cqs://actions/{action_id}",
            "name": "action-detail",
            "title": "Action Specification & JSON Schema",
            "description": "Full parameter schema, risk tier, and defaults for a single CqShell action ID.",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "cqs://choices/{action_id}",
            "name": "action-choices",
            "title": "Live Host Choices for Action",
            "description": "Enumerated live choices on this host for an action's listed parameters (subscriptions, proxy groups/nodes, kernels).",
            "mimeType": "application/json"
        }),
    ]
}

/// Resolves one `cqs://...` resource URI into MCP `resources/read` contents.
pub fn read_resource(
    ctx: &Context,
    policy: &ExecutionPolicy,
    uri: &str,
) -> std::result::Result<Value, RpcError> {
    let payload = match uri {
        "cqs://status" => system_status_json(ctx, false, false),
        "cqs://actions" => api::list_actions_json(ctx, &ActionFilter::default(), Some(policy)),
        "cqs://policy" => policy.to_json(ctx),
        "cqs://docker/snapshot" => docker_snapshot_json(ctx, "all", false)
            .map_err(|e| RpcError::new(ERR_INTERNAL, ctx.i18n.error(&e)))?,
        "cqs://mihomo/snapshot" => mihomo_snapshot_json(ctx, false, false, None, None),
        other => {
            if let Some(id) = other.strip_prefix("cqs://actions/") {
                let spec = actions::find_ref(id)
                    .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, ctx.i18n.error(&e)))?;
                api::action_detail_json(ctx, spec)
            } else if let Some(id) = other.strip_prefix("cqs://choices/") {
                api::action_choices_json(ctx, id)
                    .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, ctx.i18n.error(&e)))?
            } else {
                return Err(RpcError::new(
                    ERR_INVALID_PARAMS,
                    format!("unknown resource URI: {other}"),
                ));
            }
        }
    };

    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| RpcError::new(ERR_INTERNAL, format!("serialization failed: {e}")))?;

    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": text,
        }]
    }))
}

/// Reusable MCP prompt templates for structured server operations.
pub fn list_prompts() -> Vec<Value> {
    vec![
        json!({
            "name": "server-audit",
            "title": "Read-Only Server Health Audit",
            "description": "Collect system, network/BBR, Docker, and Mihomo status read-only and summarize findings with safe recommendations.",
            "arguments": []
        }),
        json!({
            "name": "safe-optimize",
            "title": "Dry-Run Server Optimization Plan",
            "description": "Inspect host capabilities and produce a dry-run CqShell optimization batch plan for user approval before mutating anything.",
            "arguments": [
                {
                    "name": "goal",
                    "description": "Optional focus area (e.g. 'network', 'cleanup', 'full').",
                    "required": false
                }
            ]
        }),
        json!({
            "name": "mihomo-diagnose",
            "title": "Diagnose Mihomo Proxy & Routing",
            "description": "Inspect redacted Mihomo snapshot, recent logs, policy groups, and DNS resolution to diagnose proxy issues.",
            "arguments": [
                {
                    "name": "domain",
                    "description": "Optional domain to query via clash.dns.query.",
                    "required": false
                }
            ]
        }),
        json!({
            "name": "docker-diagnose",
            "title": "Diagnose Docker Workload",
            "description": "Inspect Docker engine health, container state, redacted environment variables, and recent bounded logs.",
            "arguments": [
                {
                    "name": "container",
                    "description": "Optional container name or ID to inspect in detail.",
                    "required": false
                }
            ]
        }),
    ]
}

/// Renders one named MCP prompt template with optional string arguments.
pub fn get_prompt(name: &str, args: &Value) -> std::result::Result<Value, RpcError> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    let (description, prompt_text) = match name {
        "server-audit" => (
            "Read-only audit of host resources, network tuning, Docker, and Mihomo.",
            "Perform a strictly read-only audit of this server using CqShell MCP tools:\n\
             1. Call `cqs_system_status` with `sample_cpu=true`.\n\
             2. Call `cqs_docker_snapshot` with `section='all'`.\n\
             3. Call `cqs_mihomo_snapshot`.\n\
             4. Summarize CPU/memory/disk pressure, BBR/TCP tuning state, unhealthy containers, and Mihomo readiness, then propose concrete next steps without mutating anything yet."
                .to_owned(),
        ),
        "safe-optimize" => {
            let goal = arg("goal");
            let scope = if goal.is_empty() {
                "general baseline optimization (system.update, system.clean, tcp.tune, bbr.enable)"
            } else {
                goal
            };
            (
                "Generate a dry-run CqShell optimization plan for user approval.",
                format!(
                    "Prepare a safe CqShell optimization plan focused on: {scope}.\n\
                     1. Read `cqs_system_status` and `cqs://policy`.\n\
                     2. Call `cqs_describe_action` for each candidate action to verify parameters and risk tiers.\n\
                     3. Call `cqs_run_batch` with `dry_run=true` and `confirm=false`.\n\
                     4. Present the dry-run plan, risk levels, and any blockers to the user and wait for explicit confirmation before executing."
                ),
            )
        }
        "mihomo-diagnose" => {
            let domain = arg("domain");
            let dns_step = if domain.is_empty() {
                String::new()
            } else {
                format!("\n4. Run read-only `cqs_run_action` with `action_id='clash.dns.query'` and `parameters={{\"dns_domain\":\"{domain}\"}}`.")
            };
            (
                "Diagnose Mihomo proxy availability, groups, and recent redacted logs.",
                format!(
                    "Diagnose Mihomo proxy health using read-only tools first:\n\
                     1. Call `cqs_mihomo_snapshot` with `include_providers=true`.\n\
                     2. Call `cqs_mihomo_logs` with `lines=120`.\n\
                     3. Check controller availability, active profile quota/expiry, and selected policy group nodes.{dns_step}\n\
                     Never request or display plaintext controller secrets or SOCKS5 passwords."
                ),
            )
        }
        "docker-diagnose" => {
            let container = arg("container");
            let detail = if container.is_empty() {
                "Inspect any exited or unhealthy containers with `cqs_docker_inspect` and `cqs_docker_logs`."
                    .to_owned()
            } else {
                format!(
                    "Inspect container `{container}` with `cqs_docker_inspect` (`kind='container'`, `id='{container}'`) and `cqs_docker_logs` (`id='{container}'`, `tail=150`)."
                )
            };
            (
                "Diagnose Docker engine and container health.",
                format!(
                    "Diagnose Docker health on this host:\n\
                     1. Call `cqs_docker_snapshot` with `section='all'` and `include_stats=true`.\n\
                     2. {detail}\n\
                     3. Summarize root causes and propose exact `cqs_docker_operate` actions with `dry_run=true` before modifying anything."
                ),
            )
        }
        other => {
            return Err(RpcError::new(
                ERR_INVALID_PARAMS,
                format!("unknown prompt: {other}"),
            ));
        }
    };

    Ok(json!({
        "description": description,
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": prompt_text,
            }
        }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_static_resources_templates_and_error_uris_resolve_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let policy = ExecutionPolicy::mcp_default();

        let statics = list_resources();
        assert_eq!(statics.len(), 5);
        for res in &statics {
            let uri = res["uri"].as_str().unwrap();
            let reply = read_resource(&ctx, &policy, uri).unwrap();
            let item = &reply["contents"][0];
            assert_eq!(item["uri"], uri);
            assert_eq!(item["mimeType"], "application/json");
            let doc: Value = serde_json::from_str(item["text"].as_str().unwrap()).unwrap();
            assert_eq!(doc["ok"], true, "{uri}");
        }

        let templates = list_resource_templates();
        assert_eq!(templates.len(), 2);
        let action_res = read_resource(&ctx, &policy, "cqs://actions/bbr.enable").unwrap();
        let action_doc: Value =
            serde_json::from_str(action_res["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(action_doc["action"]["id"], "bbr.enable");

        let choices_res = read_resource(&ctx, &policy, "cqs://choices/bbr.enable").unwrap();
        let choices_doc: Value =
            serde_json::from_str(choices_res["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(choices_doc["action_id"], "bbr.enable");

        for bad_uri in [
            "cqs://actions/does.not.exist",
            "cqs://choices/does.not.exist",
            "cqs://unknown",
        ] {
            assert_eq!(
                read_resource(&ctx, &policy, bad_uri).unwrap_err().code,
                ERR_INVALID_PARAMS,
                "{bad_uri}"
            );
        }
    }

    #[test]
    fn all_prompts_render_default_and_parameterized_variants_and_reject_unknown() {
        let prompts = list_prompts();
        assert_eq!(prompts.len(), 4);

        let audit = get_prompt("server-audit", &Value::Null).unwrap();
        assert!(
            audit["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("cqs_system_status")
        );

        let opt_default = get_prompt("safe-optimize", &Value::Null).unwrap();
        assert!(
            opt_default["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("general baseline optimization")
        );
        let opt_goal = get_prompt("safe-optimize", &json!({"goal": "network"})).unwrap();
        assert!(
            opt_goal["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("focused on: network")
        );

        let m_default = get_prompt("mihomo-diagnose", &Value::Null).unwrap();
        assert!(
            !m_default["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("clash.dns.query")
        );
        let m_domain = get_prompt("mihomo-diagnose", &json!({"domain": "example.com"})).unwrap();
        assert!(
            m_domain["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("example.com")
        );

        let d_default = get_prompt("docker-diagnose", &Value::Null).unwrap();
        assert!(
            d_default["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("exited or unhealthy")
        );
        let d_ctr = get_prompt("docker-diagnose", &json!({"container": "web-01"})).unwrap();
        assert!(
            d_ctr["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("web-01")
        );

        assert_eq!(
            get_prompt("unknown-prompt", &Value::Null).unwrap_err().code,
            ERR_INVALID_PARAMS
        );
    }
}
