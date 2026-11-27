//! Native zero-`tokio` MCP Streamable HTTP (`2025-06-18` / `2025-03-26`) server over
//! `std::net::TcpListener` and bounded worker threads.
//!
//! Security invariants:
//! - Non-loopback bindings strictly require Bearer token authentication (enforced at startup and
//!   on every request, failing closed if the token file disappears).
//! - Constant-time SHA-256 XOR comparison for `Authorization: Bearer <token>`.
//! - Strict `Origin` validation against browser DNS rebinding attacks.
//! - Bounded headers (16 KiB), request body (`MAX_MESSAGE_BYTES` = 4 MiB), concurrent workers
//!   (32), and in-memory sessions (64, 1h idle TTL).
//! - `GET <endpoint>` returns `405 Method Not Allowed` (no hanging server-initiated SSE streams).

use super::{
    ERR_INVALID_REQUEST, ERR_PARSE, MAX_MESSAGE_BYTES, McpServer, RpcError,
    config::{
        McpHttpConfig, constant_time_token_eq, generate_session_id, is_loopback_ip, read_token,
        token_path,
    },
    error_response,
};
use crate::{
    api::ExecutionPolicy,
    error::{Error, Result},
    runtime::Context,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_CONCURRENT_CONNS: usize = 32;
const MAX_SESSIONS: usize = 64;
const SESSION_TTL: Duration = Duration::from_secs(3600);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
struct SessionEntry {
    initialized: bool,
    protocol_version: String,
    last_used: Instant,
}

type FileFingerprint = (u64, Option<SystemTime>, u64);

struct TokenCache {
    fingerprint: Option<FileFingerprint>,
    token: Option<String>,
}

struct SharedState {
    ctx: Context,
    config: McpHttpConfig,
    policy: ExecutionPolicy,
    token_cache: Mutex<TokenCache>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
    active_conns: AtomicUsize,
}

impl SharedState {
    fn new(ctx: Context, config: McpHttpConfig, policy: ExecutionPolicy) -> Result<Self> {
        let ip = config.validate()?;
        policy.validate()?;
        if !is_loopback_ip(&ip) && !config.auth_enabled {
            return Err(Error::new(
                "error.input",
                "non-loopback MCP HTTP listener requires Bearer token authentication",
            ));
        }
        let initial_token = if config.auth_enabled {
            let tok = read_token(&ctx)?.ok_or_else(|| {
                Error::new(
                    "error.config",
                    "MCP Bearer token is missing; run 'cqs mcp token init' first",
                )
            })?;
            Some(tok)
        } else {
            None
        };
        let initial_fp = stat_fingerprint(&token_path(&ctx));
        Ok(Self {
            ctx,
            config,
            policy,
            token_cache: Mutex::new(TokenCache {
                fingerprint: initial_fp,
                token: initial_token,
            }),
            sessions: Mutex::new(HashMap::new()),
            active_conns: AtomicUsize::new(0),
        })
    }

    /// Checks Bearer authentication, hot-reloading the `0600` token file whenever its stat
    /// fingerprint changes (so `cqs mcp token rotate` takes effect immediately without restart).
    /// Fails closed if the token file becomes missing or unreadable.
    fn verify_bearer(&self, auth_header: Option<&str>) -> bool {
        if !self.config.auth_enabled {
            return true;
        }
        let Some(raw) = auth_header else {
            return false;
        };
        let Some((scheme, provided)) = raw.trim().split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("bearer") {
            return false;
        }
        let provided = provided.trim();
        if provided.is_empty() {
            return false;
        }

        let current_fp = stat_fingerprint(&token_path(&self.ctx));
        let Ok(mut guard) = self.token_cache.lock() else {
            return false;
        };
        #[cfg(unix)]
        let need_reload = guard.fingerprint != current_fp;
        #[cfg(not(unix))]
        let need_reload = true;
        if need_reload {
            guard.fingerprint = current_fp;
            guard.token = read_token(&self.ctx).ok().flatten();
        }
        let Some(expected) = guard.token.as_deref() else {
            return false;
        };
        constant_time_token_eq(expected, provided)
    }

    fn origin_allowed(&self, origin: Option<&str>) -> bool {
        let Some(raw) = origin.map(str::trim).filter(|s| !s.is_empty()) else {
            // Non-browser MCP clients (Claude Code, Codex, Cursor, curl) do not send Origin.
            return true;
        };
        if self.config.allowed_origins.iter().any(|o| o == raw) {
            return true;
        }
        let Ok(parsed) = url::Url::parse(raw) else {
            return false;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return false;
        }
        match parsed.host_str() {
            Some("localhost") => true,
            Some(host) => host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| is_loopback_ip(&ip)),
            None => false,
        }
    }

    fn create_session(&self, protocol_version: String) -> Result<String> {
        let id = generate_session_id()?;
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| Error::new("error.io", "session lock poisoned"))?;
        prune_sessions(&mut sessions, now);
        sessions.insert(
            id.clone(),
            SessionEntry {
                initialized: false,
                protocol_version,
                last_used: now,
            },
        );
        Ok(id)
    }

    fn get_session(&self, id: &str) -> Option<SessionEntry> {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().ok()?;
        prune_sessions(&mut sessions, now);
        let entry = sessions.get_mut(id)?;
        entry.last_used = now;
        Some(entry.clone())
    }

    fn update_session(&self, id: &str, initialized: bool, protocol_version: String) {
        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(entry) = sessions.get_mut(id)
        {
            entry.initialized = initialized;
            entry.protocol_version = protocol_version;
            entry.last_used = Instant::now();
        }
    }

    fn remove_session(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .ok()
            .and_then(|mut s| s.remove(id))
            .is_some()
    }
}

