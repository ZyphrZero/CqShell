//! Mihomo's configured REST controller, without shelling out with credentials.

use super::*;
use serde_json::Value as Json;
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    time::Instant,
};

const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const MAX_HEADERS: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Settings {
    pub address: String,
    pub url: String,
    pub bind_address: String,
    pub bind_loopback: bool,
    socket: SocketAddr,
    secret: String,
}

impl Settings {
    pub(crate) fn has_secret(&self) -> bool {
        !self.secret.is_empty()
    }
}

fn invalid(detail: &str) -> Error {
    Error::new("error.data", detail)
}

fn listener_address(value: &str) -> Result<SocketAddr> {
    // Do not resolve arbitrary names: the request must go to the address in
    // the configuration, with no DNS rebinding or URL interpretation.
    let socket = if let Some(port) = value.strip_prefix("localhost:") {
        let port = port
            .parse::<u16>()
            .map_err(|_| invalid("controller requires an IP address or localhost and a port"))?;
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    } else {
        value
            .parse::<SocketAddr>()
            .map_err(|_| invalid("controller requires an IP address or localhost and a port"))?
    };
    if socket.port() == 0 {
        return Err(invalid("controller port must be between 1 and 65535"));
    }
    Ok(socket)
}

fn socket_address(value: &str) -> Result<SocketAddr> {
    let mut socket = listener_address(value)?;
    // A wildcard is a listen address, not a client destination. Use its
    // same-family loopback address when connecting to the local listener.
    if socket.ip().is_unspecified() {
        socket.set_ip(match socket.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    Ok(socket)
}

fn valid_secret(value: &str) -> Result<()> {
    if value.len() > 1024 || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(invalid(
            "controller secret must contain at most 1024 printable ASCII characters without spaces",
        ));
    }
    Ok(())
}

fn validate_listeners(map: &serde_yaml::Mapping, secret: &str) -> Result<()> {
    for key in ["external-controller", "external-controller-tls"] {
        let Some(raw) = map.get(Value::String(key.into())) else {
            continue;
        };
        let raw = raw
            .as_str()
            .ok_or_else(|| invalid(&format!("{key} must be an address string")))?;
        if !raw.is_empty() && !listener_address(raw)?.ip().is_loopback() && secret.is_empty() {
            return Err(invalid(&format!(
                "a non-loopback {key} requires a secret; configure a secret before exposing the listener"
            )));
        }
    }
    Ok(())
}

pub(super) fn from_config(value: &Value) -> Result<Option<Settings>> {
    let map = value
        .as_mapping()
        .ok_or_else(|| invalid("Clash configuration root must be a map"))?;
    let secret = match map.get(Value::String("secret".into())) {
        None | Some(Value::Null) => "",
        Some(Value::String(value)) => value,
        _ => return Err(invalid("controller secret must be a string")),
    };
    valid_secret(secret)?;
    validate_listeners(map, secret)?;
    let Some(raw) = map.get(Value::String("external-controller".into())) else {
        return Ok(None);
    };
    let raw = raw
        .as_str()
        .ok_or_else(|| invalid("external-controller must be an address string"))?;
    if raw.is_empty() {
        return Ok(None);
    }
    let listener = listener_address(raw)?;
    let bind_loopback = listener.ip().is_loopback();
    let socket = socket_address(raw)?;
    let address = socket.to_string();
    Ok(Some(Settings {
        url: format!("http://{address}"),
        address,
        bind_address: raw.to_owned(),
        bind_loopback,
        socket,
        secret: secret.into(),
    }))
}

fn effective_config(ctx: &Context) -> Result<Value> {
    let bytes = if matches!(state(ctx)?, State::Running(_)) {
        storage::check_path(&ctx.paths.clash_runtime, true)?;
        fs::read(&ctx.paths.clash_runtime)?
    } else {
        build_runtime_config(ctx).map_err(|error| {
            if error.key == "error.data" {
                invalid("cannot parse controller configuration")
            } else {
                error
            }
        })?
    };
    serde_yaml::from_slice(&bytes).map_err(|_| invalid("cannot parse controller configuration"))
}

pub(crate) fn effective_settings(ctx: &Context) -> Result<Option<Settings>> {
    from_config(&effective_config(ctx)?)
}

pub(super) fn settings(ctx: &Context) -> Result<Settings> {
    effective_settings(ctx)?.ok_or_else(|| invalid("external-controller is not configured"))
}

pub(super) fn address(ctx: &Context) -> Result<Option<String>> {
    Ok(from_config(&effective_config(ctx)?)?.map(|settings| settings.url))
}

pub fn status(ctx: &Context) -> Result<()> {
    let detail = match from_config(&effective_config(ctx)?)? {
        Some(settings) => format!(
            "{}; secret={}",
            settings.url,
            if settings.secret.is_empty() {
                "none"
            } else {
                "set (redacted)"
            }
        ),
        None => "not configured".into(),
    };
    ctx.message("notice.clash_controller", detail);
    Ok(())
}

pub fn configure(ctx: &Context, address: &str) -> Result<()> {
    socket_address(address)?;
    let applied = edit_mixin_and_apply(ctx, MixinApplyMode::RestartIfRunning, |map| {
        map.insert(
            Value::String("external-controller".into()),
            Value::String(address.into()),
        );
        validate_override(ctx, map)
    })?;
    ctx.message(
        "notice.clash_controller",
        format!("{address} ({})", applied.suffix()),
    );
    Ok(())
}

/// Pick an available loopback controller address (`127.0.0.1:9090..=9099`, defaulting to `9090`).
pub(super) fn pick_loopback_controller_address() -> String {
    for port in 9090u16..=9099 {
        if std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return format!("127.0.0.1:{port}");
        }
    }
    "127.0.0.1:9090".into()
}

/// One-click helper for the Mihomo TUI workspace: enable a loopback-only REST controller
/// (`127.0.0.1:9090`, or the first free port in `9090..=9099`) and restart Mihomo automatically.
pub(crate) fn enable_loopback_and_apply(ctx: &Context) -> Result<String> {
    if let Ok(Some(existing)) = effective_settings(ctx) {
        return Ok(existing.bind_address);
    }
    let address = pick_loopback_controller_address();
    let _ = edit_mixin_and_apply(ctx, MixinApplyMode::RestartIfRunning, |map| {
        map.insert(
            Value::String("external-controller".into()),
            Value::String(address.clone()),
        );
        validate_override(ctx, map)
    })?;
    Ok(address)
}

