use super::*;
use std::io::{Seek, SeekFrom};

const WINDOW: u64 = 1024 * 1024;

pub(super) fn open_private_log(ctx: &Context) -> Result<fs::File> {
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(&ctx.paths.clash_log)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    Ok(options.open(&ctx.paths.clash_log)?)
}

fn collect_sensitive(value: &Value, sensitive: bool, secrets: &mut Vec<String>) {
    match value {
        Value::String(text) if sensitive && !text.is_empty() => {
            secrets.push(text.clone());
            if let Some((_, password)) = text.split_once(':')
                && !password.is_empty()
            {
                secrets.push(password.into());
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_sensitive(item, sensitive, secrets);
            }
        }
        Value::Mapping(map) => {
            for (key, item) in map {
                let key = key.as_str().unwrap_or_default().to_ascii_lowercase();
                collect_sensitive(
                    item,
                    sensitive
                        || matches!(
                            key.as_str(),
                            "secret"
                                | "password"
                                | "passwd"
                                | "token"
                                | "authentication"
                                | "authorization"
                                | "private-key"
                                | "uuid"
                                | "psk"
                                | "auth"
                                | "url"
                        ),
                    secrets,
                );
            }
        }
        _ => (),
    }
}

/// Never fall back to raw logs if configuration cannot be read for redaction.
pub(super) fn redact(ctx: &Context, text: &str) -> Result<String> {
    let mut secrets = Vec::new();
    for path in [
        &ctx.paths.clash_runtime,
        &ctx.paths.clash_mixin,
        &ctx.paths.clash_config,
        &ctx.paths.clash_profiles,
    ] {
        storage::check_path(path, true)?;
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut bytes = Vec::new();
        file.take(8 * WINDOW + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > 8 * WINDOW {
            return Err(Error::new(
                "error.data",
                "configuration too large for safe log redaction",
            ));
        }
        let value: Value = serde_yaml::from_slice(&bytes).map_err(|_| {
            Error::new(
                "error.data",
                "invalid YAML; cannot safely redact Mihomo logs",
            )
        })?;
        collect_sensitive(&value, false, &mut secrets);
    }
    let encoded = secrets
        .iter()
        .map(|secret| {
            let encoded = serde_json::to_string(secret)?;
            Ok(encoded[1..encoded.len() - 1].to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    secrets.extend(encoded);
    secrets
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    secrets.dedup();
    let mut output = text.to_owned();
    if !secrets.is_empty() {
        let pattern = secrets
            .iter()
            .map(|secret| regex::escape(secret))
            .collect::<Vec<_>>()
            .join("|");
        output = regex::Regex::new(&pattern)
            .map_err(|_| Error::new("error.data", "credentials too large for safe log redaction"))?
            .replace_all(text, "[redacted]")
            .into_owned();
    }
    let fields = regex::Regex::new(
        r#"(?im)((?:authorization|secret|password|passwd|token|uuid|psk|auth)[\"']?\s*[:=]\s*)[^\r\n]+"#,
    )
    .expect("constant credential regex");
    output = fields.replace_all(&output, "${1}[redacted]").into_owned();
    // Credentials and subscription query strings can also appear in old logs.
    let urls = regex::Regex::new(r#"(?i)(?:https?|socks4a?|socks5h?)://[^\s\"'<>]+"#)
        .expect("constant URL regex");
    output = urls
        .replace_all(
            &output,
            |caps: &regex::Captures<'_>| match url::Url::parse(&caps[0]) {
                Ok(mut url) => {
                    if !url.username().is_empty() || url.password().is_some() {
                        let _ = url.set_username("redacted");
                        let _ = url.set_password(None);
                    }
                    if url.query().is_some() {
                        url.set_query(Some("redacted"));
                    }
                    if url.fragment().is_some() {
                        url.set_fragment(Some("redacted"));
                    }
                    url.to_string()
                }
                Err(_) => "[redacted URL]".into(),
            },
        )
        .into_owned();
    Ok(output)
}

pub(super) fn startup_tail(ctx: &Context) -> Result<String> {
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut file = fs::File::open(&ctx.paths.clash_log)?;
    let start = file.metadata()?.len().saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes)?;
    redact(ctx, &String::from_utf8_lossy(&bytes))
}

pub fn logs(ctx: &Context, lines: &str, follow: bool) -> Result<()> {
    let lines = lines
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=2000).contains(n))
        .ok_or_else(|| Error::new("error.input", "lines must be between 1 and 2000"))?;
    ctx.check()?;
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut file = fs::File::open(&ctx.paths.clash_log)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file).take(WINDOW).read_to_end(&mut bytes)?;
    let mut offset = start + bytes.len() as u64;
    let mut pending = Vec::new();
    let end = if follow && !bytes.ends_with(b"\n") {
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        pending.extend_from_slice(&bytes[end..]);
        end
    } else {
        bytes.len()
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, tail)| tail)
    } else {
        &text
    };
    let rows: Vec<_> = text.lines().rev().take(lines).collect();
    ctx.log(redact(
        ctx,
        &rows.into_iter().rev().collect::<Vec<_>>().join("\n"),
    )?);
    if !follow {
        return Ok(());
    }
    loop {
        ctx.check()?;
        thread::sleep(Duration::from_millis(200));
        storage::check_path(&ctx.paths.clash_log, true)?;
        let mut current = fs::File::open(&ctx.paths.clash_log)?;
        let metadata = current.metadata()?;
        #[cfg(unix)]
        let replaced = {
            use std::os::unix::fs::MetadataExt;
            let previous = file.metadata()?;
            previous.ino() != metadata.ino() || previous.dev() != metadata.dev()
        };
        #[cfg(not(unix))]
        let replaced = false;
        if replaced || metadata.len() < offset {
            offset = 0;
            pending.clear();
        }
        current.seek(SeekFrom::Start(offset))?;
        bytes.clear();
        (&mut current).take(WINDOW).read_to_end(&mut bytes)?;
        offset += bytes.len() as u64;
        pending.extend_from_slice(&bytes);
        if let Some(last) = pending.iter().rposition(|b| *b == b'\n') {
            let text = String::from_utf8_lossy(&pending[..=last]);
            ctx.log(redact(ctx, &text)?);
            pending.drain(..=last);
        }
        if pending.len() as u64 > WINDOW {
            return Err(Error::new("error.data", "Mihomo log line exceeds 1 MiB"));
        }
        #[cfg(unix)]
        {
            file = current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::context;

    #[test]
    fn logs_redact_secrets_proxy_passwords_and_subscription_queries() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "secret: controller-secret\nauthentication: ['user:proxy-password']\n",
        )
        .unwrap();
        let output = redact(&ctx, "controller-secret proxy-password http://user:old-password@localhost:9090/ui https://example.invalid/sub?token=old-token socks5h://user:old-socks-password@localhost:7890").unwrap();
        for secret in [
            "controller-secret",
            "proxy-password",
            "old-password",
            "old-token",
            "old-socks-password",
        ] {
            assert!(!output.contains(secret));
        }
        assert!(output.contains("localhost:9090/ui"));
    }

    #[test]
    fn malformed_config_prevents_raw_log_output() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: [unterminated-secret").unwrap();
        let error = redact(&ctx, "unterminated-secret").unwrap_err();
        assert!(!error.to_string().contains("unterminated-secret"));
    }

    #[test]
    fn private_log_rejects_invalid_line_count_and_is_private() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        assert!(logs(&ctx, "0", false).is_err());
        assert!(logs(&ctx, "2001", false).is_err());
        ensure_layout(&ctx).unwrap();
        open_private_log(&ctx).unwrap();
        assert_eq!(storage::mode(&ctx.paths.clash_log).unwrap(), 0o600);
    }

    #[test]
    fn old_bearer_tokens_and_json_escaped_secrets_are_redacted() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: 'quoted\"token'\n").unwrap();
        let text = redact(&ctx, "Authorization: Bearer old-rotated-secret\n{\"name\":\"quoted\\\"token\"}\npassword=old-password\n").unwrap();
        assert!(!text.contains("old-rotated-secret"));
        assert!(!text.contains("old-password"));
        assert!(!text.contains("quoted"));
    }

    #[test]
    fn proxy_credentials_are_redacted_in_config_and_old_logs() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - name: node\n    type: vless\n    uuid: 00000000-0000-4000-8000-000000000001\n    auth: private-auth\n    psk: private-psk\n",
        )
        .unwrap();
        let config = fs::read_to_string(&ctx.paths.clash_mixin).unwrap();
        let text = redact(&ctx, &config).unwrap();
        for secret in [
            "00000000-0000-4000-8000-000000000001",
            "private-auth",
            "private-psk",
        ] {
            assert!(!text.contains(secret));
            assert_eq!(redact(&ctx, secret).unwrap(), "[redacted]");
        }
        let old = redact(&ctx, "uuid=old-uuid\nauth=old-auth\npsk=old-psk\n").unwrap();
        assert_eq!(old, "uuid=[redacted]\nauth=[redacted]\npsk=[redacted]\n");
    }

    #[test]
    fn short_credentials_do_not_expand_the_whole_log() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - password: r\n  - password: e\n  - password: d\n  - password: a\n  - password: c\n  - password: t\n",
        )
        .unwrap();
        let output = redact(&ctx, "re").unwrap();
        assert_eq!(output, "[redacted][redacted]");
        assert_eq!(redact(&ctx, &"r".repeat(1024)).unwrap().len(), 1024 * 10);
    }

    #[test]
    fn overlapping_and_json_escaped_credentials_are_replaced_once() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - password: 'quoted'\n  - password: 'quoted\"value'\n  - password: 'quoted\\'\n",
        )
        .unwrap();
        assert_eq!(
            redact(&ctx, "quoted\"value quoted\\\"value quoted\\\\ quoted").unwrap(),
            "[redacted] [redacted] [redacted] [redacted]"
        );
    }

    #[test]
    fn follow_buffers_incomplete_secret_lines_and_cancels() {
        use std::{
            io::Write,
            sync::{atomic::Ordering, mpsc},
        };
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: split-private-token\n").unwrap();
        let mut file = open_private_log(&ctx).unwrap();
        write!(file, "ready\nsplit-private").unwrap();
        file.flush().unwrap();
        let (tx, rx) = mpsc::channel();
        ctx.events = Some(tx);
        let worker_ctx = ctx.clone();
        let worker = thread::spawn(move || logs(&worker_ctx, "100", true));
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(first, crate::runtime::Event::Log(ref text) if text == "ready"));
        writeln!(file, "-token").unwrap();
        file.flush().unwrap();
        let next = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        ctx.cancel.store(true, Ordering::SeqCst);
        let result = worker.join().unwrap();
        assert!(
            matches!(next, crate::runtime::Event::Log(ref text) if text.contains("[redacted]") && !text.contains("private"))
        );
        assert_eq!(result.unwrap_err().key, "error.cancelled");
    }
}