fn stat_fingerprint(path: &std::path::Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    Some((meta.len(), meta.modified().ok(), ino))
}

fn prune_sessions(sessions: &mut HashMap<String, SessionEntry>, now: Instant) {
    sessions.retain(|_, v| now.saturating_duration_since(v.last_used) < SESSION_TTL);
    while sessions.len() >= MAX_SESSIONS {
        let oldest = sessions
            .iter()
            .min_by_key(|(_, v)| v.last_used)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                sessions.remove(&k);
            }
            None => break,
        }
    }
}

/// Running handle for a bound Streamable HTTP MCP listener (also used directly in tests).
pub struct BoundHttpServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    state: Arc<SharedState>,
}

impl BoundHttpServer {
    pub fn bind(ctx: &Context, config: McpHttpConfig, policy: ExecutionPolicy) -> Result<Self> {
        let ip = config.validate()?;
        let state = Arc::new(SharedState::new(ctx.clone(), config, policy)?);
        let listener = TcpListener::bind(SocketAddr::new(ip, state.config.port))?;
        let local_addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            local_addr,
            state,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Runs the accept loop until `ctx.cancel` is raised.
    pub fn serve(self) -> Result<()> {
        let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();
        loop {
            if self.state.ctx.cancel.load(Ordering::SeqCst) {
                break;
            }
            // Reap completed worker handles periodically so memory stays bounded.
            handles.retain(|h| !h.is_finished());
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    let prev = self.state.active_conns.fetch_add(1, Ordering::AcqRel);
                    if prev >= MAX_CONCURRENT_CONNS {
                        self.state.active_conns.fetch_sub(1, Ordering::AcqRel);
                        let mut s = stream;
                        let _ = write_http_response(
                            &mut s,
                            503,
                            "Service Unavailable",
                            &[],
                            Some(
                                &json!({ "error": "server busy: too many concurrent connections" })
                                    .to_string(),
                            ),
                        );
                        continue;
                    }
                    let state = Arc::clone(&self.state);
                    let handle = thread::spawn(move || {
                        let _guard = ActiveConnGuard(Arc::clone(&state));
                        handle_connection(stream, &state);
                    });
                    handles.push(handle);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        for handle in handles {
            let _ = handle.join();
        }
        Ok(())
    }
}

struct ActiveConnGuard(Arc<SharedState>);
impl Drop for ActiveConnGuard {
    fn drop(&mut self) {
        self.0.active_conns.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Convenience entrypoint for `cqs mcp serve`.
pub fn serve_http(ctx: &Context, config: McpHttpConfig, policy: ExecutionPolicy) -> Result<()> {
    let bound = BoundHttpServer::bind(ctx, config.clone(), policy)?;
    let endpoint = format!("http://{}{}", bound.local_addr(), config.endpoint_path);
    eprintln!(
        "cqs MCP Streamable HTTP listening on {endpoint} (auth={}, policy={})",
        if config.auth_enabled { "bearer" } else { "off" },
        config.policy_mode.as_str(),
    );
    if !config.is_loopback() && config.public_url.is_empty() {
        eprintln!(
            "WARNING: non-loopback HTTP listener sends Bearer tokens in cleartext; place behind an HTTPS reverse proxy or private WireGuard/Tailscale tunnel."
        );
    }
    bound.serve()
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

enum ParseError {
    Empty,
    BadRequest(&'static str),
    PayloadTooLarge,
}

fn read_http_request(stream: &TcpStream) -> std::result::Result<HttpRequest, ParseError> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    let n = read_header_line(&mut reader, &mut request_line, 2048)?;
    if n == 0 {
        return Err(ParseError::Empty);
    }
    let trimmed = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.split_whitespace();
    let method = parts
        .next()
        .ok_or(ParseError::BadRequest("missing HTTP method"))?
        .to_owned();
    let target = parts
        .next()
        .ok_or(ParseError::BadRequest("missing request target"))?;
    let version = parts
        .next()
        .ok_or(ParseError::BadRequest("missing HTTP version"))?;
    if !version.starts_with("HTTP/1.") {
        return Err(ParseError::BadRequest("unsupported HTTP version"));
    }
    let path = target.split('?').next().unwrap_or(target).to_owned();

    let mut headers = HashMap::new();
    let mut total_header_bytes = n;
    loop {
        let mut line = String::new();
        let read = read_header_line(&mut reader, &mut line, 4096)?;
        if read == 0 {
            return Err(ParseError::BadRequest("truncated HTTP headers"));
        }
        total_header_bytes += read;
        if total_header_bytes > MAX_HEADER_BYTES {
            return Err(ParseError::BadRequest("HTTP headers exceed 16 KiB limit"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            return Err(ParseError::BadRequest("malformed HTTP header"));
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim().to_owned();
        headers.insert(key, val);
    }

    let is_chunked = headers
        .get("transfer-encoding")
        .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"));

    let body = if is_chunked {
        read_chunked_body(&mut reader, MAX_MESSAGE_BYTES)?
    } else if let Some(cl_raw) = headers.get("content-length") {
        let cl = cl_raw
            .parse::<usize>()
            .map_err(|_| ParseError::BadRequest("invalid Content-Length"))?;
        if cl > MAX_MESSAGE_BYTES {
            return Err(ParseError::PayloadTooLarge);
        }
        let mut buf = vec![0u8; cl];
        reader
            .read_exact(&mut buf)
            .map_err(|_| ParseError::BadRequest("truncated HTTP request body"))?;
        buf
    } else {
        Vec::new()
    };

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn read_header_line<R: BufRead>(
    reader: &mut R,
    out: &mut String,
    limit: usize,
) -> std::result::Result<usize, ParseError> {
    let mut raw = Vec::new();
    loop {
        let avail = reader
            .fill_buf()
            .map_err(|_| ParseError::BadRequest("I/O error reading headers"))?;
        if avail.is_empty() {
            break;
        }
        if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
            if raw.len() + pos + 1 > limit {
                return Err(ParseError::BadRequest("header line too long"));
            }
            raw.extend_from_slice(&avail[..=pos]);
            reader.consume(pos + 1);
            break;
        }
        if raw.len() + avail.len() > limit {
            return Err(ParseError::BadRequest("header line too long"));
        }
        raw.extend_from_slice(avail);
        let len = avail.len();
        reader.consume(len);
    }
    let len = raw.len();
    let s = String::from_utf8(raw).map_err(|_| ParseError::BadRequest("non-UTF-8 HTTP header"))?;
    out.push_str(&s);
    Ok(len)
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> std::result::Result<Vec<u8>, ParseError> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        let n = read_header_line(reader, &mut size_line, 128)?;
        if n == 0 {
            return Err(ParseError::BadRequest("truncated chunked body"));
        }
        let size_hex = size_line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| ParseError::BadRequest("invalid chunk size"))?;
        if size == 0 {
            // Drain trailing CRLF / optional trailers.
            let mut trailer = String::new();
            let _ = read_header_line(reader, &mut trailer, 256);
            break;
        }
        if out.len().saturating_add(size) > limit {
            return Err(ParseError::PayloadTooLarge);
        }
        let start = out.len();
        out.resize(start + size, 0);
        reader
            .read_exact(&mut out[start..])
            .map_err(|_| ParseError::BadRequest("truncated chunk payload"))?;
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|_| ParseError::BadRequest("missing chunk terminator"))?;
        if &crlf != b"\r\n" {
            return Err(ParseError::BadRequest("invalid chunk terminator"));
        }
    }
    Ok(out)
}

fn handle_connection(mut stream: TcpStream, state: &SharedState) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let req = match read_http_request(&stream) {
        Ok(req) => req,
        Err(ParseError::Empty) => return,
        Err(ParseError::PayloadTooLarge) => {
            let body = serde_json::to_string(&error_response(
                Value::Null,
                RpcError::new(
                    ERR_PARSE,
                    format!("JSON-RPC body exceeds {MAX_MESSAGE_BYTES} bytes limit"),
                ),
            ))
            .unwrap_or_default();
            let _ = write_http_response(&mut stream, 413, "Payload Too Large", &[], Some(&body));
            return;
        }
        Err(ParseError::BadRequest(msg)) => {
            let body = json!({ "error": msg }).to_string();
            let _ = write_http_response(&mut stream, 400, "Bad Request", &[], Some(&body));
            return;
        }
    };

    if req.path != state.config.endpoint_path {
        let body = json!({ "error": "not found" }).to_string();
        let _ = write_http_response(&mut stream, 404, "Not Found", &[], Some(&body));
        return;
    }

    if !state.origin_allowed(req.headers.get("origin").map(String::as_str)) {
        let body = json!({ "error": "forbidden origin" }).to_string();
        let _ = write_http_response(&mut stream, 403, "Forbidden", &[], Some(&body));
        return;
    }

    if !state.verify_bearer(req.headers.get("authorization").map(String::as_str)) {
        let body = json!({ "error": "unauthorized: valid Bearer token required" }).to_string();
        let _ = write_http_response(
            &mut stream,
            401,
            "Unauthorized",
            &[("WWW-Authenticate", "Bearer realm=\"cqs-mcp\"")],
            Some(&body),
        );
        return;
    }

    match req.method.as_str() {
        "POST" => handle_post(&mut stream, &req, state),
        "DELETE" => handle_delete(&mut stream, &req, state),
        "GET" => {
            let body = json!({ "error": "server does not provide unsolicited GET SSE streams" })
                .to_string();
            let _ = write_http_response(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "POST, DELETE")],
                Some(&body),
            );
        }
        _ => {
            let body = json!({ "error": "method not allowed" }).to_string();
            let _ = write_http_response(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "POST, DELETE")],
                Some(&body),
            );
        }
    }
}

