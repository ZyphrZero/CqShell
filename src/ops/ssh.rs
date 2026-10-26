use crate::{
    error::{Error, Result},
    runtime::{Context, path},
    storage::{self, Lock, Saved},
};
use std::{fs, path::Path, thread, time::Duration};

pub fn candidate(original: &str, settings: &[(&str, String)]) -> String {
    let mut output = settings
        .iter()
        .map(|(k, v)| format!("{k} {v}\n"))
        .collect::<String>();
    let mut in_match = false;
    for line in original.split_inclusive('\n') {
        let key = line.split_whitespace().next().unwrap_or("");
        if key.eq_ignore_ascii_case("match") {
            in_match = true;
        }
        if !in_match && settings.iter().any(|(k, _)| key.eq_ignore_ascii_case(k)) {
            continue;
        }
        output.push_str(line);
    }
    output
}

pub fn validate_effective(effective: &str, settings: &[(&str, String)]) -> Result<()> {
    for (key, value) in settings {
        let found: Vec<_> = effective
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                if parts.next().is_some_and(|v| v.eq_ignore_ascii_case(key)) {
                    parts.next()
                } else {
                    None
                }
            })
            .collect();
        if found != vec![value.as_str()] {
            return Err(Error::new(
                "error.verify",
                format!("{key}: {found:?} != {value}"),
            ));
        }
        if *key == "Port" {
            for line in effective.lines() {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.first() == Some(&"listenaddress")
                    && parts
                        .get(1)
                        .is_none_or(|v| !v.ends_with(&format!(":{value}")))
                {
                    return Err(Error::new("error.verify", line));
                }
            }
        }
    }
    Ok(())
}
fn listening(ctx: &Context, port: &str) -> Result<bool> {
    let pid = ctx.output(
        "systemctl",
        &["show", "--property=MainPID", "--value", "ssh"],
    )?;
    if pid.parse::<u32>().ok().is_none_or(|p| p == 0) {
        return Ok(false);
    }
    Ok(ctx
        .output("ss", &["-H", "-ltnp", &format!("sport = :{port}")])?
        .contains(&format!("pid={pid},")))
}
fn configure(ctx: &Context, settings: &[(&str, String)]) -> Result<()> {
    ctx.require(&["sshd", "systemctl", "ss"])?;
    if ctx.active("ssh.socket")? || !ctx.active("ssh")? {
        return Err(Error::new("error.ssh_service", "ssh / ssh.socket"));
    }
    let target = &ctx.paths.ssh;
    let _lock = Lock::acquire(&storage::suffix(target, ".cqs-lock"))?;
    storage::check_path(target, true)?;
    let desired = candidate(&fs::read_to_string(target)?, settings);
    super::service::update(
        ctx,
        target,
        desired.as_bytes(),
        "ssh",
        true,
        |p| {
            ctx.run("sshd", &["-t", "-f", path(p)?])?;
            validate_effective(&ctx.output("sshd", &["-T", "-f", path(p)?])?, settings)
        },
        || {
            ctx.run("systemctl", &["reload", "ssh"])?;
            if !ctx.active("ssh")? {
                return Err(Error::new("error.verify", "ssh"));
            }
            if let Some((_, port)) = settings.iter().find(|(k, _)| *k == "Port") {
                for _ in 0..50 {
                    ctx.check()?;
                    if listening(ctx, port)? {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                return Err(Error::new("error.verify", format!("ssh: {port}")));
            }
            Ok(())
        },
    )?;
    ctx.log(ctx.i18n.text("notice.ssh"));
    Ok(())
}
pub fn port(ctx: &Context, port: &str) -> Result<()> {
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|c| c.is_ascii_digit()) {
        return Err(Error::new("error.input", port));
    }
    let number = port
        .parse::<u16>()
        .map_err(|e| Error::new("error.input", e))?;
    if number == 0 {
        return Err(Error::new("error.input", port));
    }
    configure(ctx, &[("Port", number.to_string())])
}

pub fn key_login(ctx: &Context, public_key: &Path) -> Result<()> {
    storage::check_path(public_key, true)?;
    let key = fs::read_to_string(public_key)?;
    let lines: Vec<_> = key.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 1
        || ![
            "ssh-ed25519 ",
            "ssh-rsa ",
            "ecdsa-sha2-",
            "sk-ssh-ed25519@",
            "sk-ecdsa-sha2-",
        ]
        .iter()
        .any(|p| lines[0].starts_with(p))
    {
        return Err(Error::new("error.public_key", public_key.display()));
    }
    ctx.run("ssh-keygen", &["-l", "-f", path(public_key)?])?;
    let directory = ctx.paths.home.join(".ssh");
    storage::private_dir(&directory)?;
    let target = directory.join("authorized_keys");
    let _lock = Lock::acquire(&directory.join("cqs.lock"))?;
    let before = Saved::capture(&target)?;
    let mut current = match &before {
        Some(saved) => {
            String::from_utf8(saved.bytes()?).map_err(|e| Error::new("error.data", e))?
        }
        None => String::new(),
    };
    if !current.lines().any(|l| l.trim() == lines[0].trim()) {
        if !current.is_empty() && !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str(lines[0].trim());
        current.push('\n');
        storage::atomic_write(&target, current.as_bytes(), 0o600)?;
    }
    // Keep the installed public key on cancellation so a new connection can be tested.
    ctx.confirm("confirm.key_tested", target.display(), "KEY-TESTED")?;
    configure(
        ctx,
        &[
            ("PubkeyAuthentication", "yes".into()),
            ("PasswordAuthentication", "no".into()),
            ("KbdInteractiveAuthentication", "no".into()),
        ],
    )
}
