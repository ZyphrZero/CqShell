//! End-to-end tests for CqShell's native AI control interfaces:
//! - Structured CLI JSON subcommands (`status`, `describe`, `choices`, `run --json`, `batch --json`)
//! - Native MCP JSON-RPC 2.0 stdio server (`cqs mcp`) with policy guardrails and secret safety

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

fn cqs(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cqs"));
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("CQS_LANG")
        .env_remove("CQS_GH_PROXY")
        .env_remove("GH_PROXY")
        .stdin(Stdio::null());
    cmd
}

#[test]
fn status_describe_and_choices_output_structured_json_without_side_effects() {
    let temp = tempfile::tempdir().unwrap();

    // `cqs status --json` across system, docker, mihomo, and all scopes.
    for section in ["system", "docker", "mihomo", "all"] {
        let out = cqs(temp.path())
            .args(["--lang", "en-US", "status", "--section", section, "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "section={section} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(parsed["schema_version"], "1");
        assert_eq!(parsed["ok"], true);
    }

    // Human-readable `cqs status` works too.
    let text_out = cqs(temp.path())
        .args(["--lang", "zh-CN", "status"])
        .output()
        .unwrap();
    assert!(text_out.status.success());
    assert!(
        String::from_utf8(text_out.stdout)
            .unwrap()
            .contains("CqShell")
    );

    // `cqs describe <id> --json` exposes risk tier, defaults, dynamic sources, and JSON Schema.
    let desc = cqs(temp.path())
        .args(["--lang", "en-US", "describe", "bbr.enable", "--json"])
        .output()
        .unwrap();
    assert!(desc.status.success());
    let parsed: Value = serde_json::from_slice(&desc.stdout).unwrap();
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["action"]["id"], "bbr.enable");
    assert_eq!(parsed["action"]["risk"], "reversible");
    assert_eq!(parsed["action"]["non_interactive_supported"], true);
    assert_eq!(parsed["action"]["input_schema"]["type"], "object");
    assert_eq!(
        parsed["action"]["input_schema"]["properties"]["qdisc"]["enum"],
        json!(["fq", "fq_codel", "fq_pie", "cake"])
    );

    // Unknown action in `--json` mode returns structured error envelope on stdout with exit code 2.
    let bad = cqs(temp.path())
        .args(["--lang", "en-US", "describe", "nope.action", "--json"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let err_json: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(err_json["ok"], false);
    assert_eq!(err_json["error"]["key"], "error.action");

    // `cqs choices <id> --json` works for both static and dynamic parameter actions.
    let choices = cqs(temp.path())
        .args(["--lang", "en-US", "choices", "bbr.enable", "--json"])
        .output()
        .unwrap();
    assert!(choices.status.success());
    let c_json: Value = serde_json::from_slice(&choices.stdout).unwrap();
    assert_eq!(c_json["ok"], true);
    assert_eq!(c_json["action_id"], "bbr.enable");
    assert_eq!(c_json["fields"][0]["field"], "qdisc");

    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn run_and_batch_json_support_dry_run_params_json_confirmation_and_execution() {
    let temp = tempfile::tempdir().unwrap();

    // Read-only action runs directly with `--json` and `--params-json` (including boolean coercion).
    let preview = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "run",
            "terminal.preview",
            "--json",
            "--params-json",
            r#"{"style":"box","color":"tokyo-night","time":true,"git":false}"#,
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let p_json: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(p_json["ok"], true);
    assert_eq!(p_json["status"], "completed");
    assert_eq!(p_json["parameters"]["time"], "on");
    assert_eq!(p_json["parameters"]["git"], "off");
    assert!(!p_json["logs"].as_array().unwrap().is_empty());

    // `--params-json -` reads JSON object from stdin cleanly.
    let mut stdin_child = cqs(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "--lang",
            "en-US",
            "run",
            "terminal.preview",
            "--json",
            "--params-json",
            "-",
        ])
        .spawn()
        .unwrap();
    stdin_child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"style":"compact","color":"nord"}"#)
        .unwrap();
    let stdin_out = stdin_child.wait_with_output().unwrap();
    assert!(stdin_out.status.success());
    let stdin_json: Value = serde_json::from_slice(&stdin_out.stdout).unwrap();
    assert_eq!(stdin_json["ok"], true);
    assert_eq!(stdin_json["parameters"]["style"], "compact");

    // `--dry-run` validates and preflights without writing anything.
    let dry = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "run",
            "bbr.enable",
            "--json",
            "--dry-run",
            "--set",
            "qdisc=fq_codel",
        ])
        .output()
        .unwrap();
    assert!(dry.status.success());
    let dry_json: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry_json["ok"], true);
    assert_eq!(dry_json["status"], "dry_run");
    assert_eq!(dry_json["parameters"]["qdisc"], "fq_codel");
    assert_eq!(dry_json["requires_confirmation"], true);

    // Interactive TTY tools and streaming clash logs are rejected cleanly up front.
    for args in [
        &["--lang", "en-US", "run", "test.ip", "--json", "--yes"][..],
        &[
            "--lang",
            "en-US",
            "run",
            "clash.logs",
            "--json",
            "--set",
            "follow=on",
        ],
    ] {
        let out = cqs(temp.path()).args(args).output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        let err_json: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(err_json["ok"], false);
        assert_eq!(err_json["error"]["key"], "error.tty");
    }

    // Mutating action without `--yes` returns structured `confirmation_required` with no writes.
    let unconfirmed = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "run",
            "github.proxy",
            "--json",
            "--params-json",
            r#"{"gh_proxy_state":true,"gh_proxy_url":"https://mirror.example.com/gh"}"#,
        ])
        .output()
        .unwrap();
    assert_eq!(unconfirmed.status.code(), Some(1));
    let u_json: Value = serde_json::from_slice(&unconfirmed.stdout).unwrap();
    assert_eq!(u_json["ok"], false);
    assert_eq!(u_json["status"], "confirmation_required");
    assert_eq!(u_json["error"]["key"], "error.confirm");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);

    // Same action with `--yes` succeeds and persists config.toml cleanly.
    let confirmed = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "--yes",
            "run",
            "github.proxy",
            "--json",
            "--params-json",
            r#"{"gh_proxy_state":true,"gh_proxy_url":"https://mirror.example.com/gh"}"#,
        ])
        .output()
        .unwrap();
    assert!(confirmed.status.success());
    let c_json: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    assert_eq!(c_json["ok"], true);
    assert_eq!(c_json["status"], "completed");
    let saved = fs::read_to_string(temp.path().join("config/cqs/config.toml")).unwrap();
    assert!(saved.contains("github_proxy_enabled = true"), "{saved}");
    assert!(
        saved.contains("github_proxy = \"https://mirror.example.com/gh/\""),
        "{saved}"
    );

    // Batch dry-run + execution in JSON mode.
    let batch_dry = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "batch",
            "terminal.preview",
            "github.proxy",
            "--json",
            "--dry-run",
            "--params-json",
            r#"{"terminal.preview":{"style":"minimal"},"github.proxy":{"gh_proxy_state":false}}"#,
        ])
        .output()
        .unwrap();
    assert!(batch_dry.status.success());
    let bd_json: Value = serde_json::from_slice(&batch_dry.stdout).unwrap();
    assert_eq!(bd_json["ok"], true);
    assert_eq!(bd_json["status"], "dry_run");
    assert_eq!(bd_json["total"], 2);
    assert_eq!(bd_json["requires_confirmation"], true);

    let batch_run = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "--yes",
            "batch",
            "terminal.preview",
            "github.proxy",
            "--json",
            "--params-json",
            r#"{"terminal.preview":{"style":"minimal"},"github.proxy":{"gh_proxy_state":false}}"#,
        ])
        .output()
        .unwrap();
    assert!(batch_run.status.success());
    let br_json: Value = serde_json::from_slice(&batch_run.stdout).unwrap();
    assert_eq!(br_json["ok"], true);
    assert_eq!(br_json["status"], "completed");
    assert_eq!(br_json["steps"][0]["id"], "terminal.preview");
    assert_eq!(br_json["steps"][0]["outcome"], "done");
    assert_eq!(br_json["steps"][1]["id"], "github.proxy");
    assert_eq!(br_json["steps"][1]["outcome"], "done");
}

