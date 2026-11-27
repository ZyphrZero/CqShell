//! Safe YAML preprocessing, merge-key expansion, and subscription sanitization
//! for Mihomo runtime configurations.

use super::{sha256_hex, socks};
use crate::error::{Error, Result};
use serde_yaml::{Mapping, Value};
use std::{
    borrow::Cow,
    net::IpAddr,
    path::{Path, PathBuf},
};

const MAX_YAML_BYTES: usize = 32 * 1024 * 1024;
const MAX_MERGE_NODES: usize = 250_000;

/// Private / link-local / CGNAT ranges excluded from TUN routing by default on Linux servers
/// so management interfaces and local services never loop back into the tunnel.
pub(crate) const PRIVATE_ROUTE_CIDRS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
];

fn key(name: &str) -> Value {
    Value::String(name.into())
}

fn needs_short_id_quoting(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let first = token.as_bytes()[0];
    if matches!(
        first,
        b'\'' | b'"' | b'[' | b'{' | b'|' | b'>' | b'&' | b'*' | b'!' | b'#'
    ) {
        return false;
    }
    if token.eq_ignore_ascii_case("null") || token == "~" {
        return false;
    }
    // REALITY short-id values are hex strings; YAML 1.1/1.2 may parse unquoted digits,
    // leading-zero octals, hex literals, scientific notation (`12e4`), or sexagesimal as numbers.
    token.bytes().all(|b| {
        b.is_ascii_hexdigit() || matches!(b, b'x' | b'X' | b'o' | b'O' | b'_' | b'+' | b'-' | b'.')
    }) && token.bytes().any(|b| b.is_ascii_digit())
}

fn quote_flow_sequence_items(seq_inner: &str) -> String {
    let mut out = String::with_capacity(seq_inner.len() + 8);
    for (idx, part) in seq_inner.split(',').enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let leading = part.len() - part.trim_start().len();
        let trimmed_end = part.trim_end();
        let trailing_len = part.len() - trimmed_end.len();
        let core = trimmed_end.trim_start();
        out.push_str(&part[..leading]);
        if needs_short_id_quoting(core) {
            out.push('\'');
            out.push_str(core);
            out.push('\'');
        } else {
            out.push_str(core);
        }
        out.push_str(&part[part.len() - trailing_len..]);
    }
    out
}

/// Split an inline comment (` #...` or `#...` outside quotes) from a YAML value fragment.
fn split_yaml_comment(value: &str) -> (&str, &str) {
    let bytes = value.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => {
                if i == 0 || bytes[i - 1] != b'\\' {
                    in_double = !in_double;
                }
            }
            b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return (&value[..i], &value[i..]);
            }
            _ => {}
        }
        i += 1;
    }
    (value, "")
}

/// Rewrite inline flow occurrences of `short-id: <unquoted-numeric>` on a single YAML line.
fn rewrite_inline_short_ids(line: &str) -> String {
    let (code, comment) = split_yaml_comment(line);
    let mut out = String::with_capacity(line.len() + 8);
    let bytes = code.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i < bytes.len() {
        if !in_single && !in_double {
            // Check for `short-id`, `'short-id'`, or `"short-id"` followed by optional spaces + `:`.
            let key_len = if code[i..].starts_with("short-id") {
                Some(8usize)
            } else if code[i..].starts_with("'short-id'") || code[i..].starts_with("\"short-id\"") {
                Some(10usize)
            } else {
                None
            };
            let boundary_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'{' | b',' | b'-' | b'[' | b'\n'
                );
            if let Some(klen) = key_len
                && boundary_ok
            {
                let after_key = i + klen;
                let mut colon = after_key;
                while colon < bytes.len() && matches!(bytes[colon], b' ' | b'\t') {
                    colon += 1;
                }
                if colon < bytes.len() && bytes[colon] == b':' {
                    let after_colon = colon + 1;
                    // Must be followed by whitespace or EOL to be a YAML mapping key.
                    if after_colon == bytes.len() || bytes[after_colon].is_ascii_whitespace() {
                        out.push_str(&code[i..after_colon]);
                        let mut v_start = after_colon;
                        while v_start < bytes.len() && matches!(bytes[v_start], b' ' | b'\t') {
                            out.push(bytes[v_start] as char);
                            v_start += 1;
                        }
                        if v_start < bytes.len() && bytes[v_start] == b'[' {
                            if let Some(rel_end) = code[v_start + 1..].find(']') {
                                let seq_end = v_start + 1 + rel_end;
                                out.push('[');
                                out.push_str(&quote_flow_sequence_items(
                                    &code[v_start + 1..seq_end],
                                ));
                                out.push(']');
                                i = seq_end + 1;
                                continue;
                            }
                        } else {
                            let mut v_end = v_start;
                            while v_end < bytes.len() && !matches!(bytes[v_end], b',' | b'}' | b']')
                            {
                                v_end += 1;
                            }
                            let raw_val = &code[v_start..v_end];
                            let trimmed = raw_val.trim_end();
                            let trailing = &raw_val[trimmed.len()..];
                            if needs_short_id_quoting(trimmed) {
                                out.push('\'');
                                out.push_str(trimmed);
                                out.push('\'');
                                out.push_str(trailing);
                            } else {
                                out.push_str(raw_val);
                            }
                            i = v_end;
                            continue;
                        }
                    }
                }
            }
        }

        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single && (i == 0 || bytes[i - 1] != b'\\') => in_double = !in_double,
            _ => {}
        }
        let ch = code[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.push_str(comment);
    out
}

