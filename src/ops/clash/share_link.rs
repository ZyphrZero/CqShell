//! Pure-Rust converter from Base64 or raw share-link subscription lists
//! (`ss://`, `vmess://`, `vless://`, `trojan://`, `hysteria2://`/`hy2://`, `tuic://`, `socks5://`)
//! into a standard Mihomo YAML configuration.

use base64::Engine as _;
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, HashSet};
use url::Url;

fn key(name: &str) -> Value {
    Value::String(name.into())
}

fn decode_base64_lenient(raw: &str) -> Option<Vec<u8>> {
    let compact: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    let engines = [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ];
    for engine in engines {
        if let Ok(bytes) = engine.decode(&compact) {
            return Some(bytes);
        }
    }
    // Retry after padding to a multiple of 4 bytes.
    let rem = compact.len() % 4;
    if rem != 0 {
        let mut padded = compact;
        for _ in 0..(4 - rem) {
            padded.push('=');
        }
        for engine in [
            &base64::engine::general_purpose::STANDARD,
            &base64::engine::general_purpose::URL_SAFE,
        ] {
            if let Ok(bytes) = engine.decode(&padded) {
                return Some(bytes);
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn query_map(url: &Url) -> BTreeMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.into_owned()))
        .collect()
}

fn bool_param(params: &BTreeMap<String, String>, names: &[&str]) -> bool {
    names.iter().any(|name| {
        params
            .get(*name)
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
    })
}

fn non_empty<'a>(params: &'a BTreeMap<String, String>, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| params.get(*name).map(String::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn clean_host(host: &str) -> String {
    host.trim_matches(['[', ']']).to_owned()
}

fn parse_ss(line: &str) -> Option<(String, Mapping)> {
    let rest = line.strip_prefix("ss://")?;
    let (without_frag, frag) = match rest.split_once('#') {
        Some((before, f)) => (before, percent_decode(f)),
        None => (rest, String::new()),
    };
    let (main_part, _query) = without_frag.split_once('?').unwrap_or((without_frag, ""));

    let (method, password, server, port) =
        if let Some((userinfo, hostport)) = main_part.split_once('@') {
            let decoded_info = if userinfo.contains(':') {
                percent_decode(userinfo)
            } else {
                let raw = decode_base64_lenient(&percent_decode(userinfo))?;
                String::from_utf8(raw).ok()?
            };
            let (method, password) = decoded_info.split_once(':')?;
            let parsed = Url::parse(&format!("http://{hostport}")).ok()?;
            let server = clean_host(parsed.host_str()?);
            let port = parsed.port()?;
            (method.to_owned(), password.to_owned(), server, port)
        } else {
            let raw = decode_base64_lenient(main_part)?;
            let decoded = String::from_utf8(raw).ok()?;
            let (userinfo, hostport) = decoded.rsplit_once('@')?;
            let (method, password) = userinfo.split_once(':')?;
            let parsed = Url::parse(&format!("http://{hostport}")).ok()?;
            let server = clean_host(parsed.host_str()?);
            let port = parsed.port()?;
            (method.to_owned(), password.to_owned(), server, port)
        };

    if method.is_empty() || password.is_empty() || server.is_empty() || port == 0 {
        return None;
    }
    let mut map = Mapping::new();
    map.insert(key("type"), key("ss"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("cipher"), key(&method));
    map.insert(key("password"), key(&password));
    map.insert(key("udp"), Value::Bool(true));
    let name = if frag.trim().is_empty() {
        format!("ss-{server}:{port}")
    } else {
        frag.trim().to_owned()
    };
    Some((name, map))
}

fn json_str_or_num(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn json_u16(v: Option<&serde_json::Value>) -> Option<u16> {
    match v? {
        serde_json::Value::Number(n) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
        serde_json::Value::String(s) => s.trim().parse::<u16>().ok(),
        _ => None,
    }
}

fn parse_vmess(line: &str) -> Option<(String, Mapping)> {
    let encoded = line.strip_prefix("vmess://")?.trim();
    let raw = decode_base64_lenient(encoded)?;
    let obj: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let server = json_str_or_num(obj.get("add"))?;
    let port = json_u16(obj.get("port")).filter(|&p| p > 0)?;
    let uuid = json_str_or_num(obj.get("id"))?;
    let alter_id = json_u16(obj.get("aid")).unwrap_or(0);
    let cipher = json_str_or_num(obj.get("scy"))
        .or_else(|| json_str_or_num(obj.get("cipher")))
        .unwrap_or_else(|| "auto".into());
    let net = json_str_or_num(obj.get("net"))
        .unwrap_or_else(|| "tcp".into())
        .to_ascii_lowercase();
    let tls = json_str_or_num(obj.get("tls")).is_some_and(|v| {
        v.eq_ignore_ascii_case("tls") || v == "1" || v.eq_ignore_ascii_case("true")
    });

    let mut map = Mapping::new();
    map.insert(key("type"), key("vmess"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("uuid"), key(&uuid));
    map.insert(key("alterId"), Value::Number(alter_id.into()));
    map.insert(key("cipher"), key(&cipher));
    map.insert(key("udp"), Value::Bool(true));
    if tls {
        map.insert(key("tls"), Value::Bool(true));
        if let Some(sni) =
            json_str_or_num(obj.get("sni")).or_else(|| json_str_or_num(obj.get("host")))
        {
            map.insert(key("servername"), key(&sni));
        }
        if let Some(fp) = json_str_or_num(obj.get("fp")) {
            map.insert(key("client-fingerprint"), key(&fp));
        }
    }
    if net == "ws" {
        map.insert(key("network"), key("ws"));
        let mut ws = Mapping::new();
        if let Some(path) = json_str_or_num(obj.get("path")) {
            ws.insert(key("path"), key(&path));
        }
        if let Some(host) = json_str_or_num(obj.get("host")) {
            let mut headers = Mapping::new();
            headers.insert(key("Host"), key(&host));
            ws.insert(key("headers"), Value::Mapping(headers));
        }
        if !ws.is_empty() {
            map.insert(key("ws-opts"), Value::Mapping(ws));
        }
    } else if net == "grpc" {
        map.insert(key("network"), key("grpc"));
        if let Some(path) = json_str_or_num(obj.get("path")) {
            let mut grpc = Mapping::new();
            grpc.insert(key("grpc-service-name"), key(&path));
            map.insert(key("grpc-opts"), Value::Mapping(grpc));
        }
    }
    let name = json_str_or_num(obj.get("ps")).unwrap_or_else(|| format!("vmess-{server}:{port}"));
    Some((name, map))
}

fn apply_common_transport(map: &mut Mapping, params: &BTreeMap<String, String>) {
    let network = non_empty(params, &["type", "net"])
        .unwrap_or("tcp")
        .to_ascii_lowercase();
    match network.as_str() {
        "ws" => {
            map.insert(key("network"), key("ws"));
            let mut ws = Mapping::new();
            if let Some(path) = non_empty(params, &["path"]) {
                ws.insert(key("path"), key(path));
            }
            if let Some(host) = non_empty(params, &["host"]) {
                let mut headers = Mapping::new();
                headers.insert(key("Host"), key(host));
                ws.insert(key("headers"), Value::Mapping(headers));
            }
            if !ws.is_empty() {
                map.insert(key("ws-opts"), Value::Mapping(ws));
            }
        }
        "grpc" => {
            map.insert(key("network"), key("grpc"));
            if let Some(svc) = non_empty(params, &["servicename", "path"]) {
                let mut grpc = Mapping::new();
                grpc.insert(key("grpc-service-name"), key(svc));
                map.insert(key("grpc-opts"), Value::Mapping(grpc));
            }
        }
        "h2" | "http" => {
            map.insert(key("network"), key("h2"));
            let mut h2 = Mapping::new();
            if let Some(path) = non_empty(params, &["path"]) {
                h2.insert(key("path"), key(path));
            }
            if let Some(host) = non_empty(params, &["host"]) {
                h2.insert(key("host"), Value::Sequence(vec![key(host)]));
            }
            if !h2.is_empty() {
                map.insert(key("h2-opts"), Value::Mapping(h2));
            }
        }
        _ => {}
    }
}

fn parse_vless(line: &str) -> Option<(String, Mapping)> {
    let url = Url::parse(line).ok()?;
    let uuid = percent_decode(url.username());
    let server = clean_host(url.host_str()?);
    let port = url.port().filter(|&p| p > 0)?;
    if uuid.is_empty() || server.is_empty() {
        return None;
    }
    let params = query_map(&url);
    let mut map = Mapping::new();
    map.insert(key("type"), key("vless"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("uuid"), key(&uuid));
    map.insert(key("udp"), Value::Bool(true));

    if let Some(flow) = non_empty(&params, &["flow"]) {
        map.insert(key("flow"), key(flow));
    }
    let security = non_empty(&params, &["security"])
        .unwrap_or("none")
        .to_ascii_lowercase();
    if matches!(security.as_str(), "tls" | "reality") {
        map.insert(key("tls"), Value::Bool(true));
        if let Some(sni) = non_empty(&params, &["sni", "peer"]) {
            map.insert(key("servername"), key(sni));
        }
        if let Some(fp) = non_empty(&params, &["fp"]) {
            map.insert(key("client-fingerprint"), key(fp));
        }
        if bool_param(&params, &["allowinsecure", "insecure"]) {
            map.insert(key("skip-cert-verify"), Value::Bool(true));
        }
        if let Some(alpn) = non_empty(&params, &["alpn"]) {
            let items: Vec<Value> = alpn
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(key)
                .collect();
            if !items.is_empty() {
                map.insert(key("alpn"), Value::Sequence(items));
            }
        }
        if security == "reality" {
            let mut reality = Mapping::new();
            if let Some(pbk) = non_empty(&params, &["pbk"]) {
                reality.insert(key("public-key"), key(pbk));
            }
            if let Some(sid) = non_empty(&params, &["sid"]) {
                reality.insert(key("short-id"), key(sid));
            }
            if !reality.is_empty() {
                map.insert(key("reality-opts"), Value::Mapping(reality));
            }
        }
    }
    apply_common_transport(&mut map, &params);
    let name = url
        .fragment()
        .map(percent_decode)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("vless-{server}:{port}"));
    Some((name, map))
}

fn parse_trojan(line: &str) -> Option<(String, Mapping)> {
    let url = Url::parse(line).ok()?;
    let password = percent_decode(url.username());
    let server = clean_host(url.host_str()?);
    let port = url.port().unwrap_or(443);
    if password.is_empty() || server.is_empty() || port == 0 {
        return None;
    }
    let params = query_map(&url);
    let mut map = Mapping::new();
    map.insert(key("type"), key("trojan"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("password"), key(&password));
    map.insert(key("udp"), Value::Bool(true));
    if let Some(sni) = non_empty(&params, &["sni", "peer"]) {
        map.insert(key("sni"), key(sni));
    }
    if bool_param(&params, &["allowinsecure", "insecure"]) {
        map.insert(key("skip-cert-verify"), Value::Bool(true));
    }
    if let Some(fp) = non_empty(&params, &["fp"]) {
        map.insert(key("client-fingerprint"), key(fp));
    }
    apply_common_transport(&mut map, &params);
    let name = url
        .fragment()
        .map(percent_decode)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("trojan-{server}:{port}"));
    Some((name, map))
}

fn parse_hysteria2(line: &str) -> Option<(String, Mapping)> {
    let normalized = if let Some(rest) = line.strip_prefix("hy2://") {
        format!("hysteria2://{rest}")
    } else {
        line.to_owned()
    };
    let url = Url::parse(&normalized).ok()?;
    let password = percent_decode(url.username());
    let server = clean_host(url.host_str()?);
    let port = url.port().unwrap_or(443);
    if password.is_empty() || server.is_empty() || port == 0 {
        return None;
    }
    let params = query_map(&url);
    let mut map = Mapping::new();
    map.insert(key("type"), key("hysteria2"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("password"), key(&password));
    if let Some(sni) = non_empty(&params, &["sni", "peer"]) {
        map.insert(key("sni"), key(sni));
    }
    if bool_param(&params, &["insecure", "allowinsecure"]) {
        map.insert(key("skip-cert-verify"), Value::Bool(true));
    }
    if let Some(obfs) = non_empty(&params, &["obfs"])
        && !obfs.eq_ignore_ascii_case("none")
    {
        map.insert(key("obfs"), key(obfs));
        if let Some(obfs_pw) = non_empty(&params, &["obfs-password"]) {
            map.insert(key("obfs-password"), key(obfs_pw));
        }
    }
    let name = url
        .fragment()
        .map(percent_decode)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("hy2-{server}:{port}"));
    Some((name, map))
}

fn parse_tuic(line: &str) -> Option<(String, Mapping)> {
    let url = Url::parse(line).ok()?;
    let uuid = percent_decode(url.username());
    let password = url.password().map(percent_decode).unwrap_or_default();
    let server = clean_host(url.host_str()?);
    let port = url.port().unwrap_or(443);
    if uuid.is_empty() || server.is_empty() || port == 0 {
        return None;
    }
    let params = query_map(&url);
    let mut map = Mapping::new();
    map.insert(key("type"), key("tuic"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("uuid"), key(&uuid));
    if !password.is_empty() {
        map.insert(key("password"), key(&password));
    }
    if let Some(sni) = non_empty(&params, &["sni"]) {
        map.insert(key("sni"), key(sni));
    }
    if let Some(cc) = non_empty(&params, &["congestion_control", "congestion-control"]) {
        map.insert(key("congestion-controller"), key(cc));
    }
    if let Some(mode) = non_empty(&params, &["udp_relay_mode", "udp-relay-mode"]) {
        map.insert(key("udp-relay-mode"), key(mode));
    }
    if bool_param(&params, &["allow_insecure", "allowinsecure", "insecure"]) {
        map.insert(key("skip-cert-verify"), Value::Bool(true));
    }
    let name = url
        .fragment()
        .map(percent_decode)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("tuic-{server}:{port}"));
    Some((name, map))
}

fn parse_socks5(line: &str) -> Option<(String, Mapping)> {
    let url = Url::parse(line).ok()?;
    let server = clean_host(url.host_str()?);
    let port = url.port().filter(|&p| p > 0)?;
    if server.is_empty() {
        return None;
    }
    let mut map = Mapping::new();
    map.insert(key("type"), key("socks5"));
    map.insert(key("server"), key(&server));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("udp"), Value::Bool(true));
    let user = percent_decode(url.username());
    if !user.is_empty() {
        map.insert(key("username"), key(&user));
    }
    if let Some(pass) = url.password().map(percent_decode)
        && !pass.is_empty()
    {
        map.insert(key("password"), key(&pass));
    }
    let name = url
        .fragment()
        .map(percent_decode)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("socks5-{server}:{port}"));
    Some((name, map))
}

fn parse_single_link(line: &str) -> Option<(String, Mapping)> {
    let trimmed = line.trim();
    if trimmed.starts_with("ss://") {
        parse_ss(trimmed)
    } else if trimmed.starts_with("vmess://") {
        parse_vmess(trimmed)
    } else if trimmed.starts_with("vless://") {
        parse_vless(trimmed)
    } else if trimmed.starts_with("trojan://") {
        parse_trojan(trimmed)
    } else if trimmed.starts_with("hysteria2://") || trimmed.starts_with("hy2://") {
        parse_hysteria2(trimmed)
    } else if trimmed.starts_with("tuic://") {
        parse_tuic(trimmed)
    } else if trimmed.starts_with("socks5://") || trimmed.starts_with("socks://") {
        parse_socks5(trimmed)
    } else {
        None
    }
}

fn has_supported_scheme(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        t.starts_with("ss://")
            || t.starts_with("vmess://")
            || t.starts_with("vless://")
            || t.starts_with("trojan://")
            || t.starts_with("hysteria2://")
            || t.starts_with("hy2://")
            || t.starts_with("tuic://")
            || t.starts_with("socks5://")
            || t.starts_with("socks://")
    })
}

/// Attempt to convert a raw or Base64-encoded share-link subscription body into a standard
/// Mihomo YAML document. Returns `None` if no valid proxy entries can be decoded.
pub(crate) fn try_convert_to_clash_yaml(raw: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(raw).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let decoded_owned;
    let source = if has_supported_scheme(text) {
        text
    } else {
        let bytes = decode_base64_lenient(text)?;
        decoded_owned = String::from_utf8(bytes).ok()?;
        if !has_supported_scheme(&decoded_owned) {
            return None;
        }
        decoded_owned.as_str()
    };

    let mut seen_names = HashSet::new();
    let mut proxies = Vec::new();
    let mut proxy_names = Vec::new();

    for line in source.lines() {
        let Some((base_name, mut map)) = parse_single_link(line) else {
            continue;
        };
        let mut unique_name = base_name.clone();
        let mut suffix = 2usize;
        while !seen_names.insert(unique_name.clone()) {
            unique_name = format!("{base_name}-{suffix}");
            suffix += 1;
        }
        map.insert(key("name"), key(&unique_name));
        proxy_names.push(unique_name);
        proxies.push(Value::Mapping(map));
    }

    if proxies.is_empty() {
        return None;
    }

    let mut selector = Mapping::new();
    selector.insert(key("name"), key("Proxy"));
    selector.insert(key("type"), key("select"));
    let mut sel_list = Vec::with_capacity(proxy_names.len() + 2);
    sel_list.push(key("Auto"));
    for name in &proxy_names {
        sel_list.push(key(name));
    }
    sel_list.push(key("DIRECT"));
    selector.insert(key("proxies"), Value::Sequence(sel_list));

    let mut auto = Mapping::new();
    auto.insert(key("name"), key("Auto"));
    auto.insert(key("type"), key("url-test"));
    auto.insert(key("url"), key(super::controller::DEFAULT_DELAY_URL));
    auto.insert(key("interval"), Value::Number(300.into()));
    auto.insert(key("tolerance"), Value::Number(50.into()));
    auto.insert(
        key("proxies"),
        Value::Sequence(proxy_names.iter().map(|n| key(n)).collect()),
    );

    let mut root = Mapping::new();
    root.insert(key("mixed-port"), Value::Number(7890.into()));
    root.insert(key("allow-lan"), Value::Bool(false));
    root.insert(key("mode"), key("rule"));
    root.insert(key("log-level"), key("info"));
    root.insert(key("proxies"), Value::Sequence(proxies));
    root.insert(
        key("proxy-groups"),
        Value::Sequence(vec![Value::Mapping(selector), Value::Mapping(auto)]),
    );
    root.insert(key("rules"), Value::Sequence(vec![key("MATCH,Proxy")]));

    serde_yaml::to_string(&Value::Mapping(root))
        .ok()
        .map(String::into_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_bundle_converts_all_common_protocols_and_preserves_reality_short_id() {
        let vmess_json = serde_json::json!({
            "v": "2",
            "ps": "VMess-Node",
            "add": "198.51.100.10",
            "port": "443",
            "id": "00000000-0000-4000-8000-000000000001",
            "aid": "0",
            "net": "ws",
            "host": "ws.example.com",
            "path": "/ws",
            "tls": "tls"
        })
        .to_string();
        let vmess_link = format!(
            "vmess://{}",
            base64::engine::general_purpose::STANDARD.encode(vmess_json)
        );
        let ss_userinfo =
            base64::engine::general_purpose::STANDARD.encode("aes-128-gcm:fixture-ss-pass");
        let lines = [
            format!("ss://{ss_userinfo}@192.0.2.10:8388#SS%20One"),
            vmess_link,
            "vless://00000000-0000-4000-8000-000000000002@203.0.113.10:443?security=reality&sni=example.com&fp=chrome&pbk=PubKeyDemo&sid=01234567&flow=xtls-rprx-vision#Reality-01".to_owned(),
            "trojan://fixture-trojan-pass@192.0.2.20:443?sni=trojan.example.com&allowInsecure=1#Trojan-Node".to_owned(),
            "hy2://fixture-hy2-pass@198.51.100.20:443?sni=hy2.example.com&obfs=salamander&obfs-password=obfs-pw#HY2-Node".to_owned(),
            "tuic://00000000-0000-4000-8000-000000000003:fixture-tuic-pass@203.0.113.20:443?sni=tuic.example.com&congestion_control=bbr#TUIC-Node".to_owned(),
        ]
        .join("\n");
        let b64_sub = base64::engine::general_purpose::STANDARD.encode(lines.as_bytes());
        let yaml_bytes = try_convert_to_clash_yaml(b64_sub.as_bytes()).expect("converted YAML");
        let parsed = super::super::yaml_prep::parse_yaml_safe(&yaml_bytes).unwrap();
        let proxies = parsed["proxies"].as_sequence().unwrap();
        assert_eq!(proxies.len(), 6);
        assert_eq!(proxies[0]["type"], "ss");
        assert_eq!(proxies[0]["name"], "SS One");
        assert_eq!(proxies[1]["type"], "vmess");
        assert_eq!(proxies[1]["ws-opts"]["path"], "/ws");
        assert_eq!(proxies[2]["type"], "vless");
        assert_eq!(proxies[2]["reality-opts"]["short-id"], "01234567");
        assert_eq!(proxies[3]["type"], "trojan");
        assert_eq!(proxies[3]["skip-cert-verify"], true);
        assert_eq!(proxies[4]["type"], "hysteria2");
        assert_eq!(proxies[4]["obfs"], "salamander");
        assert_eq!(proxies[5]["type"], "tuic");
        assert_eq!(proxies[5]["congestion-controller"], "bbr");
        assert_eq!(parsed["proxy-groups"][0]["name"], "Proxy");
        assert_eq!(parsed["rules"][0], "MATCH,Proxy");
    }

    #[test]
    fn unparseable_placeholder_share_links_return_none() {
        // `dm1lc3M6Ly9leGFtcGxl` decodes to `vmess://example` (not valid JSON).
        assert!(try_convert_to_clash_yaml(b"dm1lc3M6Ly9leGFtcGxl\n").is_none());
        assert!(try_convert_to_clash_yaml(b"plain text not a link").is_none());
    }
}