struct McpClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(home: &Path, extra_args: &[&str]) -> Self {
        let mut child = cqs(home)
            .args(["--lang", "en-US", "mcp"])
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "MCP server closed stdout unexpectedly");
        let reply: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        reply
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn send_raw(&mut self, line: &str) -> Value {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let mut reply_line = String::new();
        let n = self.stdout.read_line(&mut reply_line).unwrap();
        assert!(n > 0);
        serde_json::from_str(reply_line.trim()).unwrap()
    }

    fn finish(self) {
        drop(self.stdin);
        let output = self.child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "MCP server exited with {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn mcp_stdio_end_to_end_protocol_tools_resources_prompts_and_safety_guardrails() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = McpClient::spawn(temp.path(), &[]);

    // 1. Handshake & version negotiation
    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "cqs");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("confirm=true")
    );
    client.notify("notifications/initialized", json!({}));

    let pong = client.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));

    // 2. Tools discovery
    let tools = client.request("tools/list", json!({}));
    let tool_arr = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tool_arr.len(), 13);
    assert!(tool_arr.iter().any(|t| t["name"] == "cqs_system_status"));
    assert!(tool_arr.iter().any(|t| t["name"] == "cqs_run_action"));
    assert!(tool_arr.iter().any(|t| t["name"] == "cqs_docker_operate"));

    // 3. Read-only system status & action discovery
    let status = client.request(
        "tools/call",
        json!({
            "name": "cqs_system_status",
            "arguments": { "include_public_ip": false, "sample_cpu": false }
        }),
    );
    assert_eq!(status["result"]["isError"], false);
    assert_eq!(status["result"]["structuredContent"]["ok"], true);
    assert!(status["result"]["structuredContent"]["public_network"].is_null());

    let listed = client.request(
        "tools/call",
        json!({
            "name": "cqs_list_actions",
            "arguments": { "group": "network", "query": "bbr" }
        }),
    );
    assert_eq!(listed["result"]["isError"], false);
    assert!(
        listed["result"]["structuredContent"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "bbr.enable")
    );

    // 4. Read-only action runs immediately without `confirm`
    let preview = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "terminal.preview",
                "parameters": { "style": "box", "color": "nord" }
            }
        }),
    );
    assert_eq!(preview["result"]["isError"], false);
    assert_eq!(
        preview["result"]["structuredContent"]["status"],
        "completed"
    );

    // 5. Mutating action without `confirm=true` returns `confirmation_required` without writing
    let need_confirm = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "github.proxy",
                "parameters": { "gh_proxy_state": true, "gh_proxy_url": "https://mirror.example.org/gh" }
            }
        }),
    );
    assert_eq!(need_confirm["result"]["isError"], false);
    assert_eq!(
        need_confirm["result"]["structuredContent"]["status"],
        "confirmation_required"
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);

    // 6. Mutating action with `confirm=true` succeeds and persists
    let ran = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "github.proxy",
                "parameters": { "gh_proxy_state": true, "gh_proxy_url": "https://mirror.example.org/gh" },
                "confirm": true
            }
        }),
    );
    assert_eq!(ran["result"]["isError"], false);
    assert_eq!(ran["result"]["structuredContent"]["status"], "completed");
    let saved = fs::read_to_string(temp.path().join("config/cqs/config.toml")).unwrap();
    assert!(saved.contains("https://mirror.example.org/gh/"), "{saved}");

    // 7. Danger action is blocked by default policy even with `confirm=true`
    let danger_blocked = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "ssh.port",
                "parameters": { "port": 2222 },
                "confirm": true
            }
        }),
    );
    assert_eq!(danger_blocked["result"]["isError"], true);
    assert_eq!(
        danger_blocked["result"]["structuredContent"]["status"],
        "policy_denied"
    );
    assert_eq!(
        danger_blocked["result"]["structuredContent"]["error"]["key"],
        "error.policy"
    );

    // 8. Docker mutate preview + danger guardrail
    let docker_preview = client.request(
        "tools/call",
        json!({
            "name": "cqs_docker_operate",
            "arguments": {
                "operation": "container_restart",
                "ids": ["web-01"]
            }
        }),
    );
    assert_eq!(docker_preview["result"]["isError"], false);
    assert_eq!(
        docker_preview["result"]["structuredContent"]["status"],
        "confirmation_required"
    );
    assert_eq!(
        docker_preview["result"]["structuredContent"]["operation"]["cli_preview"],
        "docker restart web-01"
    );

    let docker_danger = client.request(
        "tools/call",
        json!({
            "name": "cqs_docker_operate",
            "arguments": {
                "operation": "volume_remove",
                "names": ["pg-data"],
                "confirm": true
            }
        }),
    );
    assert_eq!(docker_danger["result"]["isError"], true);
    assert_eq!(
        docker_danger["result"]["structuredContent"]["status"],
        "policy_denied"
    );

    // 9. Resources, templates, and prompts
    let resources = client.request("resources/list", json!({}));
    assert_eq!(
        resources["result"]["resources"].as_array().unwrap().len(),
        5
    );

    let templates = client.request("resources/templates/list", json!({}));
    assert_eq!(
        templates["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let policy_res = client.request("resources/read", json!({ "uri": "cqs://policy" }));
    let policy_doc: Value = serde_json::from_str(
        policy_res["result"]["contents"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(policy_doc["policy"]["read_only"], false);
    assert_eq!(policy_doc["policy"]["allow_danger"], false);

    let action_res = client.request(
        "resources/read",
        json!({ "uri": "cqs://actions/bbr.enable" }),
    );
    let action_doc: Value = serde_json::from_str(
        action_res["result"]["contents"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(action_doc["action"]["id"], "bbr.enable");

    let prompts = client.request("prompts/list", json!({}));
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 4);

    let prompt = client.request(
        "prompts/get",
        json!({ "name": "docker-diagnose", "arguments": { "container": "nginx" } }),
    );
    assert!(
        prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("nginx")
    );

    // 10. Batch execution via MCP: read-only runs directly, mutating requires confirm,
    // and any policy-denied job aborts the whole batch before running earlier jobs.
    let ro_batch = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_batch",
            "arguments": {
                "jobs": [
                    { "action_id": "terminal.preview", "parameters": { "style": "minimal" } }
                ]
            }
        }),
    );
    assert_eq!(ro_batch["result"]["isError"], false);
    assert_eq!(
        ro_batch["result"]["structuredContent"]["status"],
        "completed"
    );
    assert_eq!(
        ro_batch["result"]["structuredContent"]["steps"][0]["outcome"],
        "done"
    );

    let mut_batch_unconfirmed = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_batch",
            "arguments": {
                "jobs": [
                    { "action_id": "terminal.preview" },
                    { "action_id": "github.proxy", "parameters": { "gh_proxy_state": false } }
                ]
            }
        }),
    );
    assert_eq!(mut_batch_unconfirmed["result"]["isError"], false);
    assert_eq!(
        mut_batch_unconfirmed["result"]["structuredContent"]["status"],
        "confirmation_required"
    );

    let danger_batch = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_batch",
            "arguments": {
                "jobs": [
                    { "action_id": "github.proxy", "parameters": { "gh_proxy_state": false } },
                    { "action_id": "ssh.port", "parameters": { "port": 2222 } }
                ],
                "confirm": true
            }
        }),
    );
    assert_eq!(danger_batch["result"]["isError"], true);
    assert_eq!(
        danger_batch["result"]["structuredContent"]["status"],
        "policy_denied"
    );
    // Earlier job in the rejected batch must not have mutated config.toml.
    let still_enabled = fs::read_to_string(temp.path().join("config/cqs/config.toml")).unwrap();
    assert!(
        still_enabled.contains("github_proxy_enabled = true"),
        "{still_enabled}"
    );

    // 11. Malformed JSON yields -32700 Parse error and server remains healthy
    let parse_err = client.send_raw("{broken");
    assert_eq!(parse_err["error"]["code"], -32700);
    let still_alive = client.request("ping", json!({}));
    assert_eq!(still_alive["result"], json!({}));

    client.finish();
}