/// Protect unquoted VLESS/REALITY `short-id` scalars from being coerced into numbers by
/// `serde_yaml`, which would otherwise drop leading zeros (`0123` -> `123`, `00` -> `0`).
pub(crate) fn quote_unquoted_short_ids(raw: &str) -> Cow<'_, str> {
    if !raw.contains("short-id") {
        return Cow::Borrowed(raw);
    }
    let mut out = String::with_capacity(raw.len() + 32);
    let mut changed = false;
    let mut block_seq_indent: Option<usize> = None;

    for segment in raw.split_inclusive('\n') {
        let (line, nl) = if let Some(stripped) = segment.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = segment.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (segment, "")
        };

        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        // Check block-sequence items directly under `short-id:` (`  - 01234567`).
        if let Some(parent_indent) = block_seq_indent {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                out.push_str(line);
                out.push_str(nl);
                continue;
            }
            if indent >= parent_indent && trimmed.starts_with("- ") {
                let (code, comment) = split_yaml_comment(&trimmed[2..]);
                let leading_ws = code.len() - code.trim_start().len();
                let core = code.trim();
                if needs_short_id_quoting(core) {
                    let trailing_ws = code.len() - code.trim_end().len();
                    out.push_str(&line[..indent]);
                    out.push_str("- ");
                    out.push_str(&code[..leading_ws]);
                    out.push('\'');
                    out.push_str(core);
                    out.push('\'');
                    out.push_str(&code[code.len() - trailing_ws..]);
                    out.push_str(comment);
                    out.push_str(nl);
                    changed = true;
                    continue;
                }
            } else {
                block_seq_indent = None;
            }
        }

        if line.contains("short-id") {
            let (code, _) = split_yaml_comment(trimmed);
            let bare_block = code
                .strip_prefix("short-id")
                .or_else(|| code.strip_prefix("'short-id'"))
                .or_else(|| code.strip_prefix("\"short-id\""))
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| rest.trim().is_empty());
            if bare_block {
                block_seq_indent = Some(indent);
                out.push_str(line);
                out.push_str(nl);
                continue;
            }
            let rewritten = rewrite_inline_short_ids(line);
            if rewritten != line {
                changed = true;
            }
            out.push_str(&rewritten);
            out.push_str(nl);
        } else {
            out.push_str(line);
            out.push_str(nl);
        }
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(raw)
    }
}