/// Wait briefly for a freshly restarted Mihomo process to accept connections on its REST socket.
/// Fails immediately if the Mihomo process exits during startup; otherwise returns once ready
/// (or after timeout if the process remains running).
pub(super) fn wait_until_listening(
    ctx: &Context,
    settings: &Settings,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        ctx.check()?;
        if TcpStream::connect_timeout(&settings.socket, Duration::from_millis(120)).is_ok() {
            return Ok(());
        }
        match state(ctx)? {
            State::Running(_) => {}
            State::Stopped | State::NotInstalled => {
                let log_tail = logs::startup_tail(ctx).unwrap_or_default();
                return Err(Error::new(
                    "error.command",
                    format!(
                        "Mihomo exited before controller {} became ready: {}",
                        settings.address,
                        tail(&log_tail)
                    ),
                ));
            }
            State::Unknown => return Err(unknown_process()),
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn secret_generate(ctx: &Context) -> Result<()> {
    ctx.check()?;
    #[cfg(unix)]
    {
        let mut random = [0u8; 32];
        fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let secret = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        save_secret(ctx, &secret)
    }
    #[cfg(not(unix))]
    Err(Error::new(
        "error.platform",
        "secret generation requires Unix; use a private secret file",
    ))
}

pub fn secret_set(ctx: &Context, source: &str) -> Result<()> {
    let path = Path::new(source);
    storage::check_path(path, true)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > 1026 {
        return Err(invalid(
            "secret file must be a regular file of at most 1026 bytes",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid(
                "secret file must not be accessible by group or others (chmod 600)",
            ));
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(1027).read_to_end(&mut bytes)?;
    if bytes.len() > 1026 {
        return Err(invalid("secret file exceeds 1026 bytes"));
    }
    let secret =
        String::from_utf8(bytes).map_err(|_| invalid("cannot read secret file as UTF-8"))?;
    let secret = secret.strip_suffix('\n').unwrap_or(&secret);
    let secret = secret.strip_suffix('\r').unwrap_or(secret);
    if secret.is_empty() {
        return Err(invalid(
            "secret file is empty; use the clear action to disable authentication",
        ));
    }
    save_secret(ctx, secret)
}

pub fn secret_clear(ctx: &Context) -> Result<()> {
    save_secret(ctx, "")
}

fn save_secret(ctx: &Context, secret: &str) -> Result<()> {
    valid_secret(secret)?;
    let applied = edit_mixin_and_apply(ctx, MixinApplyMode::RestartIfRunning, |map| {
        map.insert(Value::String("secret".into()), Value::String(secret.into()));
        validate_override(ctx, map)
    })?;
    ctx.message("notice.clash_secret", applied.suffix());
    Ok(())
}

fn validate_override(ctx: &Context, map: &serde_yaml::Mapping) -> Result<()> {
    let bytes = runtime_with_mixin(ctx, Value::Mapping(map.clone())).map_err(|error| {
        if error.key == "error.data" {
            invalid("cannot parse controller configuration")
        } else {
            error
        }
    })?;
    let config = serde_yaml::from_slice(&bytes)
        .map_err(|_| invalid("cannot parse controller configuration"))?;
    from_config(&config)?;
    Ok(())
}

fn request(
    ctx: &Context,
    settings: &Settings,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<Vec<u8>> {
    request_with_timeout(ctx, settings, method, path, body, REQUEST_TIMEOUT)
}

fn request_with_timeout(
    ctx: &Context,
    settings: &Settings,
    method: &str,
    path: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>> {
    ctx.check()?;
    let start = Instant::now();
    let mut stream =
        TcpStream::connect_timeout(&settings.socket, Duration::from_secs(3)).map_err(|_| {
            Error::new(
                "error.command",
                "cannot connect to the configured Mihomo controller",
            )
        })?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(timeout))?;
    let authorization = if settings.secret.is_empty() {
        String::new()
    } else {
        format!("Authorization: Bearer {}\r\n", settings.secret)
    };
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\n{authorization}Content-Length: {}\r\n\r\n",
        settings.address,
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|_| Error::new("error.command", "cannot send controller request"))?;
    let mut bytes = Vec::new();
    loop {
        ctx.check()?;
        if start.elapsed() > timeout {
            return Err(Error::new("error.command", "controller response timed out"));
        }
        let mut buffer = [0u8; 8192];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => {
                if bytes.len() + size > MAX_RESPONSE + MAX_HEADERS {
                    return Err(invalid("controller response exceeds 16 MiB"));
                }
                bytes.extend_from_slice(&buffer[..size]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => {
                return Err(Error::new(
                    "error.command",
                    "cannot read controller response",
                ));
            }
        }
    }
    parse_response(&bytes)
}

fn parse_response(bytes: &[u8]) -> Result<Vec<u8>> {
    let split = bytes
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .filter(|&size| size <= MAX_HEADERS)
        .ok_or_else(|| invalid("invalid controller HTTP headers"))?;
    let header = std::str::from_utf8(&bytes[..split])
        .map_err(|_| invalid("invalid controller HTTP headers"))?;
    let mut lines = header.split("\r\n");
    let mut status = lines.next().unwrap_or("").split_whitespace();
    if !matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err(invalid("invalid controller HTTP status"));
    }
    let code: u16 = status
        .next()
        .and_then(|value| value.parse().ok())
        .filter(|value| (100..600).contains(value))
        .ok_or_else(|| invalid("invalid controller HTTP status"))?;
    // Never include response bodies: a controller can echo the Authorization
    // value in an error, and redirects must not forward its bearer token.
    if !(200..300).contains(&code) {
        return Err(Error::new(
            "error.command",
            format!("controller returned HTTP {code}"),
        ));
    }
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid controller HTTP header"))?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(invalid("duplicate controller Content-Length"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid("invalid controller Content-Length"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.trim().eq_ignore_ascii_case("chunked") {
                return Err(invalid("unsupported controller Transfer-Encoding"));
            }
            chunked = true;
        }
    }
    if chunked && length.is_some() {
        return Err(invalid("ambiguous controller response framing"));
    }
    let body = &bytes[split + 4..];
    if chunked {
        return unchunk(body);
    }
    if length.is_some_and(|size| size != body.len()) || body.len() > MAX_RESPONSE {
        return Err(invalid("truncated or oversized controller response"));
    }
    Ok(body.to_vec())
}

fn unchunk(mut bytes: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let end = bytes
            .windows(2)
            .position(|value| value == b"\r\n")
            .ok_or_else(|| invalid("truncated controller chunk"))?;
        let size = std::str::from_utf8(&bytes[..end])
            .ok()
            .and_then(|value| value.split(';').next())
            .and_then(|value| usize::from_str_radix(value, 16).ok())
            .ok_or_else(|| invalid("invalid controller chunk size"))?;
        bytes = &bytes[end + 2..];
        if size == 0 {
            if bytes == b"\r\n" || bytes.ends_with(b"\r\n\r\n") {
                return Ok(output);
            }
            return Err(invalid("invalid controller chunk trailer"));
        }
        if size > MAX_RESPONSE - output.len()
            || bytes.len() < size + 2
            || &bytes[size..size + 2] != b"\r\n"
        {
            return Err(invalid("truncated or oversized controller chunk"));
        }
        output.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
}

fn proxy_map(ctx: &Context, settings: &Settings) -> Result<serde_json::Map<String, Json>> {
    let body = request(ctx, settings, "GET", "/proxies", &[])?;
    let mut json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    json.get_mut("proxies")
        .and_then(Json::as_object_mut)
        .map(std::mem::take)
        .ok_or_else(|| invalid("controller response has no proxies map"))
}

fn redacted(settings: &Settings, value: &str) -> String {
    if settings.secret.is_empty() {
        value.into()
    } else {
        value.replace(&settings.secret, "[redacted]")
    }
}

pub fn proxies(ctx: &Context) -> Result<()> {
    list_proxies(ctx, None)
}

pub(super) fn list_proxies(ctx: &Context, group: Option<&str>) -> Result<()> {
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    if let Some(group) = group
        && !entries
            .get(group)
            .is_some_and(|entry| entry.get("all").is_some_and(Json::is_array))
    {
        return Err(Error::new("error.input", "unknown controller group"));
    }
    let mut groups = entries.into_iter().collect::<Vec<_>>();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rows = Vec::new();
    for (name, entry) in groups {
        if group.is_some_and(|group| group != name) {
            continue;
        }
        let Some(all) = entry.get("all").and_then(Json::as_array) else {
            continue;
        };
        let nodes = all
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| invalid("controller group contains an invalid node"))
            })
            .collect::<Result<Vec<_>>>()?;
        let current = entry.get("now").and_then(Json::as_str).unwrap_or("none");
        let kind = entry.get("type").and_then(Json::as_str).unwrap_or("group");
        rows.push(format!(
            "{name} [{kind}]; current={current}\n  {}",
            nodes.join(", ")
        ));
    }
    let detail = if rows.is_empty() {
        "no policy groups".into()
    } else {
        rows.join("\n")
    };
    ctx.message("notice.clash_proxies", redacted(&settings, &detail));
    Ok(())
}

/// Protocol and latest measured delay of a proxy, e.g. `Vmess · 87 ms`. A zero delay is a
/// failed test, so it is left out rather than shown as instant.
fn member_detail(entries: &serde_json::Map<String, Json>, name: &str) -> String {
    let Some(entry) = entries.get(name) else {
        return String::new();
    };
    let kind = entry.get("type").and_then(Json::as_str).unwrap_or("");
    let delay = entry
        .get("history")
        .and_then(Json::as_array)
        .and_then(|history| history.last())
        .and_then(|sample| sample.get("delay"))
        .and_then(Json::as_u64)
        .filter(|delay| *delay > 0);
    match delay {
        Some(delay) if !kind.is_empty() => format!("{kind} · {delay} ms"),
        Some(delay) => format!("{delay} ms"),
        None => kind.into(),
    }
}