#[test]
fn mcp_read_only_and_policy_flags_enforce_strict_boundaries() {
    let temp = tempfile::tempdir().unwrap();

    // Invalid group / action in policy flags fail immediately on startup (exit code 2).
    for bad_args in [
        &["mcp", "--allow-group", "nonexistent"][..],
        &["mcp", "--deny-action", "nope.id"],
        &["mcp", "--read-only", "--allow-danger"],
    ] {
        let out = cqs(temp.path()).args(bad_args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{bad_args:?}");
    }

    // Strict `--read-only` hides `cqs_docker_operate` and rejects mutating actions even with confirm=true.
    let mut ro = McpClient::spawn(temp.path(), &["--read-only"]);
    let init = ro.request(
        "initialize",
        json!({ "protocolVersion": "2024-11-05", "capabilities": {} }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    let tools = ro.request("tools/list", json!({}));
    let tool_arr = tools["result"]["tools"].as_array().unwrap();
    assert!(!tool_arr.iter().any(|t| t["name"] == "cqs_docker_operate"));

    let blocked = ro.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "github.proxy",
                "parameters": { "gh_proxy_state": true },
                "confirm": true
            }
        }),
    );
    assert_eq!(blocked["result"]["isError"], true);
    assert_eq!(
        blocked["result"]["structuredContent"]["status"],
        "policy_denied"
    );

    let readonly_ok = ro.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": { "action_id": "terminal.preview" }
        }),
    );
    assert_eq!(readonly_ok["result"]["isError"], false);
    assert_eq!(
        readonly_ok["result"]["structuredContent"]["status"],
        "completed"
    );
    ro.finish();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);

    // `--allow-group` and `--deny-action` enforce fine-grained scope.
    let mut scoped = McpClient::spawn(
        temp.path(),
        &[
            "--allow-group",
            "network,terminal",
            "--deny-action",
            "github.proxy",
        ],
    );
    let denied_by_id = scoped.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": { "action_id": "github.proxy", "dry_run": true }
        }),
    );
    assert_eq!(denied_by_id["result"]["isError"], true);
    assert_eq!(
        denied_by_id["result"]["structuredContent"]["status"],
        "policy_denied"
    );

    let denied_by_group = scoped.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": { "action_id": "system.info", "dry_run": true }
        }),
    );
    assert_eq!(denied_by_group["result"]["isError"], true);
    assert_eq!(
        denied_by_group["result"]["structuredContent"]["status"],
        "policy_denied"
    );

    let allowed_group = scoped.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": { "action_id": "bbr.enable", "dry_run": true }
        }),
    );
    assert_eq!(allowed_group["result"]["isError"], false);
    assert_eq!(
        allowed_group["result"]["structuredContent"]["status"],
        "dry_run"
    );
    scoped.finish();

    // `--allow-danger` authorizes high-risk dry-run/confirmation path.
    let mut danger = McpClient::spawn(temp.path(), &["--allow-danger"]);
    let ssh_plan = danger.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "ssh.port",
                "parameters": { "port": 2222 },
                "dry_run": true
            }
        }),
    );
    assert_eq!(ssh_plan["result"]["isError"], false);
    assert_eq!(ssh_plan["result"]["structuredContent"]["status"], "dry_run");
    assert_eq!(
        ssh_plan["result"]["structuredContent"]["action"]["risk"],
        "danger"
    );
    danger.finish();
}

