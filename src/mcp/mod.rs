//! Native Model Context Protocol (MCP) server over newline-delimited JSON-RPC 2.0 stdio.
//!
//! Built solely on the Rust standard library (`BufRead` / `Write`) and `serde_json` to keep
//! CqShell's zero-`tokio`, single-static-binary architecture and MSRV 1.88 compatibility intact.
//!
//! Protocol invariant: `stdout` receives strictly one JSON-RPC 2.0 object per line. All CqShell
//! action execution runs through `Context::captured(...)` so logs never leak into `stdout`.

pub mod resources;
pub mod tools;

use crate::{
    api::ExecutionPolicy,
    error::{Error, Result},
    runtime::Context,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufWriter, Write},
    sync::atomic::Ordering,
};

/// MCP protocol versions supported by CqShell, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const LATEST_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];

/// Upper bound on a single JSON-RPC stdio line (4 MiB) to guard against unbounded allocation.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

pub const ERR_PARSE: i64 = -32700;
pub const ERR_INVALID_REQUEST: i64 = -32600;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_INTERNAL: i64 = -32603;

/// Structured JSON-RPC 2.0 protocol error.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }
}

/// Stateful MCP JSON-RPC 2.0 server session.
pub struct McpServer {
    ctx: Context,
    policy: ExecutionPolicy,
    initialized: bool,
    protocol_version: String,
}

impl McpServer {
    pub fn new(ctx: Context, policy: ExecutionPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            ctx,
            policy,
            initialized: false,
            protocol_version: LATEST_PROTOCOL_VERSION.to_owned(),
        })
    }

    /// Runs the blocking stdio transport loop until EOF or cancellation signal.
    pub fn serve_stdio(ctx: &Context, policy: ExecutionPolicy) -> Result<()> {
        let mut server = Self::new(ctx.clone(), policy)?;
        let stdin = io::stdin();
        let stdout = io::stdout();
        let reader = stdin.lock();
        let writer = BufWriter::new(stdout.lock());
        server.serve_io(reader, writer)
    }

    /// Runs the JSON-RPC loop over arbitrary `BufRead` / `Write` streams (used by stdio and tests).
    pub fn serve_io<R: BufRead, W: Write>(&mut self, mut reader: R, mut writer: W) -> Result<()> {
        let mut buf = Vec::new();
        loop {
            if self.ctx.cancel.load(Ordering::SeqCst) {
                break;
            }
            buf.clear();
            match read_bounded_line(&mut reader, &mut buf, MAX_MESSAGE_BYTES)? {
                LineRead::Eof => break,
                LineRead::TooLong => {
                    let resp = error_response(
                        Value::Null,
                        RpcError::new(
                            ERR_PARSE,
                            format!("JSON-RPC message exceeds {MAX_MESSAGE_BYTES} bytes limit"),
                        ),
                    );
                    write_message(&mut writer, &resp)?;
                    continue;
                }
                LineRead::Line => {}
            }
            let text = match std::str::from_utf8(&buf) {
                Ok(s) => s.trim(),
                Err(_) => {
                    let resp = error_response(
                        Value::Null,
                        RpcError::new(ERR_PARSE, "invalid UTF-8 in JSON-RPC message"),
                    );
                    write_message(&mut writer, &resp)?;
                    continue;
                }
            };
            if text.is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(text) {
                write_message(&mut writer, &reply)?;
            }
        }
        Ok(())
    }

    /// Processes a single JSON-RPC line and returns an optional response object (`None` for
    /// valid JSON-RPC notifications).
    pub fn handle_line(&mut self, raw: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(err) => {
                return Some(error_response(
                    Value::Null,
                    RpcError::new(ERR_PARSE, format!("invalid JSON: {err}")),
                ));
            }
        };
        self.handle_value(msg)
    }

    pub fn handle_value(&mut self, msg: Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(error_response(
                Value::Null,
                RpcError::new(ERR_INVALID_REQUEST, "JSON-RPC message must be an object"),
            ));
        };

        let id = obj.get("id").cloned();
        let is_notification = id.is_none();

        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                RpcError::new(ERR_INVALID_REQUEST, "missing or invalid jsonrpc='2.0'"),
            ));
        }

        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // Clients may send responses to server-initiated requests; ignore if no method.
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Value::Null),
                RpcError::new(ERR_INVALID_REQUEST, "missing string 'method'"),
            ));
        };

        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let outcome = self.dispatch(method, &params, is_notification);

        if is_notification {
            return None;
        }
        let req_id = id.unwrap_or(Value::Null);
        Some(match outcome {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": result,
            }),
            Err(err) => error_response(req_id, err),
        })
    }

    fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
        is_notification: bool,
    ) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => Ok(self.handle_initialize(params)),
            "notifications/initialized" => {
                self.initialized = true;
                Ok(Value::Null)
            }
            "notifications/cancelled" => Ok(Value::Null),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tools::list_tools(&self.policy),
            })),
            "tools/call" => self.handle_tool_call(params),
            "resources/list" => Ok(json!({
                "resources": resources::list_resources(),
            })),
            "resources/templates/list" => Ok(json!({
                "resourceTemplates": resources::list_resource_templates(),
            })),
            "resources/read" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        RpcError::new(
                            ERR_INVALID_PARAMS,
                            "resources/read requires non-empty 'uri'",
                        )
                    })?;
                resources::read_resource(&self.ctx, &self.policy, uri)
            }
            "prompts/list" => Ok(json!({
                "prompts": resources::list_prompts(),
            })),
            "prompts/get" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        RpcError::new(ERR_INVALID_PARAMS, "prompts/get requires non-empty 'name'")
                    })?;
                let args = params.get("arguments").unwrap_or(&Value::Null);
                resources::get_prompt(name, args)
            }
            other => {
                if is_notification {
                    Ok(Value::Null)
                } else {
                    Err(RpcError::new(
                        ERR_METHOD_NOT_FOUND,
                        format!("unsupported method: {other}"),
                    ))
                }
            }
        }
    }

    fn handle_initialize(&mut self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LATEST_PROTOCOL_VERSION);
        let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            LATEST_PROTOCOL_VERSION
        };
        self.protocol_version = negotiated.to_owned();

        let mode_note = if self.policy.read_only {
            "Server is running in strict READ-ONLY mode (--read-only); all mutating actions and Docker write operations are disabled."
        } else if self.policy.allow_danger {
            "Server allows read-only queries, standard mutations, and high-risk/danger mutations (--allow-danger) when explicitly confirmed with confirm=true."
        } else {
            "Server allows read-only queries and standard/reversible mutations when explicitly confirmed with confirm=true. High-risk/danger operations are blocked unless the server is started with --allow-danger."
        };

        let instructions = format!(
            "CqShell (cqs) native server toolbox MCP interface. \
             Start by inspecting state via cqs_system_status, cqs_docker_snapshot, cqs_mihomo_snapshot, \
             cqs_list_actions, cqs_describe_action, and cqs_list_action_choices. \
             Before mutating the host, preview with dry_run=true (or omit confirm) to inspect resolved parameters, risk tier, and preflight blockers, then ask the user and re-invoke with confirm=true. \
             Secrets (Mihomo controller secret, SOCKS5 password, sensitive Docker env vars) are always redacted, interactive TTY tools are blocked, and CqShell never auto-reboots the host. \
             {mode_note}"
        );

        json!({
            "protocolVersion": negotiated,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false },
                "prompts": { "listChanged": false },
            },
            "serverInfo": {
                "name": "cqs",
                "title": "CqShell",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": instructions,
        })
    }

    fn handle_tool_call(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                RpcError::new(ERR_INVALID_PARAMS, "tools/call requires non-empty 'name'")
            })?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err(RpcError::new(
                ERR_INVALID_PARAMS,
                "tools/call 'arguments' must be a JSON object",
            ));
        }

        let out = tools::call_tool(&self.ctx, &self.policy, name, &arguments)?;
        let text = serde_json::to_string_pretty(&out.structured)
            .map_err(|e| RpcError::new(ERR_INTERNAL, format!("serialization failed: {e}")))?;

        Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": out.structured,
            "isError": out.is_error,
        }))
    }
}