/// Selector groups, the only ones `select` can switch, in configuration order, each with
/// its members. The controller map is unordered; GLOBAL lists every group in file order.
pub fn group_choices(ctx: &Context) -> Result<Vec<crate::ops::Choice>> {
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    let order = entries
        .get("GLOBAL")
        .and_then(|global| global.get("all"))
        .and_then(Json::as_array)
        .map(|all| all.iter().filter_map(Json::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut groups = Vec::new();
    for (name, entry) in &entries {
        if entry.get("type").and_then(Json::as_str) != Some("Selector") {
            continue;
        }
        let Some(all) = entry.get("all").and_then(Json::as_array) else {
            continue;
        };
        let now = entry.get("now").and_then(Json::as_str).unwrap_or("");
        let members = all
            .iter()
            .map(|value| {
                let member = value
                    .as_str()
                    .ok_or_else(|| invalid("controller group contains an invalid node"))?;
                Ok(crate::ops::Choice {
                    value: member.into(),
                    detail: member_detail(&entries, member),
                    current: member == now,
                    children: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        groups.push(crate::ops::Choice {
            value: name.clone(),
            detail: if now.is_empty() {
                String::new()
            } else {
                format!("→ {now}")
            },
            current: false,
            children: members,
        });
    }
    let rank = |name: &str| order.iter().position(|item| *item == name);
    // Groups missing from GLOBAL, including GLOBAL itself, follow in name order.
    groups.sort_by(|a, b| match (rank(&a.value), rank(&b.value)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.value.cmp(&b.value),
    });
    Ok(groups)
}

fn segment(value: &str) -> String {
    // Encode every byte except RFC 3986 unreserved bytes. In particular,
    // slashes and query/fragment delimiters are part of the group name.
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'~') {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

pub fn select(ctx: &Context, group: &str, node: &str) -> Result<()> {
    if group.is_empty() || node.is_empty() || group.len() > 4096 || node.len() > 4096 {
        return Err(Error::new(
            "error.input",
            "group and node must be nonempty names of at most 4096 bytes",
        ));
    }
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    let entry = entries
        .get(group)
        .ok_or_else(|| Error::new("error.input", "unknown controller group"))?;
    if entry.get("type").and_then(Json::as_str) != Some("Selector") {
        return Err(Error::new(
            "error.input",
            "only Selector policy groups support manual switching",
        ));
    }
    let nodes = entry
        .get("all")
        .and_then(Json::as_array)
        .ok_or_else(|| invalid("controller group has no node list"))?;
    if !nodes.iter().any(|value| value.as_str() == Some(node)) {
        return Err(Error::new(
            "error.input",
            "node does not belong to the selected controller group",
        ));
    }
    let body = serde_json::to_vec(&serde_json::json!({"name":node}))?;
    request(
        ctx,
        &settings,
        "PUT",
        &format!("/proxies/{}", segment(group)),
        &body,
    )?;
    ctx.message(
        "notice.clash_selected",
        redacted(&settings, &format!("{group} -> {node}")),
    );
    Ok(())
}

pub const DEFAULT_DELAY_URL: &str = "https://www.gstatic.com/generate_204";
const MIN_DELAY_TIMEOUT_MS: u64 = 500;
const MAX_DELAY_TIMEOUT_MS: u64 = 15_000;
const MAX_CONNECTIONS_DISPLAY: usize = 20;

pub(crate) fn validate_test_url(value: &str) -> Result<()> {
    let value = if value.is_empty() {
        DEFAULT_DELAY_URL
    } else {
        value
    };
    if value.len() > 2048 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::new(
            "error.url",
            "delay test URL must be an HTTP(S) URL without whitespace or control characters",
        ));
    }
    let parsed = url::Url::parse(value).map_err(|e| Error::new("error.url", e))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(Error::new(
            "error.url",
            "delay test URL must be HTTP or HTTPS without credentials",
        ));
    }
    Ok(())
}

pub(crate) fn validate_delay_timeout(value: &str) -> Result<u64> {
    let parsed = value.parse::<u64>().map_err(|_| {
        Error::new(
            "error.input",
            "delay timeout must be between 500 and 15000 ms",
        )
    })?;
    if !(MIN_DELAY_TIMEOUT_MS..=MAX_DELAY_TIMEOUT_MS).contains(&parsed) {
        return Err(Error::new(
            "error.input",
            "delay timeout must be between 500 and 15000 ms",
        ));
    }
    Ok(parsed)
}

/// All policy groups that support group latency tests, ordered as in `GLOBAL`.
pub fn delay_group_choices(ctx: &Context) -> Result<Vec<crate::ops::Choice>> {
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    let order = entries
        .get("GLOBAL")
        .and_then(|global| global.get("all"))
        .and_then(Json::as_array)
        .map(|all| all.iter().filter_map(Json::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut groups = Vec::new();
    for (name, entry) in &entries {
        if entry.get("all").and_then(Json::as_array).is_none() {
            continue;
        }
        let kind = entry.get("type").and_then(Json::as_str).unwrap_or("group");
        let now = entry.get("now").and_then(Json::as_str).unwrap_or("");
        let detail = if now.is_empty() {
            kind.into()
        } else {
            format!("{kind} · → {now}")
        };
        groups.push(crate::ops::Choice {
            value: name.clone(),
            detail,
            current: false,
            children: Vec::new(),
        });
    }
    let rank = |name: &str| order.iter().position(|item| *item == name);
    groups.sort_by(|a, b| match (rank(&a.value), rank(&b.value)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.value.cmp(&b.value),
    });
    Ok(groups)
}

/// Switch Mihomo's proxy routing mode (`rule`, `global`, or `direct`), optionally
/// persisting the choice to Mixin.
pub fn mode(ctx: &Context, mode: &str, persist: bool) -> Result<()> {
    ctx.check()?;
    if !matches!(mode, "rule" | "global" | "direct") {
        return Err(Error::new(
            "error.input",
            "proxy mode must be rule, global or direct",
        ));
    }
    let current = state(ctx)?;
    if matches!(current, State::Unknown) {
        return Err(unknown_process());
    }
    let body = serde_json::to_vec(&serde_json::json!({ "mode": mode }))?;
    if persist {
        if matches!(current, State::Running(_)) {
            let settings = settings(ctx)?;
            request(ctx, &settings, "PATCH", "/configs", &body)?;
            super::persist_running_mode(ctx, mode)?;
            ctx.message("notice.clash_mode", format!("{mode} (live + persisted)"));
        } else {
            let applied = edit_mixin_and_apply(ctx, MixinApplyMode::HotReloadOrRestart, |map| {
                map.insert(Value::String("mode".into()), Value::String(mode.into()));
                Ok(())
            })?;
            ctx.message(
                "notice.clash_mode",
                format!("{mode} ({})", applied.suffix()),
            );
        }
    } else {
        let settings = settings(ctx)?;
        request(ctx, &settings, "PATCH", "/configs", &body)?;
        ctx.message("notice.clash_mode", format!("{mode} (live)"));
    }
    Ok(())
}

pub(super) fn push_runtime(ctx: &Context, settings: &Settings, runtime: &[u8]) -> Result<()> {
    let payload = std::str::from_utf8(runtime)
        .map_err(|_| invalid("generated runtime configuration is not valid UTF-8"))?;
    let body = serde_json::to_vec(&serde_json::json!({ "payload": payload }))?;
    request_with_timeout(
        ctx,
        settings,
        "PUT",
        "/configs?force=true",
        &body,
        Duration::from_secs(15),
    )?;
    Ok(())
}

/// Rebuild, validate, atomically publish `clash_runtime`, and hot-reload Mihomo
/// via `PUT /configs?force=true` without restarting the process.
pub fn reload(ctx: &Context) -> Result<()> {
    ctx.check()?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let current_state = state(ctx)?;
    if matches!(current_state, State::Unknown) {
        return Err(unknown_process());
    }
    ensure_layout(ctx)?;
    let settings = settings(ctx)?;
    let previous = match fs::read(&ctx.paths.clash_runtime) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let runtime = build_runtime_config(ctx)?;
    let running_pid = match current_state {
        State::Running(pid) => Some(pid),
        _ => None,
    };
    super::tun::validate_reload(ctx, &runtime, running_pid)?;
    let candidate = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    fs::write(candidate.path(), &runtime)?;
    validate_config(ctx, candidate.path(), true)?;
    validate_with_mihomo(ctx, candidate.path())?;
    let payload = String::from_utf8(runtime.clone())
        .map_err(|_| invalid("generated runtime configuration is not valid UTF-8"))?;
    storage::atomic_write(&ctx.paths.clash_runtime, &runtime, 0o600)?;
    // Send the validated YAML via `payload` rather than `path`: Mihomo rejects
    // `path` outside its `-d` resource directory unless `SAFE_PATHS` includes
    // `clash_data`, so `payload` works even for already-running Mihomo daemons.
    let body = serde_json::to_vec(&serde_json::json!({ "payload": payload }))?;
    if let Err(error) = request_with_timeout(
        ctx,
        &settings,
        "PUT",
        "/configs?force=true",
        &body,
        Duration::from_secs(15),
    ) {
        let _ = match previous {
            Some(bytes) => storage::atomic_write(&ctx.paths.clash_runtime, &bytes, 0o600),
            None => storage::remove_file(&ctx.paths.clash_runtime),
        };
        if matches!(current_state, State::Stopped | State::NotInstalled)
            && error.detail.contains("cannot connect")
        {
            return Err(Error::new(
                "error.command",
                "Mihomo is not running; start Mihomo instead of hot-reloading",
            ));
        }
        return Err(error);
    }
    ctx.message(
        "notice.clash_reload",
        "configuration validated and hot-reloaded",
    );
    Ok(())
}

/// Measure latency for all nodes in a policy group via `GET /group/{name}/delay`.
pub fn delay(ctx: &Context, group: &str, test_url: &str, timeout_ms: &str) -> Result<()> {
    ctx.check()?;
    if group.is_empty() || group.len() > 4096 {
        return Err(Error::new(
            "error.input",
            "group must be a nonempty name of at most 4096 bytes",
        ));
    }
    let effective_url = if test_url.is_empty() {
        DEFAULT_DELAY_URL
    } else {
        test_url
    };
    validate_test_url(effective_url)?;
    let timeout = validate_delay_timeout(timeout_ms)?;
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    let members = entries
        .get(group)
        .and_then(|entry| entry.get("all"))
        .and_then(Json::as_array)
        .ok_or_else(|| Error::new("error.input", "unknown controller group"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("controller group contains an invalid node"))
        })
        .collect::<Result<Vec<_>>>()?;

    let path = format!(
        "/group/{}/delay?url={}&timeout={timeout}",
        segment(group),
        segment(effective_url)
    );
    let body = match request_with_timeout(
        ctx,
        &settings,
        "GET",
        &path,
        &[],
        Duration::from_millis(timeout + 5_000),
    ) {
        Ok(body) => body,
        // Upstream Mihomo returns HTTP 504 (or 408/503) when every node in a group times out.
        Err(err)
            if err.detail.contains("HTTP 408")
                || err.detail.contains("HTTP 503")
                || err.detail.contains("HTTP 504") =>
        {
            b"{}".to_vec()
        }
        Err(err) => return Err(err),
    };
    let delays: serde_json::Map<String, Json> =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let mut reachable: Vec<(String, u64)> = Vec::new();
    let mut timed_out: Vec<String> = Vec::new();
    for member in &members {
        match delays.get(member).and_then(Json::as_u64) {
            Some(ms) if ms > 0 => reachable.push((member.clone(), ms)),
            _ => timed_out.push(member.clone()),
        }
    }
    reachable.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut lines = Vec::new();
    lines.push(format!(
        "{group}: {}/{} reachable (timeout {} ms)",
        reachable.len(),
        members.len(),
        timeout
    ));
    for (name, ms) in &reachable {
        lines.push(format!("  {ms:>4} ms  {name}"));
    }
    for name in &timed_out {
        lines.push(format!("  timeout  {name}"));
    }
    ctx.message("notice.clash_delay", redacted(&settings, &lines.join("\n")));
    Ok(())
}

/// Inspect active connections and traffic totals, optionally closing all active connections.
pub fn connections(ctx: &Context, close_all: bool) -> Result<()> {
    ctx.check()?;
    let settings = settings(ctx)?;
    let body = request(ctx, &settings, "GET", "/connections", &[])?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let upload = json.get("uploadTotal").and_then(Json::as_u64).unwrap_or(0);
    let download = json
        .get("downloadTotal")
        .and_then(Json::as_u64)
        .unwrap_or(0);
    let memory = json.get("memory").and_then(Json::as_u64);
    let mut list: Vec<&Json> = json
        .get("connections")
        .and_then(Json::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let total_count = list.len();
    list.sort_by(|a, b| {
        let bytes = |item: &Json| {
            item.get("upload")
                .and_then(Json::as_u64)
                .unwrap_or(0)
                .saturating_add(item.get("download").and_then(Json::as_u64).unwrap_or(0))
        };
        bytes(b).cmp(&bytes(a))
    });
    let mut header = format!(
        "active={total_count}; up={}; down={}",
        format_bytes(upload),
        format_bytes(download)
    );
    if let Some(mem) = memory {
        header.push_str(&format!("; memory={}", format_bytes(mem)));
    }
    let mut lines = vec![header];
    for item in list.iter().take(MAX_CONNECTIONS_DISPLAY) {
        let meta = item.get("metadata");
        let network = meta
            .and_then(|m| m.get("network"))
            .and_then(Json::as_str)
            .unwrap_or("tcp");
        let host = meta
            .and_then(|m| m.get("host"))
            .and_then(Json::as_str)
            .filter(|h| !h.is_empty())
            .or_else(|| {
                meta.and_then(|m| m.get("destinationIP"))
                    .and_then(Json::as_str)
            })
            .unwrap_or("-");
        let port = meta
            .and_then(|m| m.get("destinationPort"))
            .and_then(Json::as_str)
            .unwrap_or("");
        let target = if port.is_empty() {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        };
        let chains = item
            .get("chains")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .rev()
                    .filter_map(Json::as_str)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into());
        let up = format_bytes(item.get("upload").and_then(Json::as_u64).unwrap_or(0));
        let down = format_bytes(item.get("download").and_then(Json::as_u64).unwrap_or(0));
        lines.push(format!(
            "  {network} {target} [{chains}] (up {up}, down {down})"
        ));
    }
    if total_count > MAX_CONNECTIONS_DISPLAY {
        lines.push(format!(
            "  ... {} more active connections",
            total_count - MAX_CONNECTIONS_DISPLAY
        ));
    }
    ctx.message(
        "notice.clash_connections",
        redacted(&settings, &lines.join("\n")),
    );
    if close_all {
        request(ctx, &settings, "DELETE", "/connections", &[])?;
        ctx.message("notice.clash_connections", "all active connections closed");
    }
    Ok(())
}

/// Structured live configuration read from `GET /configs`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ControllerConfigs {
    pub mode: String,
    pub mixed_port: Option<u16>,
    pub http_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub allow_lan: bool,
    pub tun_enabled: bool,
    pub tun_stack: String,
}

/// Raw snapshot returned by `GET /connections`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RawConnections {
    pub upload_total: u64,
    pub download_total: u64,
    pub memory: Option<u64>,
    pub connections: Vec<super::model::ConnectionItem>,
}

fn non_zero_port(value: Option<&Json>) -> Option<u16> {
    value
        .and_then(Json::as_u64)
        .filter(|&p| (1..=65535).contains(&p))
        .map(|p| p as u16)
}

pub(crate) fn fetch_version(ctx: &Context, settings: &Settings) -> Result<String> {
    let body = request_with_timeout(
        ctx,
        settings,
        "GET",
        "/version",
        &[],
        Duration::from_secs(3),
    )?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    Ok(json
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_owned())
}

pub(crate) fn fetch_configs(ctx: &Context, settings: &Settings) -> Result<ControllerConfigs> {
    let body = request_with_timeout(
        ctx,
        settings,
        "GET",
        "/configs",
        &[],
        Duration::from_secs(3),
    )?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let tun = json.get("tun");
    Ok(ControllerConfigs {
        mode: json
            .get("mode")
            .and_then(Json::as_str)
            .unwrap_or("rule")
            .to_ascii_lowercase(),
        mixed_port: non_zero_port(json.get("mixed-port")),
        http_port: non_zero_port(json.get("port")),
        socks_port: non_zero_port(json.get("socks-port")),
        allow_lan: json
            .get("allow-lan")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        tun_enabled: tun
            .and_then(|t| t.get("enable"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        tun_stack: tun
            .and_then(|t| t.get("stack"))
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

fn latest_history_delay(entry: &Json) -> (Option<u64>, bool) {
    let Some(history) = entry.get("history").and_then(Json::as_array) else {
        return (None, false);
    };
    if history.is_empty() {
        return (None, false);
    }
    let delay = history
        .last()
        .and_then(|sample| sample.get("delay"))
        .and_then(Json::as_u64)
        .filter(|&d| d > 0);
    (delay, true)
}

fn resolve_exit_chain(entries: &serde_json::Map<String, Json>, start_group: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = start_group;
    for _ in 0..16 {
        let Some(next) = entries
            .get(current)
            .and_then(|entry| entry.get("now"))
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
        else {
            break;
        };
        if chain.iter().any(|seen: &String| seen == next) {
            break;
        }
        chain.push(next.to_owned());
        current = next;
    }
    chain
}

/// Fetch all Mihomo policy groups and their member nodes in `GLOBAL` configuration order.
pub(crate) fn fetch_groups(
    ctx: &Context,
    settings: &Settings,
) -> Result<Vec<super::model::ProxyGroupItem>> {
    use super::model::{ProxyGroupItem, ProxyGroupKind, ProxyNodeItem};

    let entries = proxy_map(ctx, settings)?;
    let order = entries
        .get("GLOBAL")
        .and_then(|global| global.get("all"))
        .and_then(Json::as_array)
        .map(|all| all.iter().filter_map(Json::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut groups = Vec::new();
    for (name, entry) in &entries {
        let Some(all) = entry.get("all").and_then(Json::as_array) else {
            continue;
        };
        let kind_str = entry
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("Selector");
        let kind = ProxyGroupKind::parse(kind_str);
        let now = entry
            .get("now")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let fixed = entry
            .get("fixed")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let hidden = entry.get("hidden").and_then(Json::as_bool).unwrap_or(false);
        let resolved_chain = resolve_exit_chain(&entries, name);

        let mut members = Vec::with_capacity(all.len());
        for (idx, item) in all.iter().enumerate() {
            let Some(member_name) = item.as_str() else {
                continue;
            };
            let meta = entries.get(member_name);
            let m_kind = meta
                .and_then(|m| m.get("type"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            let is_group = meta.is_some_and(|m| m.get("all").is_some_and(Json::is_array));
            let m_now = meta
                .and_then(|m| m.get("now"))
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let udp = meta
                .and_then(|m| m.get("udp"))
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let alive = meta
                .and_then(|m| m.get("alive"))
                .and_then(Json::as_bool)
                .unwrap_or(true);
            let (delay_ms, tested) = meta.map(latest_history_delay).unwrap_or((None, false));
            members.push(ProxyNodeItem {
                name: member_name.to_owned(),
                kind: m_kind,
                delay_ms,
                tested,
                is_group,
                now: m_now,
                udp,
                alive,
                original_index: idx,
            });
        }

        groups.push(ProxyGroupItem {
            name: name.clone(),
            kind,
            now,
            resolved_chain,
            fixed,
            hidden,
            members,
        });
    }

    let rank = |name: &str| order.iter().position(|item| *item == name);
    groups.sort_by(|a, b| match (a.name == "GLOBAL", b.name == "GLOBAL") {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => match (rank(&a.name), rank(&b.name)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        },
    });
    Ok(groups)
}

/// Switch node on a `Selector`, `URLTest`, or `Fallback` group (supports temporary pinning on
/// automatic groups in modern Mihomo).
pub(crate) fn select_proxy_live(ctx: &Context, group: &str, node: &str) -> Result<()> {
    if group.is_empty() || node.is_empty() || group.len() > 4096 || node.len() > 4096 {
        return Err(Error::new(
            "error.input",
            "group and node must be nonempty names of at most 4096 bytes",
        ));
    }
    let settings = settings(ctx)?;
    let entries = proxy_map(ctx, &settings)?;
    let entry = entries
        .get(group)
        .ok_or_else(|| Error::new("error.input", "unknown controller group"))?;
    let kind_str = entry.get("type").and_then(Json::as_str).unwrap_or("");
    if !super::model::ProxyGroupKind::parse(kind_str).supports_select() {
        return Err(Error::new(
            "error.input",
            "policy group does not support manual node selection",
        ));
    }
    let nodes = entry
        .get("all")
        .and_then(Json::as_array)
        .ok_or_else(|| invalid("controller group has no node list"))?;
    if !nodes.iter().any(|value| value.as_str() == Some(node)) {
        return Err(Error::new(
            "error.input",
            "node does not belong to the selected controller group",
        ));
    }
    let body = serde_json::to_vec(&serde_json::json!({ "name": node }))?;
    request(
        ctx,
        &settings,
        "PUT",
        &format!("/proxies/{}", segment(group)),
        &body,
    )?;
    Ok(())
}

/// Clear temporary pinned node on a `URLTest` or `Fallback` group via `DELETE /proxies/{group}`.
pub(crate) fn unfix_group_live(ctx: &Context, group: &str) -> Result<()> {
    if group.is_empty() || group.len() > 4096 {
        return Err(Error::new("error.input", "invalid policy group name"));
    }
    let settings = settings(ctx)?;
    request(
        ctx,
        &settings,
        "DELETE",
        &format!("/proxies/{}", segment(group)),
        &[],
    )?;
    Ok(())
}

/// Batch latency test for a policy group via `GET /group/{name}/delay`.
pub(crate) fn delay_group_map(
    ctx: &Context,
    group: &str,
    test_url: &str,
    timeout_ms: u64,
) -> Result<BTreeMap<String, Option<u64>>> {
    let effective_url = if test_url.is_empty() {
        DEFAULT_DELAY_URL
    } else {
        test_url
    };
    validate_test_url(effective_url)?;
    let timeout = timeout_ms.clamp(MIN_DELAY_TIMEOUT_MS, MAX_DELAY_TIMEOUT_MS);
    let settings = settings(ctx)?;
    let path = format!(
        "/group/{}/delay?url={}&timeout={timeout}",
        segment(group),
        segment(effective_url)
    );
    let body = match request_with_timeout(
        ctx,
        &settings,
        "GET",
        &path,
        &[],
        Duration::from_millis(timeout + 5_000),
    ) {
        Ok(body) => body,
        // Upstream Mihomo returns HTTP 504 (or 408/503) when every node in a group times out.
        Err(err)
            if err.detail.contains("HTTP 408")
                || err.detail.contains("HTTP 503")
                || err.detail.contains("HTTP 504") =>
        {
            return Ok(BTreeMap::new());
        }
        Err(err) => return Err(err),
    };
    let delays: serde_json::Map<String, Json> =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let mut out = BTreeMap::new();
    for (name, val) in delays {
        out.insert(name, val.as_u64().filter(|&ms| ms > 0));
    }
    Ok(out)
}

/// Single-node latency test via `GET /proxies/{name}/delay`. HTTP 408/503/504 from Mihomo
/// indicates node timeout and maps cleanly to `Ok(None)`.
pub(crate) fn delay_node_ms(
    ctx: &Context,
    node: &str,
    test_url: &str,
    timeout_ms: u64,
) -> Result<Option<u64>> {
    let effective_url = if test_url.is_empty() {
        DEFAULT_DELAY_URL
    } else {
        test_url
    };
    validate_test_url(effective_url)?;
    let timeout = timeout_ms.clamp(MIN_DELAY_TIMEOUT_MS, MAX_DELAY_TIMEOUT_MS);
    let settings = settings(ctx)?;
    let path = format!(
        "/proxies/{}/delay?url={}&timeout={timeout}",
        segment(node),
        segment(effective_url)
    );
    match request_with_timeout(
        ctx,
        &settings,
        "GET",
        &path,
        &[],
        Duration::from_millis(timeout + 3_000),
    ) {
        Ok(body) => {
            let json: Json = serde_json::from_slice(&body)
                .map_err(|_| invalid("controller returned invalid JSON"))?;
            Ok(json
                .get("delay")
                .and_then(Json::as_u64)
                .filter(|&ms| ms > 0))
        }
        Err(err)
            if err.detail.contains("HTTP 408")
                || err.detail.contains("HTTP 503")
                || err.detail.contains("HTTP 504") =>
        {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

/// Fetch active connections snapshot from `GET /connections`.
pub(crate) fn fetch_connections_raw(ctx: &Context, settings: &Settings) -> Result<RawConnections> {
    use super::model::ConnectionItem;

    let body = request_with_timeout(
        ctx,
        settings,
        "GET",
        "/connections",
        &[],
        Duration::from_secs(3),
    )?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let upload_total = json.get("uploadTotal").and_then(Json::as_u64).unwrap_or(0);
    let download_total = json
        .get("downloadTotal")
        .and_then(Json::as_u64)
        .unwrap_or(0);
    let memory = json.get("memory").and_then(Json::as_u64);
    let mut connections = Vec::new();
    if let Some(items) = json.get("connections").and_then(Json::as_array) {
        for item in items {
            let id = item
                .get("id")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            if id.is_empty() {
                continue;
            }
            let meta = item.get("metadata");
            let network = meta
                .and_then(|m| m.get("network"))
                .and_then(Json::as_str)
                .unwrap_or("tcp")
                .to_ascii_uppercase();
            let conn_type = meta
                .and_then(|m| m.get("type"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            let src_ip = meta
                .and_then(|m| m.get("sourceIP"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let src_port = meta
                .and_then(|m| m.get("sourcePort"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let source = if src_port.is_empty() {
                src_ip.to_owned()
            } else {
                format!("{src_ip}:{src_port}")
            };
            let dst_ip = meta
                .and_then(|m| m.get("destinationIP"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let dst_port = meta
                .and_then(|m| m.get("destinationPort"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let destination = if dst_port.is_empty() {
                dst_ip.to_owned()
            } else {
                format!("{dst_ip}:{dst_port}")
            };
            let raw_host = meta
                .and_then(|m| m.get("host"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let host = if !raw_host.is_empty() && !dst_port.is_empty() {
                format!("{raw_host}:{dst_port}")
            } else if !raw_host.is_empty() {
                raw_host.to_owned()
            } else {
                destination.clone()
            };
            let process = meta
                .and_then(|m| m.get("process"))
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    meta.and_then(|m| m.get("processPath"))
                        .and_then(Json::as_str)
                        .and_then(|p| p.rsplit(['/', '\\']).next())
                })
                .unwrap_or("")
                .to_owned();
            let rule = item
                .get("rule")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            let rule_payload = item
                .get("rulePayload")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            let chains: Vec<String> = item
                .get("chains")
                .and_then(Json::as_array)
                .map(|arr| {
                    arr.iter()
                        .rev()
                        .filter_map(Json::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let exit_node = chains.last().cloned().unwrap_or_else(|| "-".to_owned());
            let upload = item.get("upload").and_then(Json::as_u64).unwrap_or(0);
            let download = item.get("download").and_then(Json::as_u64).unwrap_or(0);
            let start = item
                .get("start")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            connections.push(ConnectionItem {
                id,
                network,
                conn_type,
                source,
                destination,
                host,
                process,
                rule,
                rule_payload,
                chains,
                exit_node,
                upload,
                download,
                up_speed: 0,
                down_speed: 0,
                start,
                closed: false,
            });
        }
    }
    Ok(RawConnections {
        upload_total,
        download_total,
        memory,
        connections,
    })
}

pub(crate) fn close_connection(ctx: &Context, id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 256 {
        return Err(Error::new("error.input", "invalid connection id"));
    }
    let settings = settings(ctx)?;
    request(
        ctx,
        &settings,
        "DELETE",
        &format!("/connections/{}", segment(id)),
        &[],
    )?;
    Ok(())
}

pub(crate) fn close_all_connections(ctx: &Context) -> Result<()> {
    let settings = settings(ctx)?;
    request(ctx, &settings, "DELETE", "/connections", &[])?;
    Ok(())
}

pub(crate) fn fetch_rules(
    ctx: &Context,
    settings: &Settings,
) -> Result<Vec<super::model::RuleItem>> {
    let body = request_with_timeout(ctx, settings, "GET", "/rules", &[], Duration::from_secs(5))?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let Some(list) = json.get("rules").and_then(Json::as_array) else {
        return Ok(Vec::new());
    };
    let mut rules = Vec::with_capacity(list.len());
    for (idx, item) in list.iter().enumerate() {
        let kind = item
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let payload = item
            .get("payload")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let proxy = item
            .get("proxy")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        rules.push(super::model::RuleItem {
            index: idx + 1,
            kind,
            payload,
            proxy,
        });
    }
    Ok(rules)
}

fn parse_provider_userinfo(value: Option<&Json>) -> Option<super::model::SubscriptionUserinfo> {
    let map = value?.as_object()?;
    let upload = map
        .get("Upload")
        .or_else(|| map.get("upload"))
        .and_then(Json::as_u64)
        .unwrap_or(0);
    let download = map
        .get("Download")
        .or_else(|| map.get("download"))
        .and_then(Json::as_u64)
        .unwrap_or(0);
    let total = map
        .get("Total")
        .or_else(|| map.get("total"))
        .and_then(Json::as_u64)
        .unwrap_or(0);
    let expire = map
        .get("Expire")
        .or_else(|| map.get("expire"))
        .and_then(Json::as_u64)
        .filter(|&e| e > 0);
    if total > 0 || upload > 0 || download > 0 || expire.is_some() {
        Some(super::model::SubscriptionUserinfo {
            upload,
            download,
            total,
            expire,
        })
    } else {
        None
    }
}

pub(crate) fn fetch_proxy_providers(
    ctx: &Context,
    settings: &Settings,
) -> Result<Vec<super::model::ProxyProviderItem>> {
    let body = request_with_timeout(
        ctx,
        settings,
        "GET",
        "/providers/proxies",
        &[],
        Duration::from_secs(4),
    )?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let Some(map) = json.get("providers").and_then(Json::as_object) else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for (name, val) in map {
        let vehicle = val
            .get("vehicleType")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        if vehicle.eq_ignore_ascii_case("Compatible") {
            continue;
        }
        let node_count = val
            .get("proxies")
            .and_then(Json::as_array)
            .map_or(0, Vec::len);
        let updated_at = val
            .get("updatedAt")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let userinfo = parse_provider_userinfo(val.get("subscriptionInfo"));
        items.push(super::model::ProxyProviderItem {
            name: name.clone(),
            vehicle_type: vehicle,
            node_count,
            updated_at,
            userinfo,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

pub(crate) fn update_proxy_provider(ctx: &Context, name: &str) -> Result<()> {
    let settings = settings(ctx)?;
    request_with_timeout(
        ctx,
        &settings,
        "PUT",
        &format!("/providers/proxies/{}", segment(name)),
        &[],
        Duration::from_secs(20),
    )?;
    Ok(())
}

pub(crate) fn healthcheck_proxy_provider(ctx: &Context, name: &str) -> Result<()> {
    let settings = settings(ctx)?;
    request_with_timeout(
        ctx,
        &settings,
        "GET",
        &format!("/providers/proxies/{}/healthcheck", segment(name)),
        &[],
        Duration::from_secs(20),
    )?;
    Ok(())
}

pub(crate) fn fetch_rule_providers(
    ctx: &Context,
    settings: &Settings,
) -> Result<Vec<super::model::RuleProviderItem>> {
    let body = request_with_timeout(
        ctx,
        settings,
        "GET",
        "/providers/rules",
        &[],
        Duration::from_secs(4),
    )?;
    let json: Json =
        serde_json::from_slice(&body).map_err(|_| invalid("controller returned invalid JSON"))?;
    let Some(map) = json.get("providers").and_then(Json::as_object) else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for (name, val) in map {
        let behavior = val
            .get("behavior")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let vehicle_type = val
            .get("vehicleType")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let rule_count = val.get("ruleCount").and_then(Json::as_u64).unwrap_or(0) as usize;
        let updated_at = val
            .get("updatedAt")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        items.push(super::model::RuleProviderItem {
            name: name.clone(),
            behavior,
            vehicle_type,
            rule_count,
            updated_at,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

pub(crate) fn update_rule_provider(ctx: &Context, name: &str) -> Result<()> {
    let settings = settings(ctx)?;
    request_with_timeout(
        ctx,
        &settings,
        "PUT",
        &format!("/providers/rules/{}", segment(name)),
        &[],
        Duration::from_secs(20),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::context;
    use std::net::TcpListener;

    #[test]
    fn controller_addresses_are_explicit_and_wildcards_become_loopback() {
        assert_eq!(
            socket_address("0.0.0.0:9090").unwrap().to_string(),
            "127.0.0.1:9090"
        );
        assert_eq!(
            socket_address("[::]:9090").unwrap().to_string(),
            "[::1]:9090"
        );
        assert_eq!(
            socket_address("localhost:9090").unwrap().to_string(),
            "127.0.0.1:9090"
        );
        for value in [
            "",
            "127.0.0.1:0",
            "localhost:65536",
            "example.com:9090",
            "http://127.0.0.1:9090",
            "user:pass@127.0.0.1:9090",
            "127.0.0.1:9090/path",
            "127.0.0.1:9090?secret=x",
            "127.0.0.1:9090\r\n",
        ] {
            assert!(socket_address(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn secrets_and_names_do_not_inject_headers_or_url_paths() {
        for value in ["abc\r\nInjected: yes", "a b", "密钥"] {
            assert!(valid_secret(value).is_err());
        }
        assert!(valid_secret("aBc-123_\"\\").is_ok());
        assert_eq!(segment("组/one?x#\""), "%E7%BB%84%2Fone%3Fx%23%22");
        assert_eq!(segment(".."), "%2E%2E");
        let settings = from_config(
            &serde_yaml::from_str("external-controller: 127.0.0.1:9090\nsecret: private-token\n")
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(redacted(&settings, "name private-token"), "name [redacted]");
    }

    #[test]
    fn bounded_http_parser_rejects_status_errors_bad_framing_and_truncation() {
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(),
            b"{}"
        );
        assert_eq!(
            parse_response(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"
            )
            .unwrap(),
            b"{}"
        );
        for bytes in [
            b"HTTP/1.1 302 Found\r\nLocation: http://elsewhere\r\n\r\n".as_slice(),
            b"HTTP/1.1 401 Unauthorized\r\n\r\nprivate-token",
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\n",
        ] {
            let error = parse_response(bytes).unwrap_err();
            assert!(!error.to_string().contains("private-token"));
        }
    }

    fn serve(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0u8; 1024];
                    let size = stream.read(&mut buffer).unwrap();
                    assert_ne!(size, 0);
                    bytes.extend_from_slice(&buffer[..size]);
                    if let Some(split) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                        let header = std::str::from_utf8(&bytes[..split]).unwrap();
                        let len = header
                            .lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() >= split + 4 + len {
                            break;
                        }
                    }
                }
                requests.push(bytes);
                stream.write_all(&response).unwrap();
            }
            requests
        });
        (address, worker)
    }

    fn response(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn real_http_select_encodes_group_serializes_node_and_authenticates() {
        let group = "组/one?x";
        let node = "node\"\\two";
        let proxies =
            serde_json::json!({"proxies":{group:{"type":"Selector","all":[node],"now":"DIRECT"}}})
                .to_string();
        let (address, worker) = serve(vec![
            response(&proxies),
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\nsecret: private-token\n"),
        )
        .unwrap();
        let before = fs::read(&ctx.paths.clash_config).unwrap();
        select(&ctx, group, node).unwrap();
        let requests = worker.join().unwrap();
        let put = String::from_utf8(requests[1].clone()).unwrap();
        assert!(put.starts_with("PUT /proxies/%E7%BB%84%2Fone%3Fx HTTP/1.1\r\n"));
        assert!(put.contains("Authorization: Bearer private-token\r\n"));
        let body = put.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(serde_json::from_str::<Json>(body).unwrap()["name"], node);
        assert_eq!(fs::read(&ctx.paths.clash_config).unwrap(), before);
    }

    #[test]
    fn nonexistent_nodes_and_automatic_groups_do_not_send_put_or_change_files() {
        for kind in ["Selector", "URLTest"] {
            let body =
                serde_json::json!({"proxies":{"group":{"type":kind,"all":["one"]}}}).to_string();
            let (address, worker) = serve(vec![response(&body)]);
            let temp = tempfile::tempdir().unwrap();
            let ctx = context(temp.path());
            ensure_layout(&ctx).unwrap();
            fs::write(
                &ctx.paths.clash_config,
                format!("external-controller: {address}\n"),
            )
            .unwrap();
            let before = fs::read(&ctx.paths.clash_config).unwrap();
            assert!(
                select(
                    &ctx,
                    "group",
                    if kind == "Selector" { "missing" } else { "one" }
                )
                .is_err()
            );
            assert_eq!(worker.join().unwrap().len(), 1);
            assert_eq!(fs::read(&ctx.paths.clash_config).unwrap(), before);
        }
    }

    #[test]
    fn group_choices_list_selectors_in_config_order_with_member_details() {
        let body = serde_json::json!({"proxies": {
            "GLOBAL": {"type": "Selector", "all": ["Proxy", "Auto", "Streaming"], "now": "Proxy"},
            "Auto": {"type": "URLTest", "all": ["HK"], "now": "HK"},
            "Streaming": {"type": "Selector", "all": ["JP", "DIRECT"], "now": "JP"},
            "Proxy": {"type": "Selector", "all": ["Auto", "HK", "JP"], "now": "HK"},
            "HK": {"type": "Vmess", "history": [{"delay": 120}, {"delay": 87}]},
            "JP": {"type": "Trojan", "history": [{"delay": 0}]},
            "DIRECT": {"type": "Direct"}
        }})
        .to_string();
        let (address, worker) = serve(vec![response(&body)]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\n"),
        )
        .unwrap();
        let groups = group_choices(&ctx).unwrap();
        assert_eq!(
            worker.join().unwrap().len(),
            1,
            "one request lists everything"
        );
        let names = groups.iter().map(|g| g.value.as_str()).collect::<Vec<_>>();
        assert_eq!(
            names,
            ["Proxy", "Streaming", "GLOBAL"],
            "file order, automatic groups skipped"
        );
        let proxy = &groups[0];
        assert_eq!(proxy.detail, "→ HK");
        let members = proxy
            .children
            .iter()
            .map(|m| (m.value.as_str(), m.detail.as_str(), m.current))
            .collect::<Vec<_>>();
        assert_eq!(
            members,
            [
                ("Auto", "URLTest", false),
                ("HK", "Vmess · 87 ms", true),
                ("JP", "Trojan", false),
            ],
            "latest delay shown; a failed test is not shown as a delay"
        );
    }

    #[test]
    fn status_uses_mixin_instead_of_stale_stopped_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_runtime,
            "external-controller: 127.0.0.1:1\n",
        )
        .unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "external-controller: 127.0.0.1:9090\nsecret: hidden\n",
        )
        .unwrap();
        assert_eq!(address(&ctx).unwrap().unwrap(), "http://127.0.0.1:9090");
    }

    #[test]
    fn real_http_errors_never_echo_response_tokens_or_follow_redirects() {
        for code in [302, 400, 401, 404] {
            let body = "private-token";
            let response = format!(
                "HTTP/1.1 {code} Failed\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let (address, worker) = serve(vec![response.into_bytes()]);
            let temp = tempfile::tempdir().unwrap();
            let ctx = context(temp.path());
            ensure_layout(&ctx).unwrap();
            fs::write(
                &ctx.paths.clash_mixin,
                format!("external-controller: {address}\nsecret: private-token\n"),
            )
            .unwrap();
            let error = proxies(&ctx).unwrap_err();
            assert!(error.detail.contains(&code.to_string()));
            assert!(!error.to_string().contains(body));
            assert_eq!(worker.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn cancelled_controller_request_stops_before_connecting() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        let settings =
            from_config(&serde_yaml::from_str("external-controller: 127.0.0.1:1\n").unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(
            request(&ctx, &settings, "GET", "/proxies", &[])
                .unwrap_err()
                .key,
            "error.cancelled"
        );
    }

    #[test]
    fn secret_file_and_controller_settings_preserve_existing_mixin() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "log-level: warning\n").unwrap();
        let source = temp.path().join("secret");
        storage::atomic_write(&source, b"private-token\n", 0o600).unwrap();
        secret_set(&ctx, source.to_str().unwrap()).unwrap();
        configure(&ctx, "127.0.0.1:9090").unwrap();
        let bytes = fs::read(&ctx.paths.clash_mixin).unwrap();
        let mixin: Value = serde_yaml::from_slice(&bytes).unwrap();
        assert_eq!(mixin["secret"], "private-token");
        assert_eq!(mixin["log-level"], "warning");
        assert_eq!(mixin["external-controller"], "127.0.0.1:9090");
        assert!(configure(&ctx, "http://credentials@127.0.0.1:9090").is_err());
        assert_eq!(fs::read(&ctx.paths.clash_mixin).unwrap(), bytes);
        secret_clear(&ctx).unwrap();
        assert!(settings(&ctx).unwrap().secret.is_empty());
    }

    #[test]
    fn non_loopback_listeners_require_a_secret_and_cannot_clear_it() {
        for bind in ["0.0.0.0:9090", "[::]:9090", "192.0.2.1:9090"] {
            let temp = tempfile::tempdir().unwrap();
            let ctx = context(temp.path());
            assert!(configure(&ctx, bind).is_err());
            assert!(!ctx.paths.clash_mixin.exists());
            let source = temp.path().join("secret");
            storage::atomic_write(&source, b"private-token", 0o600).unwrap();
            secret_set(&ctx, source.to_str().unwrap()).unwrap();
            configure(&ctx, bind).unwrap();
            let before = fs::read(&ctx.paths.clash_mixin).unwrap();
            assert!(secret_clear(&ctx).is_err());
            assert_eq!(fs::read(&ctx.paths.clash_mixin).unwrap(), before);
        }
    }

    #[test]
    fn non_loopback_tls_listener_requires_the_shared_secret() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "external-controller: 127.0.0.1:9090\nexternal-controller-tls: 0.0.0.0:9443\nsecret: private-token\n",
        )
        .unwrap();
        let before = fs::read(&ctx.paths.clash_mixin).unwrap();
        assert!(secret_clear(&ctx).is_err());
        assert_eq!(fs::read(&ctx.paths.clash_mixin).unwrap(), before);
        assert!(!ctx.paths.clash_runtime.exists());
        for http in [None, Some(""), Some("127.0.0.1:9090")] {
            for tls in ["0.0.0.0:9443", "[::]:9443", "192.0.2.1:9443"] {
                let mut map = serde_yaml::Mapping::new();
                if let Some(http) = http {
                    map.insert("external-controller".into(), http.into());
                }
                map.insert("external-controller-tls".into(), tls.into());
                assert!(from_config(&Value::Mapping(map.clone())).is_err());
                map.insert("secret".into(), "private-token".into());
                assert!(from_config(&Value::Mapping(map)).is_ok());
            }
        }
        for tls in ["127.0.0.1:9443", "[::1]:9443", "localhost:9443", ""] {
            let config = serde_yaml::from_str(&format!(
                "external-controller: 127.0.0.1:9090\nexternal-controller-tls: '{tls}'\n"
            ))
            .unwrap();
            assert!(from_config(&config).unwrap().is_some());
        }
        for tls in ["[]", "null", "'invalid'", "'127.0.0.1:0'"] {
            let config = serde_yaml::from_str(&format!(
                "external-controller-tls: {tls}\nsecret: private-token\n"
            ))
            .unwrap();
            assert!(from_config(&config).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a verified Mihomo binary in CQS_CLASH_SMOKE_BINARY"]
    fn real_mihomo_controller_switches_encoded_groups_without_changing_config() {
        let binary = std::env::var_os("CQS_CLASH_SMOKE_BINARY").expect("CQS_CLASH_SMOKE_BINARY");
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
        fs::copy(binary, &ctx.paths.clash_binary).unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let group = "策略/测试?group";
        let config = format!(
            "mixed-port: 0\nexternal-controller: 127.0.0.1:{port}\nsecret: controller-smoke-token\nmode: rule\nproxies:\n  - name: direct-one\n    type: direct\n  - name: reject-two\n    type: reject\nproxy-groups:\n  - name: '{group}'\n    type: select\n    proxies: [direct-one, reject-two]\nrules:\n  - 'MATCH,{group}'\n"
        );
        fs::write(&ctx.paths.clash_config, &config).unwrap();
        start_inner(&ctx, false).unwrap();
        struct StopOnDrop(Context);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                let _ = stop(&self.0.recovery());
            }
        }
        let _cleanup = StopOnDrop(ctx.clone());
        let settings = settings(&ctx).unwrap();
        let mut ready = false;
        for _ in 0..30 {
            if proxy_map(&ctx, &settings).is_ok() {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "Mihomo controller did not become ready");
        select(&ctx, group, "reject-two").unwrap();
        let entries = proxy_map(&ctx, &settings).unwrap();
        assert_eq!(entries[group]["now"], "reject-two");
        assert_eq!(fs::read_to_string(&ctx.paths.clash_config).unwrap(), config);
        assert!(select(&ctx, group, "does-not-exist").is_err());
        assert_eq!(
            proxy_map(&ctx, &settings).unwrap()[group]["now"],
            "reject-two"
        );
        let State::Running(pid) = state(&ctx).unwrap() else {
            panic!("Mihomo process ownership was lost");
        };
        stop(&ctx).unwrap();
        assert!(!ctx.paths.clash_pid.exists());
        assert!(!ctx.paths.clash_pid_start.exists());
        let wait_for_reaping = |pid| {
            let process = PathBuf::from(format!("/proc/{pid}"));
            let deadline = Instant::now() + STOP_WAIT;
            while process.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!process.exists(), "Mihomo child {pid} was not reaped");
        };
        wait_for_reaping(pid);

        // Setting cancellation after the PID is persisted targets the short
        // startup verification window, rather than YAML validation or -t.
        let watcher_context = ctx.clone();
        let watcher = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Ok(text) = fs::read_to_string(&watcher_context.paths.clash_pid)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    watcher_context
                        .cancel
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return pid;
                }
                thread::sleep(Duration::from_millis(1));
            }
            panic!("Mihomo PID was not published during restart");
        });
        let result = start_inner(&ctx, false);
        let pid = watcher.join().unwrap();
        ctx.cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(result.unwrap_err().key, "error.cancelled");
        assert!(!ctx.paths.clash_pid.exists());
        assert!(!ctx.paths.clash_pid_start.exists());
        wait_for_reaping(pid);
    }

    #[test]
    fn mode_switches_live_and_persists_when_requested() {
        let (address, worker) = serve(vec![
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!(
                "mode: rule\nexternal-controller: {address}\nsecret: mode-secret\nproxies:\n  - name: p1\n    type: direct\n"
            ),
        )
        .unwrap();
        mode(&ctx, "global", false).unwrap();
        let requests = worker.join().unwrap();
        let patch = String::from_utf8(requests[0].clone()).unwrap();
        assert!(patch.starts_with("PATCH /configs HTTP/1.1\r\n"));
        assert!(patch.contains("Authorization: Bearer mode-secret\r\n"));
        let body = patch.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(
            serde_json::from_str::<Json>(body).unwrap()["mode"],
            "global"
        );
        assert!(!ctx.paths.clash_mixin.exists());

        // Persist when stopped updates Mixin cleanly without calling HTTP.
        mode(&ctx, "direct", true).unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["mode"], "direct");
        assert!(mode(&ctx, "invalid", false).is_err());
    }

    #[test]
    fn reload_validates_writes_runtime_and_rolls_back_on_http_error() {
        let (address, worker) = serve(vec![
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("mode: direct\nexternal-controller: {address}\nsecret: reload-secret\n"),
        )
        .unwrap();
        fs::write(&ctx.paths.clash_mixin, "log-level: info\n").unwrap();
        reload(&ctx).unwrap();
        let saved = fs::read(&ctx.paths.clash_runtime).unwrap();
        assert!(String::from_utf8_lossy(&saved).contains("log-level: info"));

        // Change Mixin, trigger HTTP 500 -> clash_runtime must roll back to previous bytes.
        fs::write(&ctx.paths.clash_mixin, "log-level: debug\n").unwrap();
        assert!(reload(&ctx).is_err());
        assert_eq!(fs::read(&ctx.paths.clash_runtime).unwrap(), saved);

        let requests = worker.join().unwrap();
        let put = String::from_utf8(requests[0].clone()).unwrap();
        assert!(put.starts_with("PUT /configs?force=true HTTP/1.1\r\n"));
        assert!(put.contains("Authorization: Bearer reload-secret\r\n"));
        let body = put.split_once("\r\n\r\n").unwrap().1;
        let json: Json = serde_json::from_str(body).unwrap();
        assert!(json.get("path").is_none());
        assert!(
            json["payload"]
                .as_str()
                .unwrap()
                .contains("log-level: info")
        );
    }

    #[test]
    fn delay_tests_encoded_group_sorts_results_and_validates_inputs() {
        let group = "节点/测试";
        let proxies = serde_json::json!({
            "proxies": {
                group: {"type": "URLTest", "all": ["Slow", "Fast", "Dead"], "now": "Fast"},
                "GLOBAL": {"type": "Selector", "all": [group], "now": group}
            }
        })
        .to_string();
        let delays = serde_json::json!({"Slow": 210, "Fast": 42}).to_string();
        let (address, worker) = serve(vec![
            response(&proxies),
            response(&delays),
            response(&proxies),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\nsecret: delay-secret\n"),
        )
        .unwrap();
        delay(&ctx, group, "", "3000").unwrap();
        let groups = delay_group_choices(&ctx).unwrap();
        assert_eq!(
            groups.iter().map(|g| g.value.as_str()).collect::<Vec<_>>(),
            vec![group, "GLOBAL"]
        );
        let requests = worker.join().unwrap();
        let get_delay = String::from_utf8(requests[1].clone()).unwrap();
        assert!(get_delay.starts_with("GET /group/%E8%8A%82%E7%82%B9%2F%E6%B5%8B%E8%AF%95/delay?url=https%3A%2F%2Fwww%2Egstatic%2Ecom%2Fgenerate_204&timeout=3000 HTTP/1.1\r\n"));
        assert!(validate_test_url("ftp://example.com").is_err());
        assert!(validate_test_url("https://user:pass@example.com").is_err());
        assert!(validate_delay_timeout("100").is_err());
        assert!(validate_delay_timeout("99999").is_err());
    }

    #[test]
    fn connections_reads_summary_and_closes_when_requested() {
        let body = serde_json::json!({
            "uploadTotal": 2048,
            "downloadTotal": 1_048_576,
            "memory": 16_777_216,
            "connections": [
                {
                    "upload": 512,
                    "download": 4096,
                    "chains": ["HK-01", "Proxy"],
                    "metadata": {"network": "tcp", "host": "example.com", "destinationPort": "443"}
                }
            ]
        })
        .to_string();
        let (address, worker) = serve(vec![
            response(&body),
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\nsecret: conn-secret\n"),
        )
        .unwrap();
        connections(&ctx, true).unwrap();
        let requests = worker.join().unwrap();
        assert!(
            String::from_utf8(requests[0].clone())
                .unwrap()
                .starts_with("GET /connections HTTP/1.1\r\n")
        );
        assert!(
            String::from_utf8(requests[1].clone())
                .unwrap()
                .starts_with("DELETE /connections HTTP/1.1\r\n")
        );
    }

    #[test]
    fn structured_controller_queries_resolve_chains_pin_unfix_and_handle_node_timeout() {
        let proxies_body = serde_json::json!({
            "proxies": {
                "GLOBAL": { "type": "Selector", "all": ["Proxy", "Auto"], "now": "Proxy" },
                "Proxy": { "type": "Selector", "all": ["Auto", "JP-01"], "now": "Auto" },
                "Auto": { "type": "URLTest", "all": ["JP-01", "HK-01"], "now": "JP-01", "fixed": "JP-01" },
                "JP-01": { "type": "Hysteria2", "udp": true, "alive": true, "history": [{"delay": 74}] },
                "HK-01": { "type": "Vmess", "udp": true, "alive": true, "history": [{"delay": 0}] }
            }
        })
        .to_string();
        let rules_body = serde_json::json!({
            "rules": [
                { "type": "DomainSuffix", "payload": "openai.com", "proxy": "Proxy" },
                { "type": "Match", "payload": "", "proxy": "DIRECT" }
            ]
        })
        .to_string();
        let pp_body = serde_json::json!({
            "providers": {
                "MainSub": {
                    "vehicleType": "HTTP",
                    "proxies": [{"name": "JP-01"}],
                    "updatedAt": "2026-11-26T00:00:00Z",
                    "subscriptionInfo": { "Upload": 100, "Download": 200, "Total": 1000, "Expire": 1795046400 }
                },
                "default": { "vehicleType": "Compatible", "proxies": [] }
            }
        })
        .to_string();
        let rp_body = serde_json::json!({
            "providers": {
                "RejectAds": { "behavior": "domain", "vehicleType": "HTTP", "ruleCount": 1200, "updatedAt": "2026-11-26T00:00:00Z" }
            }
        })
        .to_string();

        let (address, worker) = serve(vec![
            response(&proxies_body),
            response(&proxies_body),
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
            response(r#"{"delay":88}"#),
            b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\n\r\n".to_vec(),
            response(&rules_body),
            response(&pp_body),
            response(&rp_body),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\nsecret: ui-secret\n"),
        )
        .unwrap();

        let s = settings(&ctx).unwrap();
        let groups = fetch_groups(&ctx, &s).unwrap();
        assert_eq!(groups[0].name, "Proxy");
        assert_eq!(groups[0].resolved_chain, vec!["Auto", "JP-01"]);
        assert_eq!(groups[1].name, "Auto");
        assert_eq!(groups[1].fixed.as_deref(), Some("JP-01"));
        assert_eq!(groups[1].members[0].delay_ms, Some(74));
        assert!(groups[1].members[1].tested && groups[1].members[1].delay_ms.is_none());
        assert_eq!(groups.last().unwrap().name, "GLOBAL");

        // Pin node on URLTest group + unfix via DELETE
        select_proxy_live(&ctx, "Auto", "HK-01").unwrap();
        unfix_group_live(&ctx, "Auto").unwrap();

        // Single-node ping success & 504 timeout -> Ok(None)
        assert_eq!(
            delay_node_ms(&ctx, "JP-01", DEFAULT_DELAY_URL, 3000).unwrap(),
            Some(88)
        );
        assert_eq!(
            delay_node_ms(&ctx, "HK-01", DEFAULT_DELAY_URL, 3000).unwrap(),
            None
        );

        let rules = fetch_rules(&ctx, &s).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].payload, "openai.com");

        let pps = fetch_proxy_providers(&ctx, &s).unwrap();
        assert_eq!(pps.len(), 1);
        assert_eq!(pps[0].name, "MainSub");
        assert_eq!(pps[0].userinfo.unwrap().used(), 300);

        let rps = fetch_rule_providers(&ctx, &s).unwrap();
        assert_eq!(rps.len(), 1);
        assert_eq!(rps[0].rule_count, 1200);

        let reqs = worker.join().unwrap();
        let put_auto = String::from_utf8(reqs[2].clone()).unwrap();
        let del_auto = String::from_utf8(reqs[3].clone()).unwrap();
        assert!(put_auto.starts_with("PUT /proxies/Auto HTTP/1.1\r\n"));
        assert!(put_auto.contains("Authorization: Bearer ui-secret\r\n"));
        assert!(del_auto.starts_with("DELETE /proxies/Auto HTTP/1.1\r\n"));
    }

    #[test]
    fn controller_metadata_group_timeout_close_and_provider_mutations_work() {
        let ver_body = serde_json::json!({ "version": "v1.19.33" }).to_string();
        let cfg_body = serde_json::json!({
            "mode": "Global",
            "mixed-port": 7890,
            "port": 7891,
            "socks-port": 7892,
            "allow-lan": true,
            "tun": { "enable": true, "stack": "mixed" }
        })
        .to_string();
        let delay_ok = serde_json::json!({ "HK-01": 65, "Dead": 0 }).to_string();
        let no_content = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec();
        let timeout_504 = b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\n\r\n".to_vec();

        let (address, worker) = serve(vec![
            response(&ver_body),
            response(&cfg_body),
            response(&delay_ok),
            timeout_504,
            no_content.clone(),
            no_content.clone(),
            no_content.clone(),
            no_content,
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_config,
            format!("external-controller: {address}\nsecret: meta-secret\n"),
        )
        .unwrap();

        let s = effective_settings(&ctx).unwrap().unwrap();
        assert!(s.has_secret());
        assert_eq!(fetch_version(&ctx, &s).unwrap(), "v1.19.33");

        let cfg = fetch_configs(&ctx, &s).unwrap();
        assert_eq!(cfg.mode, "global");
        assert_eq!(cfg.mixed_port, Some(7890));
        assert!(cfg.allow_lan);
        assert!(cfg.tun_enabled);
        assert_eq!(cfg.tun_stack, "mixed");

        let map = delay_group_map(&ctx, "Proxy", DEFAULT_DELAY_URL, 3000).unwrap();
        assert_eq!(map.get("HK-01"), Some(&Some(65)));
        assert_eq!(map.get("Dead"), Some(&None));

        // Whole-group HTTP 504 timeout maps cleanly to empty map rather than failing
        let all_timed_out = delay_group_map(&ctx, "Proxy", DEFAULT_DELAY_URL, 3000).unwrap();
        assert!(all_timed_out.is_empty());

        close_connection(&ctx, "conn/id?1").unwrap();
        update_proxy_provider(&ctx, "Main/Sub").unwrap();
        healthcheck_proxy_provider(&ctx, "Main/Sub").unwrap();
        update_rule_provider(&ctx, "Ads Rule").unwrap();

        let reqs = worker.join().unwrap();
        let req_str = |idx: usize| String::from_utf8(reqs[idx].clone()).unwrap();
        assert!(req_str(0).starts_with("GET /version HTTP/1.1\r\n"));
        assert!(req_str(1).starts_with("GET /configs HTTP/1.1\r\n"));
        assert!(req_str(4).starts_with("DELETE /connections/conn%2Fid%3F1 HTTP/1.1\r\n"));
        assert!(req_str(5).starts_with("PUT /providers/proxies/Main%2FSub HTTP/1.1\r\n"));
        assert!(
            req_str(6).starts_with("GET /providers/proxies/Main%2FSub/healthcheck HTTP/1.1\r\n")
        );
        assert!(req_str(7).starts_with("PUT /providers/rules/Ads%20Rule HTTP/1.1\r\n"));
        for idx in 0..reqs.len() {
            assert!(req_str(idx).contains("Authorization: Bearer meta-secret\r\n"));
        }
    }

    #[test]
    fn enable_loopback_and_apply_saves_loopback_controller_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_config, "mode: direct\nmixed-port: 7890\n").unwrap();

        let first = enable_loopback_and_apply(&ctx).unwrap();
        assert!(
            first.starts_with("127.0.0.1:909"),
            "unexpected addr: {first}"
        );
        let settings = effective_settings(&ctx).unwrap().unwrap();
        assert!(settings.bind_loopback);
        assert_eq!(settings.bind_address, first);

        // Calling again keeps the already-configured address unchanged.
        let second = enable_loopback_and_apply(&ctx).unwrap();
        assert_eq!(second, first);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn configure_while_running_restarts_automatically_and_rolls_back_on_restart_failure() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
        fs::write(&ctx.paths.clash_config, "mode: direct\nmixed-port: 7890\n").unwrap();

        // Compile a tiny native ELF stub via `rustc` so `/proc/<pid>/cmdline` preserves
        // `argv[0] == clash_binary` (shebang scripts rewrite argv[0] to `/bin/sh`).
        let fail_sentinel = temp.path().join("fail-start");
        let stub_src = temp.path().join("mihomo_stub.rs");
        fs::write(
            &stub_src,
            format!(
                "use std::{{env, path::Path, process, thread, time::Duration}};\n\
                 fn main() {{\n\
                     if env::args().any(|a| a == \"-t\") {{ return; }}\n\
                     if Path::new({:?}).exists() {{ process::exit(1); }}\n\
                     loop {{ thread::sleep(Duration::from_millis(50)); }}\n\
                 }}\n",
                fail_sentinel
            ),
        )
        .unwrap();
        let compiled = Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&ctx.paths.clash_binary)
            .arg(&stub_src)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "failed to compile Mihomo stub: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );

        // Start running without any external-controller configured.
        start(&ctx).unwrap();
        let pid_before = match state(&ctx).unwrap() {
            State::Running(pid) => pid,
            other => panic!("expected Running, got {other:?}"),
        };
        assert!(effective_settings(&ctx).unwrap().is_none());

        // Configure controller while running: must succeed and restart automatically (new PID).
        configure(&ctx, "127.0.0.1:19098").unwrap();
        let pid_after = match state(&ctx).unwrap() {
            State::Running(pid) => pid,
            other => panic!("expected Running after auto-restart, got {other:?}"),
        };
        assert_ne!(pid_before, pid_after);
        let applied = effective_settings(&ctx).unwrap().unwrap();
        assert_eq!(applied.bind_address, "127.0.0.1:19098");

        // Now force startup failure on the next change and verify automatic file rollback.
        fs::write(&fail_sentinel, b"1").unwrap();
        let err = configure(&ctx, "127.0.0.1:19099").unwrap_err();
        assert!(
            err.detail.contains("previous configuration restored"),
            "unexpected error detail: {}",
            err.detail
        );
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["external-controller"], "127.0.0.1:19098");
        let runtime: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_runtime).unwrap()).unwrap();
        assert_eq!(runtime["external-controller"], "127.0.0.1:19098");
        assert!(!ctx.paths.clash_data.join("pending.json").exists());

        let _ = fs::remove_file(&fail_sentinel);
        let _ = stop(&ctx);
    }
}