fn count_yaml_nodes(value: &Value, count: &mut usize) -> Result<()> {
    *count += 1;
    if *count > MAX_MERGE_NODES {
        return Err(Error::new(
            "error.data",
            "YAML configuration exceeds node complexity limit",
        ));
    }
    match value {
        Value::Sequence(items) => {
            for item in items {
                count_yaml_nodes(item, count)?;
            }
        }
        Value::Mapping(map) => {
            for (k, v) in map {
                count_yaml_nodes(k, count)?;
                count_yaml_nodes(v, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Parse Mihomo YAML while preserving unquoted REALITY `short-id` literals and expanding
/// YAML merge keys (`<<: *anchor`) with bounded complexity.
pub(crate) fn parse_yaml_safe(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_YAML_BYTES {
        return Err(Error::new("error.data", "YAML configuration exceeds limit"));
    }
    let mut value: Value = match std::str::from_utf8(bytes) {
        Ok(text) => {
            let prepared = quote_unquoted_short_ids(text);
            serde_yaml::from_str(&prepared)
                .map_err(|_| Error::new("error.data", "invalid Mihomo configuration YAML"))?
        }
        Err(_) => serde_yaml::from_slice(bytes)
            .map_err(|_| Error::new("error.data", "invalid Mihomo configuration YAML"))?,
    };
    value
        .apply_merge()
        .map_err(|_| Error::new("error.data", "invalid YAML merge key in configuration"))?;
    let mut nodes = 0usize;
    count_yaml_nodes(&value, &mut nodes)?;
    Ok(value)
}

pub(crate) fn has_valid_authentication(map: &Mapping) -> bool {
    let Some(items) = map.get(key("authentication")).and_then(Value::as_sequence) else {
        return false;
    };
    !items.is_empty()
        && items.iter().all(|item| {
            item.as_str().is_some_and(|entry| {
                let Some((user, pass)) = entry.split_once(':') else {
                    return false;
                };
                !user.is_empty()
                    && !pass.is_empty()
                    && user.bytes().all(|b| (0x21..=0x7e).contains(&b))
                    && pass.bytes().all(|b| (0x21..=0x7e).contains(&b))
            })
        })
}

fn bind_address_is_loopback(map: &Mapping) -> bool {
    let raw = map
        .get(key("bind-address"))
        .and_then(Value::as_str)
        .unwrap_or("*")
        .trim();
    if raw.is_empty() || raw == "*" {
        return false;
    }
    let candidate = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw);
    candidate
        .parse::<IpAddr>()
        .is_ok_and(|ip| socks::is_loopback_ip(&ip))
}

fn mixin_sets_non_empty(mixin: &Mapping, name: &str) -> bool {
    mixin
        .get(key(name))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

fn mixin_overrides_provider_path(mixin: &Mapping, section: &str, provider_name: &str) -> bool {
    mixin
        .get(key(section))
        .and_then(Value::as_mapping)
        .and_then(|sec| sec.get(key(provider_name)))
        .and_then(Value::as_mapping)
        .and_then(|p| p.get(key("path")))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

fn confine_providers_section(
    root: &mut Mapping,
    mixin: &Mapping,
    section: &str,
    subdir: &str,
    profile_scope: &str,
    providers_root: &Path,
) {
    let Some(providers) = root.get_mut(key(section)).and_then(Value::as_mapping_mut) else {
        return;
    };
    for (name_val, entry_val) in providers.iter_mut() {
        let Some(name) = name_val.as_str() else {
            continue;
        };
        let Some(entry) = entry_val.as_mapping_mut() else {
            continue;
        };
        let is_inline = entry
            .get(key("type"))
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("inline"));
        if is_inline || mixin_overrides_provider_path(mixin, section, name) {
            continue;
        }
        let url = entry
            .get(key("url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let hash_input = if !url.is_empty() {
            format!("{name}@{url}")
        } else {
            format!("{section}/{name}")
        };
        let digest = sha256_hex(hash_input.as_bytes());
        let ext = match entry
            .get(key("format"))
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("mrs") => "mrs",
            Some("text") => "txt",
            _ => "yaml",
        };
        let target: PathBuf = providers_root
            .join(profile_scope)
            .join(subdir)
            .join(format!("{}.{ext}", &digest[..24]));
        if let Some(path_str) = target.to_str() {
            entry.insert(key("path"), key(path_str));
        }
    }
}

/// Sanitize a merged runtime configuration against unsafe subscription fields, open LAN proxies,
/// cross-profile provider path collisions, and broken Linux TUN/DNS defaults.
pub(crate) fn sanitize_runtime(
    runtime: &mut Value,
    mixin: &Value,
    profile_key: Option<&str>,
    providers_root: Option<&Path>,
) -> Result<()> {
    let root = runtime
        .as_mapping_mut()
        .ok_or_else(|| Error::new("error.data", "Clash configuration root must be a map"))?;
    let empty_mixin = Mapping::new();
    let mixin_map = mixin.as_mapping().unwrap_or(&empty_mixin);

    // 1. Strip unauthenticated alternate controllers & remote UI download URLs from subscriptions.
    for dangerous in [
        "external-controller-unix",
        "external-controller-pipe",
        "external-ui-url",
    ] {
        if !mixin_sets_non_empty(mixin_map, dangerous) {
            root.remove(key(dangerous));
        }
    }

    // 2. Neutralize `iptables` command argument splitting if `inbound-interface` contains spaces.
    if let Some(iptables) = root
        .get_mut(key("iptables"))
        .and_then(Value::as_mapping_mut)
        && iptables
            .get(key("inbound-interface"))
            .and_then(Value::as_str)
            .is_some_and(|iface| iface.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        iptables.insert(key("enable"), Value::Bool(false));
    }

    // 3. Clear subscription-provided `interface-name` unless Mixin explicitly sets one,
    //    preventing hardcoded NIC names (e.g. `eth0`) from breaking servers with `ens3`/`enp*`.
    if !mixin_map.contains_key(key("interface-name")) {
        root.remove(key("interface-name"));
    }

    // 4. Confine non-inline proxy/rule provider cache files under `<clash_data>/providers/<scope>`.
    if let Some(root_dir) = providers_root {
        let scope = match profile_key.map(str::trim).filter(|s| !s.is_empty()) {
            Some(name) => sha256_hex(name.as_bytes())[..16].to_owned(),
            None => "default".to_owned(),
        };
        confine_providers_section(
            root,
            mixin_map,
            "proxy-providers",
            "proxies",
            &scope,
            root_dir,
        );
        confine_providers_section(root, mixin_map, "rule-providers", "rules", &scope, root_dir);
    }

    // 5. Strip untrusted subscription `skip-auth-prefixes` unless Mixin explicitly sets it,
    //    and protect non-loopback `allow-lan` listeners.
    if !mixin_map.contains_key(key("skip-auth-prefixes")) {
        root.remove(key("skip-auth-prefixes"));
    }
    let has_auth = has_valid_authentication(root);
    let allow_lan = root
        .get(key("allow-lan"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if allow_lan && !bind_address_is_loopback(root) && !has_auth {
        let mixin_requested_lan = mixin_map
            .get(key("allow-lan"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if mixin_requested_lan {
            return Err(Error::new(
                "error.data",
                "allow-lan on non-loopback addresses requires authentication",
            ));
        }
        // Subscription shipped `allow-lan: true` without authentication: clamp safely to loopback.
        root.insert(key("allow-lan"), Value::Bool(false));
        root.insert(key("bind-address"), key("127.0.0.1"));
    }

    // 6. Enforce safe Linux TUN defaults (`auto-detect-interface` + baseline DNS) when TUN is on.
    let tun_enabled = root
        .get(key("tun"))
        .and_then(Value::as_mapping)
        .and_then(|t| t.get(key("enable")))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if tun_enabled {
        let has_iface = root
            .get(key("interface-name"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if let Some(tun) = root.get_mut(key("tun")).and_then(Value::as_mapping_mut)
            && !has_iface
            && !tun.contains_key(key("auto-detect-interface"))
        {
            tun.insert(key("auto-detect-interface"), Value::Bool(true));
        }
        ensure_baseline_dns(root);
    }

    // 7. Preserve manual proxy-group selections across restarts/reloads by default.
    let mixin_sets_store = mixin_map
        .get(key("profile"))
        .and_then(Value::as_mapping)
        .is_some_and(|p| p.contains_key(key("store-selected")));
    if !mixin_sets_store {
        match root.get_mut(key("profile")) {
            Some(Value::Mapping(profile_map)) => {
                profile_map
                    .entry(key("store-selected"))
                    .or_insert(Value::Bool(true));
            }
            None | Some(Value::Null) => {
                let mut profile_map = Mapping::new();
                profile_map.insert(key("store-selected"), Value::Bool(true));
                root.insert(key("profile"), Value::Mapping(profile_map));
            }
            Some(_) => {}
        }
    }

    Ok(())
}

/// Ensure a minimal working DNS block exists whenever TUN is active so Mihomo never fails every
/// TUN-hijacked DNS query.
pub(crate) fn ensure_baseline_dns(root: &mut Mapping) {
    let entry = root
        .entry(key("dns"))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !entry.is_mapping() {
        *entry = Value::Mapping(Mapping::new());
    }
    let Some(dns) = entry.as_mapping_mut() else {
        return;
    };
    let enabled = dns
        .get(key("enable"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        dns.insert(key("enable"), Value::Bool(true));
    }
    if dns
        .get(key("enhanced-mode"))
        .and_then(Value::as_str)
        .is_none_or(|s| s.trim().is_empty())
    {
        dns.insert(key("enhanced-mode"), key("fake-ip"));
    }
    if dns
        .get(key("enhanced-mode"))
        .and_then(Value::as_str)
        .is_some_and(|m| m.eq_ignore_ascii_case("fake-ip"))
        && !dns.contains_key(key("fake-ip-range"))
    {
        dns.insert(key("fake-ip-range"), key("198.18.0.1/16"));
    }
    let has_ns = dns
        .get(key("nameserver"))
        .and_then(Value::as_sequence)
        .is_some_and(|items| !items.is_empty());
    if !has_ns {
        dns.insert(
            key("nameserver"),
            Value::Sequence(vec![key("223.5.5.5"), key("1.1.1.1")]),
        );
    }
}

/// Extract the current SSH client source CIDR (`/32` or `/128`) from `SSH_CONNECTION` so Linux
/// TUN `auto-route` can exclude management SSH sessions automatically.
pub(crate) fn current_ssh_client_cidr(
    env_map: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let raw = env_map
        .get("SSH_CONNECTION")
        .cloned()
        .or_else(|| std::env::var("SSH_CONNECTION").ok())?;
    let client_ip = raw.split_whitespace().next()?.trim();
    let parsed: IpAddr = client_ip.parse().ok()?;
    if socks::is_loopback_ip(&parsed) {
        return None;
    }
    match parsed {
        IpAddr::V4(v4) => Some(format!("{v4}/32")),
        IpAddr::V6(v6) => Some(format!("{v6}/128")),
    }
}

/// Generate a CSPRNG hexadecimal secret (Unix `/dev/urandom`; deterministic fallback in non-Unix unit tests).
#[cfg(any(unix, test))]
pub(crate) fn generate_hex_secret(num_bytes: usize) -> Result<String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut buf = vec![0u8; num_bytes];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
        Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(not(unix))]
    {
        Ok("a5".repeat(num_bytes))
    }
}

/// Generate a 24-character CSPRNG alphanumeric password without modulo bias.
pub(crate) fn generate_alnum_password() -> Result<String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        const ALPHABET: &[u8; 62] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut out = String::with_capacity(24);
        let mut urandom = std::fs::File::open("/dev/urandom")?;
        let mut buf = [0u8; 64];
        while out.len() < 24 {
            urandom.read_exact(&mut buf)?;
            for &b in &buf {
                if b < 248 {
                    out.push(ALPHABET[usize::from(b % 62)] as char);
                    if out.len() == 24 {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
    #[cfg(all(not(unix), test))]
    {
        Ok("TestOnlyAutoPass12345678".into())
    }
    #[cfg(all(not(unix), not(test)))]
    {
        Err(Error::new(
            "error.platform",
            "password generation requires Unix",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_unquoted_short_ids_preserves_leading_zeros_and_flow_mappings() {
        let yaml_src = "\
proxies:
  - name: reality-block
    type: vless
    reality-opts:
      public-key: pubkey
      short-id: 01234567 # keep zeros
  - name: reality-zero
    type: vless
    reality-opts:
      short-id: 00
  - name: reality-sci
    type: vless
    reality-opts:
      short-id: 12e4
  - {name: reality-flow, type: vless, reality-opts: {short-id: 00ab, other: 123}}
  - name: reality-list
    type: vless
    reality-opts:
      short-id:
        - 0011
        - 'already'
";
        let parsed = parse_yaml_safe(yaml_src.as_bytes()).unwrap();
        let proxies = parsed["proxies"].as_sequence().unwrap();
        assert_eq!(proxies[0]["reality-opts"]["short-id"], "01234567");
        assert_eq!(proxies[1]["reality-opts"]["short-id"], "00");
        assert_eq!(proxies[2]["reality-opts"]["short-id"], "12e4");
        assert_eq!(proxies[3]["reality-opts"]["short-id"], "00ab");
        assert_eq!(proxies[3]["reality-opts"]["other"], 123);
        assert_eq!(proxies[4]["reality-opts"]["short-id"][0], "0011");
        assert_eq!(proxies[4]["reality-opts"]["short-id"][1], "already");
    }

    #[test]
    fn parse_yaml_safe_expands_yaml_merge_keys() {
        let yaml_src = "\
p: &base
  type: ss
  cipher: aes-128-gcm
  password: fixture-password
proxies:
  - <<: *base
    name: node-a
    server: 192.0.2.10
    port: 443
";
        let parsed = parse_yaml_safe(yaml_src.as_bytes()).unwrap();
        let node = &parsed["proxies"][0];
        assert_eq!(node["name"], "node-a");
        assert_eq!(node["type"], "ss");
        assert_eq!(node["cipher"], "aes-128-gcm");
        assert_eq!(node["server"], "192.0.2.10");
    }

    #[test]
    fn sanitize_runtime_hardens_subscription_fields_and_confines_providers() {
        let base = parse_yaml_safe(
            br#"
mode: rule
allow-lan: true
interface-name: eth0
external-controller-unix: /tmp/evil.sock
external-controller-pipe: \\.\pipe\evil
external-ui-url: https://evil.invalid/ui.zip
iptables:
  enable: true
  inbound-interface: "eth0; rm -rf /"
skip-auth-prefixes:
  - 0.0.0.0/0
proxy-providers:
  subA:
    type: http
    url: https://example.com/p.yaml
    path: /etc/shadow
rule-providers:
  rulesA:
    type: http
    format: mrs
    url: https://example.com/r.mrs
    path: ../../escape.mrs
tun:
  enable: true
"#,
        )
        .unwrap();
        let mixin = Value::Mapping(Mapping::new());
        let mut runtime = base;
        let temp = tempfile::tempdir().unwrap();
        let providers_root = temp.path().join("providers");
        sanitize_runtime(&mut runtime, &mixin, Some("alpha"), Some(&providers_root)).unwrap();

        assert!(runtime.get("external-controller-unix").is_none());
        assert!(runtime.get("external-controller-pipe").is_none());
        assert!(runtime.get("external-ui-url").is_none());
        assert!(runtime.get("interface-name").is_none());
        assert_eq!(runtime["iptables"]["enable"], false);
        // Unauthenticated subscription `allow-lan: true` is clamped to loopback.
        assert_eq!(runtime["allow-lan"], false);
        assert_eq!(runtime["bind-address"], "127.0.0.1");
        // TUN gets auto-detect-interface + baseline DNS.
        assert_eq!(runtime["tun"]["auto-detect-interface"], true);
        assert_eq!(runtime["dns"]["enable"], true);
        assert_eq!(runtime["dns"]["enhanced-mode"], "fake-ip");
        assert_eq!(runtime["profile"]["store-selected"], true);
        // Providers confined into isolated directory with format extension preserved.
        let pp_path = runtime["proxy-providers"]["subA"]["path"].as_str().unwrap();
        let rp_path = runtime["rule-providers"]["rulesA"]["path"]
            .as_str()
            .unwrap();
        assert!(pp_path.starts_with(providers_root.to_str().unwrap()));
        assert!(pp_path.ends_with(".yaml"));
        assert!(rp_path.starts_with(providers_root.to_str().unwrap()));
        assert!(rp_path.ends_with(".mrs"));
    }

    #[test]
    fn sanitize_runtime_rejects_mixin_allow_lan_without_auth_and_clears_skip_auth() {
        let mut runtime =
            parse_yaml_safe(b"mode: direct\nallow-lan: true\nbind-address: '*'\n").unwrap();
        let mixin = parse_yaml_safe(b"allow-lan: true\n").unwrap();
        assert!(sanitize_runtime(&mut runtime, &mixin, None, None).is_err());

        let mut authed = parse_yaml_safe(
            b"mode: direct\nallow-lan: true\nbind-address: '*'\nauthentication:\n  - cqs:SecretPass123\nskip-auth-prefixes:\n  - 127.0.0.1/32\n",
        )
        .unwrap();
        sanitize_runtime(&mut authed, &mixin, None, None).unwrap();
        assert_eq!(authed["allow-lan"], true);
        assert!(authed.get("skip-auth-prefixes").is_none());
    }

    #[test]
    fn ssh_client_cidr_extracts_v4_and_v6_and_skips_loopback() {
        let mut env = std::collections::BTreeMap::new();
        env.insert(
            "SSH_CONNECTION".into(),
            "203.0.113.5 54321 198.51.100.1 22".into(),
        );
        assert_eq!(
            current_ssh_client_cidr(&env).as_deref(),
            Some("203.0.113.5/32")
        );

        env.insert(
            "SSH_CONNECTION".into(),
            "2001:db8::9 54321 2001:db8::1 22".into(),
        );
        assert_eq!(
            current_ssh_client_cidr(&env).as_deref(),
            Some("2001:db8::9/128")
        );

        env.insert(
            "SSH_CONNECTION".into(),
            "127.0.0.1 54321 127.0.0.1 22".into(),
        );
        assert_eq!(current_ssh_client_cidr(&env), None);
    }
}
