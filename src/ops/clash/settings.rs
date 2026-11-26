//! Structured Mihomo core settings, DNS presets, and Mixin management.

use super::*;
use serde_yaml::Mapping;

const VALID_LOG_LEVELS: &[&str] = &["info", "warning", "error", "debug", "silent"];

fn key(name: &str) -> Value {
    Value::String(name.into())
}

fn string_list(items: &[&str]) -> Value {
    Value::Sequence(items.iter().map(|item| key(item)).collect())
}

pub(crate) fn validate_mixed_port(value: &str) -> Result<u16> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::new(
            "error.input",
            "mixed port must be between 1 and 65535",
        ));
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| Error::new("error.input", "mixed port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(Error::new(
            "error.input",
            "mixed port must be between 1 and 65535",
        ));
    }
    Ok(port)
}

fn validate_log_level(value: &str) -> Result<()> {
    if !VALID_LOG_LEVELS.contains(&value) {
        return Err(Error::new(
            "error.input",
            "log level must be info, warning, error, debug or silent",
        ));
    }
    Ok(())
}

fn sniffer_mapping() -> Mapping {
    let mut http = Mapping::new();
    http.insert(
        key("ports"),
        Value::Sequence(vec![Value::Number(80.into()), key("8080-8880")]),
    );
    http.insert(key("override-destination"), Value::Bool(true));

    let mut tls = Mapping::new();
    tls.insert(
        key("ports"),
        Value::Sequence(vec![Value::Number(443.into()), Value::Number(8443.into())]),
    );

    let mut quic = Mapping::new();
    quic.insert(
        key("ports"),
        Value::Sequence(vec![Value::Number(443.into()), Value::Number(8443.into())]),
    );

    let mut sniff = Mapping::new();
    sniff.insert(key("HTTP"), Value::Mapping(http));
    sniff.insert(key("TLS"), Value::Mapping(tls));
    sniff.insert(key("QUIC"), Value::Mapping(quic));

    let mut root = Mapping::new();
    root.insert(key("enable"), Value::Bool(true));
    root.insert(key("parse-pure-ip"), Value::Bool(true));
    root.insert(key("override-destination"), Value::Bool(true));
    root.insert(key("sniff"), Value::Mapping(sniff));
    root.insert(
        key("skip-domain"),
        string_list(&["Mijia Cloud", "+.push.apple.com"]),
    );
    root
}

/// Persist core Mihomo listener, IPv6, domain sniffing, and log-level settings into Mixin.
pub fn configure(
    ctx: &Context,
    mixed_port: &str,
    allow_lan: bool,
    ipv6: bool,
    sniffer: bool,
    log_level: &str,
) -> Result<()> {
    let port = validate_mixed_port(mixed_port)?;
    validate_log_level(log_level)?;
    let applied = edit_mixin_and_apply(ctx, MixinApplyMode::HotReloadOrRestart, |map| {
        map.insert(key("mixed-port"), Value::Number(port.into()));
        map.insert(key("allow-lan"), Value::Bool(allow_lan));
        map.insert(
            key("bind-address"),
            key(if allow_lan { "*" } else { "127.0.0.1" }),
        );
        map.insert(key("ipv6"), Value::Bool(ipv6));
        map.insert(key("log-level"), key(log_level));
        if sniffer {
            map.insert(key("sniffer"), Value::Mapping(sniffer_mapping()));
        } else {
            let entry = map
                .entry(key("sniffer"))
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            match entry.as_mapping_mut() {
                Some(sniffer_map) => {
                    sniffer_map.insert(key("enable"), Value::Bool(false));
                }
                None => {
                    let mut disabled = Mapping::new();
                    disabled.insert(key("enable"), Value::Bool(false));
                    *entry = Value::Mapping(disabled);
                }
            }
        }
        Ok(())
    })?;
    ctx.message(
        "notice.clash_settings",
        format!(
            "mixed-port={port}; allow-lan={allow_lan}; ipv6={ipv6}; sniffer={sniffer}; log-level={log_level}; {}",
            applied.suffix()
        ),
    );
    Ok(())
}

fn effective_yaml(ctx: &Context) -> Result<Value> {
    let bytes = if matches!(state(ctx)?, State::Running(_)) {
        storage::check_path(&ctx.paths.clash_runtime, true)?;
        fs::read(&ctx.paths.clash_runtime)?
    } else {
        build_runtime_config(ctx)?
    };
    yaml(&bytes)
}