#[test]
fn cli_text_modes_languages_json_status_flags_proxy_env_and_error_envelopes() {
    let temp = tempfile::tempdir().unwrap();

    // `cqs languages --json`
    let langs = cqs(temp.path())
        .args(["--lang", "en-US", "languages", "--json"])
        .output()
        .unwrap();
    assert!(langs.status.success());
    let lang_json: Value = serde_json::from_slice(&langs.stdout).unwrap();
    let lang_arr = lang_json.as_array().unwrap();
    assert!(lang_arr.iter().any(|pair| pair[0] == "en-US"));
    assert!(lang_arr.iter().any(|pair| pair[0] == "zh-CN"));

    // `cqs describe <id>` human-readable text output
    let desc_txt = cqs(temp.path())
        .args(["--lang", "en-US", "describe", "bbr.enable"])
        .output()
        .unwrap();
    assert!(desc_txt.status.success());
    let desc_s = String::from_utf8(desc_txt.stdout).unwrap();
    assert!(desc_s.contains("bbr.enable (network)"), "{desc_s}");
    assert!(desc_s.contains("risk=reversible"), "{desc_s}");
    assert!(desc_s.contains("param qdisc"), "{desc_s}");

    // `cqs choices <id>` human-readable text output (static choices + no-choice action)
    let ch_txt = cqs(temp.path())
        .args(["--lang", "en-US", "choices", "bbr.enable"])
        .output()
        .unwrap();
    assert!(ch_txt.status.success());
    let ch_s = String::from_utf8(ch_txt.stdout).unwrap();
    assert!(ch_s.contains("qdisc:") && ch_s.contains("* fq"), "{ch_s}");

    let ch_none = cqs(temp.path())
        .args(["--lang", "en-US", "choices", "system.info"])
        .output()
        .unwrap();
    assert!(ch_none.status.success());
    assert!(
        String::from_utf8(ch_none.stdout)
            .unwrap()
            .contains("no enumerated parameter choices")
    );

    // `cqs run --dry-run` & `cqs batch --dry-run` human-readable text output
    let run_dry_txt = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "run",
            "bbr.enable",
            "--dry-run",
            "--set",
            "qdisc=fq_codel",
        ])
        .output()
        .unwrap();
    assert!(run_dry_txt.status.success());
    let rd_s = String::from_utf8(run_dry_txt.stdout).unwrap();
    assert!(rd_s.contains("[DRY-RUN] bbr.enable"), "{rd_s}");
    assert!(rd_s.contains("qdisc=fq_codel"), "{rd_s}");

    let batch_dry_txt = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "batch",
            "terminal.preview",
            "github.proxy",
            "--dry-run",
            "--params-json",
            r#"{"github.proxy":{"gh_proxy_state":false}}"#,
        ])
        .output()
        .unwrap();
    assert!(batch_dry_txt.status.success());
    let bd_s = String::from_utf8(batch_dry_txt.stdout).unwrap();
    assert!(bd_s.contains("[DRY-RUN] batch (2 actions)"), "{bd_s}");
    assert!(bd_s.contains("1. terminal.preview"), "{bd_s}");
    assert!(bd_s.contains("2. github.proxy"), "{bd_s}");

    // `cqs status` extended flags (`--sample-cpu`, `--include-stats`, `--include-rules`, `--include-providers`)
    for args in [
        &["--lang", "en-US", "status", "--json", "--sample-cpu"][..],
        &[
            "--lang",
            "en-US",
            "status",
            "--section",
            "docker",
            "--include-stats",
            "--json",
        ],
        &[
            "--lang",
            "en-US",
            "status",
            "--section",
            "mihomo",
            "--include-rules",
            "--include-providers",
            "--json",
        ],
    ] {
        let out = cqs(temp.path()).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["ok"], true, "{args:?}");
    }

    // `proxy-env off` (bash/zsh) succeeds; `proxy-env on` fails cleanly without Mihomo installed.
    for shell in ["bash", "zsh"] {
        let off = cqs(temp.path())
            .args(["proxy-env", "off", "--shell", shell])
            .output()
            .unwrap();
        assert!(off.status.success());
        assert!(
            String::from_utf8(off.stdout)
                .unwrap()
                .contains("unset http_proxy")
        );
    }
    let on_missing = cqs(temp.path())
        .env("CQS_CLASH_HOME", temp.path().join("clash-missing"))
        .args(["--lang", "en-US", "proxy-env", "on"])
        .output()
        .unwrap();
    assert!(!on_missing.status.success());

    // Structured JSON error envelopes for malformed params-json, non-object JSON, duplicate/unlisted batch IDs.
    for (args, expected_key) in [
        (
            &[
                "--lang",
                "en-US",
                "run",
                "terminal.preview",
                "--json",
                "--params-json",
                "{bad",
            ][..],
            "error.input",
        ),
        (
            &[
                "--lang",
                "en-US",
                "run",
                "terminal.preview",
                "--json",
                "--params-json",
                "[1,2]",
            ],
            "error.input",
        ),
        (
            &[
                "--lang",
                "en-US",
                "batch",
                "terminal.preview",
                "terminal.preview",
                "--json",
            ],
            "error.input",
        ),
        (
            &[
                "--lang",
                "en-US",
                "batch",
                "terminal.preview",
                "--json",
                "--params-json",
                r#"{"bbr.enable":{"qdisc":"fq"}}"#,
            ],
            "error.input",
        ),
    ] {
        let out = cqs(temp.path()).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["ok"], false, "{args:?}");
        assert_eq!(doc["error"]["key"], expected_key, "{args:?}");
    }
}

