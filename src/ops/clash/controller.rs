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

pub(super) struct Settings {
    pub address: String,
    pub url: String,
    socket: SocketAddr,
    secret: String,
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
    let socket = socket_address(raw)?;
    let address = socket.to_string();
    Ok(Some(Settings {
        url: format!("http://{address}"),
        address,
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

pub(super) fn settings(ctx: &Context) -> Result<Settings> {
    from_config(&effective_config(ctx)?)?
        .ok_or_else(|| invalid("external-controller is not configured"))
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
    edit_mixin(ctx, |map| {
        map.insert(
            Value::String("external-controller".into()),
            Value::String(address.into()),
        );
        validate_override(ctx, map)
    })?;
    ctx.message(
        "notice.clash_controller",
        "configuration saved; start Mihomo to apply",
    );
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
    edit_mixin(ctx, |map| {
        map.insert(Value::String("secret".into()), Value::String(secret.into()));
        validate_override(ctx, map)
    })?;
    ctx.message(
        "notice.clash_secret",
        "configuration saved; start Mihomo to apply",
    );
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
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
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
        if start.elapsed() > REQUEST_TIMEOUT {
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
}