fn error_response(id: Value, err: RpcError) -> Value {
    let mut err_obj = serde_json::Map::new();
    err_obj.insert("code".into(), json!(err.code));
    err_obj.insert("message".into(), json!(err.message));
    if let Some(data) = err.data {
        err_obj.insert("data".into(), data);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": Value::Object(err_obj),
    })
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    let serialized = serde_json::to_string(value)?;
    writer.write_all(serialized.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

enum LineRead {
    Eof,
    Line,
    TooLong,
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<LineRead> {
    let mut too_long = false;
    let mut read_any = false;
    loop {
        let available = reader.fill_buf().map_err(Error::from)?;
        if available.is_empty() {
            if !read_any {
                return Ok(LineRead::Eof);
            }
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
        read_any = true;
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            let chunk = &available[..=pos];
            if !too_long {
                if out.len() + chunk.len() > limit {
                    too_long = true;
                    out.clear();
                } else {
                    out.extend_from_slice(chunk);
                }
            }
            let consumed = pos + 1;
            reader.consume(consumed);
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
        let len = available.len();
        if !too_long {
            if out.len() + len > limit {
                too_long = true;
                out.clear();
            } else {
                out.extend_from_slice(available);
            }
        }
        reader.consume(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_line_reader_rejects_oversized_messages_and_recovers() {
        let mut buf = Vec::new();
        let mut input = Cursor::new(b"1234567890\n{\"ok\":true}\n".to_vec());
        assert!(matches!(
            read_bounded_line(&mut input, &mut buf, 5).unwrap(),
            LineRead::TooLong
        ));
        buf.clear();
        assert!(matches!(
            read_bounded_line(&mut input, &mut buf, 64).unwrap(),
            LineRead::Line
        ));
        assert_eq!(std::str::from_utf8(&buf).unwrap().trim(), "{\"ok\":true}");
    }

    #[test]
    fn server_handles_invalid_json_notifications_and_unknown_methods_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let mut server = McpServer::new(ctx, ExecutionPolicy::mcp_default()).unwrap();
        let input = concat!(
            "{not-valid-json\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"unknown/method\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        );
        let mut out = Vec::new();
        server.serve_io(Cursor::new(input), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // Notification produced zero output lines.
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["error"]["code"], ERR_PARSE);
        assert_eq!(lines[1]["id"], 1);
        assert_eq!(lines[1]["error"]["code"], ERR_METHOD_NOT_FOUND);
        assert_eq!(lines[2]["id"], 2);
        assert_eq!(lines[2]["result"], json!({}));
    }

    #[test]
    fn server_handles_protocol_errors_version_negotiation_utf8_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());

        // Protocol version negotiation & mode-specific instructions across all 3 policy modes.
        for (policy, expected_note, req_ver, expected_ver) in [
            (
                ExecutionPolicy::mcp_default(),
                "restart",
                "2025-03-26",
                "2025-03-26",
            ),
            (
                ExecutionPolicy {
                    read_only: true,
                    ..ExecutionPolicy::mcp_default()
                },
                "READ-ONLY",
                "2024-11-05",
                "2024-11-05",
            ),
            (
                ExecutionPolicy {
                    allow_danger: true,
                    ..ExecutionPolicy::mcp_default()
                },
                "--allow-danger",
                "2099-01-01",
                LATEST_PROTOCOL_VERSION,
            ),
        ] {
            let mut srv = McpServer::new(ctx.clone(), policy).unwrap();
            let init = srv
                .handle_value(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": req_ver }
                }))
                .unwrap();
            assert_eq!(init["result"]["protocolVersion"], expected_ver);
            let instr = init["result"]["instructions"].as_str().unwrap();
            assert!(
                instr.contains(expected_note) || instr.contains("allow-danger"),
                "{instr}"
            );
        }

        let mut srv = McpServer::new(ctx.clone(), ExecutionPolicy::mcp_default()).unwrap();

        // Non-object JSON, wrong jsonrpc version, missing method
        assert_eq!(
            srv.handle_value(json!([1, 2, 3])).unwrap()["error"]["code"],
            ERR_INVALID_REQUEST
        );
        assert_eq!(
            srv.handle_value(json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}))
                .unwrap()["error"]["code"],
            ERR_INVALID_REQUEST
        );
        assert_eq!(
            srv.handle_value(json!({"jsonrpc": "2.0", "id": 1}))
                .unwrap()["error"]["code"],
            ERR_INVALID_REQUEST
        );

        // Client responses (result/error without method) and notifications yield None.
        assert!(
            srv.handle_value(json!({"jsonrpc": "2.0", "id": 9, "result": {}}))
                .is_none()
        );
        assert!(
            srv.handle_value(json!({"jsonrpc": "2.0", "method": "notifications/cancelled"}))
                .is_none()
        );
        assert!(
            srv.handle_value(json!({"jsonrpc": "2.0", "method": "notifications/custom"}))
                .is_none()
        );

        // Missing/invalid required params on tools/call, resources/read, prompts/get
        for (method, params) in [
            ("tools/call", json!({})),
            (
                "tools/call",
                json!({"name": "cqs_system_status", "arguments": "not-obj"}),
            ),
            ("resources/read", json!({})),
            ("prompts/get", json!({})),
        ] {
            let reply = srv
                .handle_value(json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": method,
                    "params": params
                }))
                .unwrap();
            assert_eq!(
                reply["error"]["code"], ERR_INVALID_PARAMS,
                "{method} {params}"
            );
        }

        // Invalid UTF-8 and blank lines in serve_io recover cleanly
        let mut raw = b"\n   \n\xff\xfe\n".to_vec();
        raw.extend_from_slice(br#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
        raw.push(b'\n');
        let mut out = Vec::new();
        srv.serve_io(Cursor::new(raw), &mut out).unwrap();
        let replies: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["error"]["code"], ERR_PARSE);
        assert_eq!(replies[1]["id"], 3);
        assert_eq!(replies[1]["result"], json!({}));

        // Cancellation flag stops serve_io immediately
        ctx.cancel.store(true, Ordering::SeqCst);
        let mut cancel_srv = McpServer::new(ctx, ExecutionPolicy::mcp_default()).unwrap();
        let mut cancel_out = Vec::new();
        cancel_srv
            .serve_io(
                Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n"),
                &mut cancel_out,
            )
            .unwrap();
        assert!(cancel_out.is_empty());
    }
}