/// Summarize the effective Mihomo DNS configuration.
pub fn dns_status(ctx: &Context) -> Result<()> {
    let config = effective_yaml(ctx)?;
    let root = config
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Clash configuration root must be a map"))?;
    let detail = match root.get(key("dns")).and_then(Value::as_mapping) {
        None => "enabled=false (default system resolver)".into(),
        Some(dns) => {
            let enabled = dns
                .get(key("enable"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mode = dns
                .get(key("enhanced-mode"))
                .and_then(Value::as_str)
                .unwrap_or("normal");
            let ipv6 = dns
                .get(key("ipv6"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let nameservers = dns
                .get(key("nameserver"))
                .and_then(Value::as_sequence)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "none".into());
            let fallback_count = dns
                .get(key("fallback"))
                .and_then(Value::as_sequence)
                .map(Vec::len)
                .unwrap_or(0);
            format!(
                "enabled={enabled}; mode={mode}; ipv6={ipv6}; nameserver=[{nameservers}]; fallback={fallback_count}"
            )
        }
    };
    ctx.message("notice.clash_dns", detail);
    Ok(())
}

fn dns_mapping(mode: &str, preset: &str, ipv6: bool) -> Mapping {
    let mut dns = Mapping::new();
    dns.insert(key("enable"), Value::Bool(true));
    dns.insert(key("ipv6"), Value::Bool(ipv6));
    dns.insert(key("enhanced-mode"), key(mode));
    dns.insert(key("respect-rules"), Value::Bool(false));
    match preset {
        "china" => {
            dns.insert(
                key("default-nameserver"),
                string_list(&["223.5.5.5", "119.29.29.29"]),
            );
            dns.insert(
                key("nameserver"),
                string_list(&[
                    "https://dns.alidns.com/dns-query",
                    "https://doh.pub/dns-query",
                ]),
            );
            dns.insert(
                key("fallback"),
                string_list(&["https://1.1.1.1/dns-query", "https://dns.google/dns-query"]),
            );
            let mut filter = Mapping::new();
            filter.insert(key("geoip"), Value::Bool(true));
            filter.insert(key("geoip-code"), key("CN"));
            filter.insert(key("ipcidr"), string_list(&["240.0.0.0/4"]));
            dns.insert(key("fallback-filter"), Value::Mapping(filter));
        }
        "global" => {
            dns.insert(
                key("default-nameserver"),
                string_list(&["1.1.1.1", "8.8.8.8"]),
            );
            dns.insert(
                key("nameserver"),
                string_list(&["https://1.1.1.1/dns-query", "https://dns.google/dns-query"]),
            );
            dns.insert(key("fallback"), Value::Sequence(Vec::new()));
        }
        _ => unreachable!(),
    }
    if mode == "fake-ip" {
        dns.insert(key("fake-ip-range"), key("198.18.0.1/16"));
        dns.insert(
            key("fake-ip-filter"),
            string_list(&[
                "*.lan",
                "*.local",
                "localhost.ptlogin2.qq.com",
                "dns.msftncsi.com",
                "www.msftconnecttest.com",
                "time.*.com",
                "ntp.*.com",
                "+.pool.ntp.org",
            ]),
        );
    }
    dns
}

/// Configure Mihomo's DNS mode (`fake-ip`, `redir-host`, or `off`) and upstream preset.
pub fn dns_configure(ctx: &Context, mode: &str, preset: &str, ipv6: bool) -> Result<()> {
    if !matches!(mode, "fake-ip" | "redir-host" | "off") {
        return Err(Error::new(
            "error.input",
            "DNS mode must be fake-ip, redir-host or off",
        ));
    }
    if !matches!(preset, "china" | "global") {
        return Err(Error::new(
            "error.input",
            "DNS preset must be china or global",
        ));
    }
    let applied = edit_mixin_and_apply(ctx, MixinApplyMode::HotReloadOrRestart, |map| {
        if mode == "off" {
            let entry = map
                .entry(key("dns"))
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            match entry.as_mapping_mut() {
                Some(dns_map) => {
                    dns_map.insert(key("enable"), Value::Bool(false));
                }
                None => {
                    let mut disabled = Mapping::new();
                    disabled.insert(key("enable"), Value::Bool(false));
                    *entry = Value::Mapping(disabled);
                }
            }
        } else {
            let existing_listen = map
                .get(key("dns"))
                .and_then(Value::as_mapping)
                .and_then(|dns| dns.get(key("listen")).cloned());
            let mut dns = dns_mapping(mode, preset, ipv6);
            if let Some(listen) = existing_listen {
                dns.insert(key("listen"), listen);
            }
            map.insert(key("dns"), Value::Mapping(dns));
        }
        Ok(())
    })?;
    let detail = if mode == "off" {
        format!("disabled; {}", applied.suffix())
    } else {
        format!(
            "mode={mode}; preset={preset}; ipv6={ipv6}; {}",
            applied.suffix()
        )
    };
    ctx.message("notice.clash_dns", detail);
    Ok(())
}

fn redact_yaml_value(value: &mut Value) {
    match value {
        Value::Mapping(map) => {
            for (k, v) in map.iter_mut() {
                let name = k.as_str().unwrap_or_default().to_ascii_lowercase();
                if matches!(name.as_str(), "secret" | "password") {
                    if v.as_str().is_some_and(|s| !s.is_empty()) {
                        *v = key("[redacted]");
                    }
                } else if name == "authentication" {
                    if let Some(items) = v.as_sequence_mut() {
                        for item in items {
                            if item.as_str().is_some_and(|s| !s.is_empty()) {
                                *item = key("[redacted]");
                            }
                        }
                    }
                } else {
                    redact_yaml_value(v);
                }
            }
        }
        Value::Sequence(items) => {
            for item in items {
                redact_yaml_value(item);
            }
        }
        _ => (),
    }
}

/// Inspect the current Mixin file with secrets and proxy authentication redacted.
pub fn mixin_status(ctx: &Context) -> Result<()> {
    storage::check_path(&ctx.paths.clash_mixin, true)?;
    if !ctx.paths.clash_mixin.is_file() {
        ctx.message(
            "notice.clash_mixin",
            "empty (no persistent Mixin overrides configured)",
        );
        return Ok(());
    }
    let bytes = fs::read(&ctx.paths.clash_mixin)?;
    let mut parsed = yaml(&bytes)?;
    redact_yaml_value(&mut parsed);
    let rendered = serde_yaml::to_string(&parsed).map_err(|e| Error::new("error.data", e))?;
    let safe = logs::redact(ctx, rendered.trim_end())?;
    ctx.message(
        "notice.clash_mixin",
        format!("{}\n{safe}", ctx.paths.clash_mixin.display()),
    );
    Ok(())
}

/// Remove all persistent Mixin overrides while preserving installed subscriptions and resources.
pub fn mixin_reset(ctx: &Context) -> Result<()> {
    let applied = reset_mixin_and_apply(
        ctx,
        MixinApplyMode::RestartIfRunning,
        &[(socks::auth_path(ctx), None)],
    )?;
    ctx.message(
        "notice.clash_mixin",
        format!(
            "Mixin overrides cleared; subscriptions preserved ({})",
            applied.suffix()
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_settings_validates_inputs_and_preserves_existing_mixin() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "custom: keep\n").unwrap();

        configure(&ctx, "7891", true, true, true, "warning").unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["mixed-port"], 7891);
        assert_eq!(mixin["allow-lan"], true);
        assert_eq!(mixin["bind-address"], "*");
        assert_eq!(mixin["ipv6"], true);
        assert_eq!(mixin["log-level"], "warning");
        assert_eq!(mixin["sniffer"]["enable"], true);
        assert_eq!(mixin["sniffer"]["sniff"]["TLS"]["ports"][0], 443);
        assert_eq!(mixin["custom"], "keep");

        configure(&ctx, "7890", false, false, false, "info").unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["bind-address"], "127.0.0.1");
        assert_eq!(mixin["sniffer"]["enable"], false);

        for bad_port in ["", "0", "65536", "abc"] {
            assert!(validate_mixed_port(bad_port).is_err(), "{bad_port}");
        }
        assert!(configure(&ctx, "7890", false, false, false, "trace").is_err());
    }

    #[test]
    fn dns_configure_supports_china_global_and_off_modes() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();

        dns_configure(&ctx, "fake-ip", "china", true).unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["dns"]["enable"], true);
        assert_eq!(mixin["dns"]["enhanced-mode"], "fake-ip");
        assert_eq!(mixin["dns"]["ipv6"], true);
        assert_eq!(mixin["dns"]["fallback-filter"]["geoip-code"], "CN");
        assert_eq!(mixin["dns"]["fake-ip-range"], "198.18.0.1/16");
        dns_status(&ctx).unwrap();

        dns_configure(&ctx, "redir-host", "global", false).unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["dns"]["enhanced-mode"], "redir-host");
        assert_eq!(mixin["dns"]["nameserver"][0], "https://1.1.1.1/dns-query");
        assert_eq!(mixin["dns"]["fallback"].as_sequence().unwrap().len(), 0);
        assert!(mixin["dns"].get("fake-ip-range").is_none());

        dns_configure(&ctx, "off", "china", false).unwrap();
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["dns"]["enable"], false);

        assert!(dns_configure(&ctx, "invalid", "china", false).is_err());
        assert!(dns_configure(&ctx, "fake-ip", "invalid", false).is_err());
    }

    #[test]
    fn mixin_status_redacts_secrets_and_reset_removes_only_mixin() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_config, "mode: direct\nmixed-port: 7890\n").unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "external-controller: 0.0.0.0:9090\nsecret: super-secret-value\nauthentication:\n  - user:pass123\n",
        )
        .unwrap();

        let mut parsed = yaml(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        redact_yaml_value(&mut parsed);
        let rendered = serde_yaml::to_string(&parsed).unwrap();
        assert!(!rendered.contains("super-secret-value"));
        assert!(!rendered.contains("pass123"));
        assert!(rendered.contains("[redacted]"));
        mixin_status(&ctx).unwrap();

        mixin_reset(&ctx).unwrap();
        assert!(!ctx.paths.clash_mixin.exists());
        assert!(ctx.paths.clash_config.exists());
        assert!(ctx.paths.clash_runtime.exists());
        mixin_status(&ctx).unwrap();
    }
}