#[test]
fn mcp_stdio_covers_all_13_tools_7_resources_4_prompts_and_error_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let clash_home = temp.path().join("clash-iso");
    fs::create_dir_all(clash_home.join("data")).unwrap();
    let mixin_path = clash_home.join("mixin.yaml");
    let log_path = clash_home.join("data/mihomo.log");
    fs::write(&mixin_path, "secret: e2e-fake-secret-999\n").unwrap();
    fs::write(
        &log_path,
        "level=info msg=\"started\"\nlevel=info secret=e2e-fake-secret-999\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mixin_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let mut child = cqs(temp.path())
        .env("CQS_CLASH_HOME", &clash_home)
        .args(["--lang", "en-US", "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut client = McpClient {
        child,
        stdin,
        stdout,
        next_id: 1,
    };

    client.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
    );
    client.notify("notifications/initialized", json!({}));
    client.notify("notifications/cancelled", json!({"requestId": 999}));

    // Remaining 8 MCP tools that were not called in the first E2E test:
    let desc = client.request(
        "tools/call",
        json!({"name": "cqs_describe_action", "arguments": {"action_id": "bbr.enable"}}),
    );
    assert_eq!(desc["result"]["isError"], false);
    assert_eq!(
        desc["result"]["structuredContent"]["action"]["id"],
        "bbr.enable"
    );

    let choices = client.request(
        "tools/call",
        json!({"name": "cqs_list_action_choices", "arguments": {"action_id": "bbr.enable"}}),
    );
    assert_eq!(choices["result"]["isError"], false);
    assert_eq!(
        choices["result"]["structuredContent"]["action_id"],
        "bbr.enable"
    );

    let d_snap = client.request(
        "tools/call",
        json!({"name": "cqs_docker_snapshot", "arguments": {"section": "all", "include_stats": false}}),
    );
    assert_eq!(d_snap["result"]["isError"], false);
    assert_eq!(d_snap["result"]["structuredContent"]["ok"], true);

    let d_ins_err = client.request(
        "tools/call",
        json!({"name": "cqs_docker_inspect", "arguments": {"kind": "volume", "id": "bad"}}),
    );
    assert_eq!(d_ins_err["result"]["isError"], true);
    assert_eq!(
        d_ins_err["result"]["structuredContent"]["error"]["key"],
        "error.input"
    );

    let d_logs_err = client.request(
        "tools/call",
        json!({"name": "cqs_docker_logs", "arguments": {"id": "-inject", "tail": 10}}),
    );
    assert_eq!(d_logs_err["result"]["isError"], true);
    assert_eq!(
        d_logs_err["result"]["structuredContent"]["error"]["key"],
        "error.input"
    );

    let m_snap = client.request(
        "tools/call",
        json!({"name": "cqs_mihomo_snapshot", "arguments": {"include_rules": false}}),
    );
    assert_eq!(m_snap["result"]["isError"], false);
    assert_eq!(m_snap["result"]["structuredContent"]["ok"], true);

    let m_logs = client.request(
        "tools/call",
        json!({"name": "cqs_mihomo_logs", "arguments": {"lines": 10}}),
    );
    assert_eq!(m_logs["result"]["isError"], false);
    assert_eq!(m_logs["result"]["structuredContent"]["count"], 2);
    assert!(
        !serde_json::to_string(&m_logs["result"]["structuredContent"]["lines"])
            .unwrap()
            .contains("e2e-fake-secret-999")
    );

    let upd_err = client.request(
        "tools/call",
        json!({"name": "cqs_check_update", "arguments": {"version": "not-a-semver"}}),
    );
    assert_eq!(upd_err["result"]["isError"], true);
    assert_eq!(
        upd_err["result"]["structuredContent"]["error"]["key"],
        "error.release"
    );

    // All 5 static resources + both resource templates + unknown URI rejection
    for uri in [
        "cqs://status",
        "cqs://actions",
        "cqs://policy",
        "cqs://docker/snapshot",
        "cqs://mihomo/snapshot",
        "cqs://actions/bbr.enable",
        "cqs://choices/bbr.enable",
    ] {
        let res = client.request("resources/read", json!({"uri": uri}));
        let text = res["result"]["contents"][0]["text"].as_str().unwrap();
        let doc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(doc["ok"], true, "{uri}");
    }
    let bad_uri = client.request("resources/read", json!({"uri": "cqs://does-not-exist"}));
    assert_eq!(bad_uri["error"]["code"], -32602);

    // All 4 prompts + unknown prompt rejection
    for (name, args) in [
        ("server-audit", json!({})),
        ("safe-optimize", json!({"goal": "network"})),
        ("mihomo-diagnose", json!({"domain": "example.com"})),
        ("docker-diagnose", json!({})),
    ] {
        let pr = client.request("prompts/get", json!({"name": name, "arguments": args}));
        assert!(
            !pr["result"]["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
    let bad_prompt = client.request("prompts/get", json!({"name": "nonexistent"}));
    assert_eq!(bad_prompt["error"]["code"], -32602);

    // Unknown tool & invalid JSON-RPC request recovery
    let bad_tool = client.request(
        "tools/call",
        json!({"name": "cqs_unknown", "arguments": {}}),
    );
    assert_eq!(bad_tool["error"]["code"], -32602);
    let bad_req = client.send_raw("[1,2,3]");
    assert_eq!(bad_req["error"]["code"], -32600);
    assert_eq!(client.request("ping", json!({}))["result"], json!({}));

    // Anti-self-escalation: MCP control-plane mutations are blocked over MCP even with confirm=true
    let self_mut = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": {
                "action_id": "ai.mcp.configure",
                "parameters": { "mcp_listen": "0.0.0.0", "mcp_policy": "allow_danger" },
                "confirm": true
            }
        }),
    );
    assert_eq!(self_mut["result"]["isError"], true);
    assert_eq!(
        self_mut["result"]["structuredContent"]["status"],
        "policy_denied"
    );
    let mcp_stat = client.request(
        "tools/call",
        json!({
            "name": "cqs_run_action",
            "arguments": { "action_id": "ai.mcp.status" }
        }),
    );
    assert_eq!(mcp_stat["result"]["isError"], false);
    assert_eq!(
        mcp_stat["result"]["structuredContent"]["status"],
        "completed"
    );

    client.finish();
}