fn handle_delete(stream: &mut TcpStream, req: &HttpRequest, state: &SharedState) {
    let Some(sid) = req
        .headers
        .get("mcp-session-id")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    else {
        let body = json!({ "error": "missing MCP-Session-Id header" }).to_string();
        let _ = write_http_response(stream, 400, "Bad Request", &[], Some(&body));
        return;
    };
    if state.remove_session(sid) {
        let _ = write_http_response(stream, 204, "No Content", &[], None);
    } else {
        let body = json!({ "error": "unknown or expired session" }).to_string();
        let _ = write_http_response(stream, 404, "Not Found", &[], Some(&body));
    }
}

fn handle_post(stream: &mut TcpStream, req: &HttpRequest, state: &SharedState) {
    let raw_text = match std::str::from_utf8(&req.body) {
        Ok(s) => s.trim(),
        Err(_) => {
            let body = serde_json::to_string(&error_response(
                Value::Null,
                RpcError::new(ERR_PARSE, "invalid UTF-8 in JSON-RPC request"),
            ))
            .unwrap_or_default();
            let _ = write_http_response(stream, 400, "Bad Request", &[], Some(&body));
            return;
        }
    };
    if raw_text.is_empty() {
        let body = serde_json::to_string(&error_response(
            Value::Null,
            RpcError::new(ERR_INVALID_REQUEST, "empty JSON-RPC body"),
        ))
        .unwrap_or_default();
        let _ = write_http_response(stream, 400, "Bad Request", &[], Some(&body));
        return;
    }
    let parsed: Value = match serde_json::from_str(raw_text) {
        Ok(v) => v,
        Err(err) => {
            let body = serde_json::to_string(&error_response(
                Value::Null,
                RpcError::new(ERR_PARSE, format!("invalid JSON: {err}")),
            ))
            .unwrap_or_default();
            let _ = write_http_response(stream, 400, "Bad Request", &[], Some(&body));
            return;
        }
    };

    let incoming_sid = req
        .headers
        .get("mcp-session-id")
        .map(String::as_str)
        .filter(|s| !s.is_empty());

    let is_initialize = match &parsed {
        Value::Object(obj) => obj.get("method").and_then(Value::as_str) == Some("initialize"),
        Value::Array(items) => items.iter().any(|item| {
            item.as_object()
                .and_then(|o| o.get("method"))
                .and_then(Value::as_str)
                == Some("initialize")
        }),
        _ => false,
    };

    let mut server = match McpServer::new(state.ctx.clone(), state.policy.clone()) {
        Ok(s) => s,
        Err(err) => {
            let body = json!({ "error": err.to_string() }).to_string();
            let _ = write_http_response(stream, 500, "Internal Server Error", &[], Some(&body));
            return;
        }
    };

    let mut response_sid: Option<String> = None;
    if let Some(sid) = incoming_sid {
        let Some(entry) = state.get_session(sid) else {
            let body = json!({ "error": "unknown or expired MCP session" }).to_string();
            let _ = write_http_response(stream, 404, "Not Found", &[], Some(&body));
            return;
        };
        server.restore_session(entry.initialized, entry.protocol_version);
        response_sid = Some(sid.to_owned());
    }

    let reply_value: Option<Value> = match parsed {
        Value::Array(items) => {
            if items.is_empty() {
                Some(error_response(
                    Value::Null,
                    RpcError::new(ERR_INVALID_REQUEST, "empty JSON-RPC batch"),
                ))
            } else {
                let replies: Vec<Value> = items
                    .into_iter()
                    .filter_map(|item| server.handle_value(item))
                    .collect();
                if replies.is_empty() {
                    None
                } else {
                    Some(Value::Array(replies))
                }
            }
        }
        single => server.handle_value(single),
    };

    if is_initialize && response_sid.is_none() {
        match state.create_session(server.protocol_version().to_owned()) {
            Ok(sid) => response_sid = Some(sid),
            Err(err) => {
                let body = json!({ "error": err.to_string() }).to_string();
                let _ = write_http_response(stream, 500, "Internal Server Error", &[], Some(&body));
                return;
            }
        }
    } else if let Some(ref sid) = response_sid {
        state.update_session(
            sid,
            server.is_initialized(),
            server.protocol_version().to_owned(),
        );
    }

    let mut extra_headers: Vec<(&str, &str)> = Vec::new();
    if let Some(ref sid) = response_sid {
        extra_headers.push(("MCP-Session-Id", sid.as_str()));
    }

    match reply_value {
        None => {
            let _ = write_http_response(stream, 202, "Accepted", &extra_headers, None);
        }
        Some(value) => {
            let body = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
            let _ = write_http_response(stream, 200, "OK", &extra_headers, Some(&body));
        }
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    extra_headers: &[(&str, &str)],
    json_body: Option<&str>,
) -> io::Result<()> {
    let body_bytes = json_body.map(str::as_bytes).unwrap_or(&[]);
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n"
    );
    if json_body.is_some() {
        header.push_str("Content-Type: application/json; charset=utf-8\r\n");
    }
    header.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    for (k, v) in extra_headers {
        header.push_str(&format!("{k}: {v}\r\n"));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes())?;
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::config::{PolicyMode, ensure_token, rotate_token};

    fn http_exchange(
        addr: SocketAddr,
        raw_request: &str,
    ) -> (u16, HashMap<String, String>, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(raw_request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        let (head, body) = resp.split_once("\r\n\r\n").unwrap();
        let mut lines = head.lines();
        let status_line = lines.next().unwrap();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        (status, headers, body.to_owned())
    }

    fn post_req(
        path: &str,
        token: Option<&str>,
        session_id: Option<&str>,
        origin: Option<&str>,
        body: &str,
    ) -> String {
        let mut req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(t) = token {
            req.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if let Some(sid) = session_id {
            req.push_str(&format!("MCP-Session-Id: {sid}\r\n"));
        }
        if let Some(o) = origin {
            req.push_str(&format!("Origin: {o}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        req
    }

    #[test]
    fn streamable_http_auth_origin_sessions_chunked_rotation_and_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let (token, _) = ensure_token(&ctx).unwrap();

        let cfg = McpHttpConfig {
            listen: "127.0.0.1".into(),
            port: 0, // Bind ephemeral port directly via TcpListener in test helper below
            ..McpHttpConfig::default()
        };
        // Pick a free ephemeral port cleanly.
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let cfg = McpHttpConfig { port, ..cfg };
        assert_eq!(cfg.policy_mode, PolicyMode::ReadOnly);

        let policy = cfg.execution_policy();
        let server = BoundHttpServer::bind(&ctx, cfg, policy).unwrap();
        let addr = server.local_addr();
        let cancel = ctx.cancel.clone();
        let handle = thread::spawn(move || server.serve().unwrap());

        // 1. Missing Bearer token -> 401
        let (status, headers, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                None,
                None,
                None,
                r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            ),
        );
        assert_eq!(status, 401);
        assert!(headers.contains_key("www-authenticate"));

        // 2. Wrong Bearer token -> 401
        let (status, _, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some("wrong-token"),
                None,
                None,
                r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            ),
        );
        assert_eq!(status, 401);

        // 3. Untrusted cross-site Origin -> 403 (DNS rebinding protection)
        let (status, _, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&token),
                None,
                Some("https://evil.example.org"),
                r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            ),
        );
        assert_eq!(status, 403);

        // 4. Unknown path -> 404; GET /mcp -> 405
        let (status, _, _) = http_exchange(
            addr,
            &post_req(
                "/other",
                Some(&token),
                None,
                None,
                r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            ),
        );
        assert_eq!(status, 404);
        let (status, headers, _) = http_exchange(
            addr,
            &format!(
                "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\r\n"
            ),
        );
        assert_eq!(status, 405);
        assert_eq!(
            headers.get("allow").map(String::as_str),
            Some("POST, DELETE")
        );

        // 5. Initialize -> 200 + MCP-Session-Id + read-only capabilities
        let init_body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let (status, headers, body) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&token),
                None,
                Some("http://localhost:3000"),
                init_body,
            ),
        );
        assert_eq!(status, 200);
        let sid = headers
            .get("mcp-session-id")
            .expect("MCP-Session-Id")
            .clone();
        assert_eq!(sid.len(), 32);
        let init_json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(init_json["result"]["protocolVersion"], "2025-06-18");

        // 6. Notification -> 202 Accepted
        let (status, _, body) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&token),
                Some(&sid),
                None,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            ),
        );
        assert_eq!(status, 202);
        assert!(body.is_empty());

        // 7. Chunked tools/list call over active session -> 200 (read-only hides cqs_docker_operate)
        let chunk_payload = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let chunked = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nMCP-Session-Id: {sid}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk_payload}\r\n0\r\n\r\n",
            chunk_payload.len()
        );
        let (status, _, body) = http_exchange(addr, &chunked);
        assert_eq!(status, 200);
        let tools_json: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<&str> = tools_json["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"cqs_system_status"));
        assert!(!names.contains(&"cqs_docker_operate"));

        // 8. Token rotation takes effect immediately without server restart!
        // Sleep slightly on platforms with coarse filesystem mtime if needed, and rotate changes size/content/ino.
        let new_token = rotate_token(&ctx).unwrap();
        // Force a different length or mtime by ensuring file replaced via atomic persist (new ino on Unix;
        // on Windows mtime/size update). Let's verify old fails and new succeeds:
        let (status_new, _, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&new_token),
                Some(&sid),
                None,
                r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
            ),
        );
        assert_eq!(status_new, 200);
        let (status_old, _, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&token),
                Some(&sid),
                None,
                r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#,
            ),
        );
        assert_eq!(status_old, 401);

        // 9. DELETE session -> 204, subsequent call with deleted session -> 404
        let (status, _, _) = http_exchange(
            addr,
            &format!(
                "DELETE /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {new_token}\r\nMCP-Session-Id: {sid}\r\n\r\n"
            ),
        );
        assert_eq!(status, 204);
        let (status, _, _) = http_exchange(
            addr,
            &post_req(
                "/mcp",
                Some(&new_token),
                Some(&sid),
                None,
                r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
            ),
        );
        assert_eq!(status, 404);

        // 10. Graceful shutdown via ctx.cancel
        cancel.store(true, Ordering::SeqCst);
        handle.join().unwrap();
    }
}