#[test]
fn mcp_cli_token_config_install_uninstall_and_streamable_http_server_work_end_to_end() {
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let run_cqs = |home: &Path, args: &[&str]| cqs(home).args(args).output().unwrap();
    let parse_json = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).unwrap() };

    // 1. Token status -> absent; init -> creates 0600 token without echoing plaintext
    let st_empty = run_cqs(temp.path(), &["--json", "mcp", "token", "status"]);
    assert!(st_empty.status.success());
    let st_json = parse_json(&st_empty.stdout);
    assert_eq!(st_json["exists"], false);

    let init_redacted = run_cqs(temp.path(), &["--json", "mcp", "token", "init"]);
    assert!(init_redacted.status.success());
    let init_json = parse_json(&init_redacted.stdout);
    assert_eq!(init_json["created"], true);
    assert_eq!(init_json["revealed"], false);
    assert!(init_json.get("token").is_none());

    // 2. `cqs mcp config` redacts by default and outputs real token only with `--reveal`
    let cfg_redacted = run_cqs(
        temp.path(),
        &[
            "--json",
            "mcp",
            "config",
            "--transport",
            "http",
            "--client",
            "json",
            "--public-url",
            "https://mcp.example.com/mcp",
        ],
    );
    assert!(cfg_redacted.status.success());
    let cfg_red_json = parse_json(&cfg_redacted.stdout);
    assert_eq!(cfg_red_json["revealed"], false);
    assert!(
        cfg_red_json["snippet"]
            .as_str()
            .unwrap()
            .contains("<MCP_TOKEN>")
    );

    let cfg_revealed = run_cqs(
        temp.path(),
        &[
            "--json",
            "mcp",
            "config",
            "--transport",
            "http",
            "--client",
            "json",
            "--reveal",
        ],
    );
    assert!(cfg_revealed.status.success());
    let cfg_rev_json = parse_json(&cfg_revealed.stdout);
    assert_eq!(cfg_rev_json["revealed"], true);
    let snippet = cfg_rev_json["snippet"].as_str().unwrap();
    assert!(snippet.contains("Bearer cqs_mcp_"), "{snippet}");

    // Extract live Bearer token via `cqs mcp token init --reveal`
    let tok_out = run_cqs(temp.path(), &["mcp", "token", "init", "--reveal"]);
    assert!(tok_out.status.success());
    let token = String::from_utf8(tok_out.stdout).unwrap().trim().to_owned();
    assert!(token.starts_with("cqs_mcp_"));

    // 3. Local client install & uninstall across Claude, Codex, Cursor, and custom --file
    let custom_file = temp.path().join("custom_mcp.json");
    for (client_name, extra) in [
        ("claude", vec![]),
        ("codex", vec![]),
        ("cursor", vec![]),
        ("file", vec!["--file", custom_file.to_str().unwrap()]),
    ] {
        let mut args = vec![
            "--json",
            "mcp",
            "install",
            "--client",
            client_name,
            "--transport",
            "local_stdio",
            "--policy",
            "read_only",
        ];
        args.extend(extra.iter().copied());
        let inst = run_cqs(temp.path(), &args);
        assert!(
            inst.status.success(),
            "install {client_name}: {}",
            String::from_utf8_lossy(&inst.stderr)
        );
        let inst_json = parse_json(&inst.stdout);
        assert_eq!(inst_json["ok"], true);
        assert_eq!(inst_json["client"], client_name);

        let mut uargs = vec!["--json", "mcp", "uninstall", "--client", client_name];
        uargs.extend(extra.iter().copied());
        let uninst = run_cqs(temp.path(), &uargs);
        assert!(uninst.status.success());
        let uninst_json = parse_json(&uninst.stdout);
        assert_eq!(uninst_json["removed"], true);
    }

    // 4. Non-loopback HTTP with `--no-auth` must be rejected with exit code 2
    let bad_serve = run_cqs(
        temp.path(),
        &["mcp", "serve", "--listen", "0.0.0.0", "--no-auth"],
    );
    assert_eq!(bad_serve.status.code(), Some(2));

    // 5. Live `cqs mcp serve` child process over loopback TCP: 401 -> 403 -> 200 initialize ->
    // 200 tools/list -> anti-self-escalation denial even under --allow-danger
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut child = cqs(temp.path())
        .args([
            "--lang",
            "en-US",
            "mcp",
            "serve",
            "--listen",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--allow-danger",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let addr = format!("127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(&addr).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    assert!(ready, "cqs mcp serve did not start listening on {addr}");

    let exchange = |raw: &str| -> (u16, String) {
        let mut s = TcpStream::connect(&addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        let code: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (code, body.to_owned())
    };

    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let (s401, _) = exchange(&format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{ping}",
        ping.len()
    ));
    assert_eq!(s401, 401);

    let (s403, _) = exchange(&format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nOrigin: https://evil.example.org\r\nContent-Length: {}\r\n\r\n{ping}",
        ping.len()
    ));
    assert_eq!(s403, 403);

    let self_mut_body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"cqs_run_action","arguments":{"action_id":"ai.mcp.token.rotate","confirm":true}}}"#;
    let (s200, body) = exchange(&format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{self_mut_body}",
        self_mut_body.len()
    ));
    assert_eq!(s200, 200);
    let parsed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"]["isError"], true);
    assert_eq!(
        parsed["result"]["structuredContent"]["status"],
        "policy_denied"
    );

    let _ = child.kill();
    let _ = child.wait();
}
