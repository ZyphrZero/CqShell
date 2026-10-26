use crate::{
    error::{Error, Result},
    runtime::{Context, path},
    storage::{self, Lock, Saved},
};
use std::{collections::BTreeMap, fs, path::Path};

pub type Parameters = BTreeMap<String, String>;
const KEYS: &[&str] = &[
    "net.core.default_qdisc",
    "net.ipv4.tcp_congestion_control",
    "net.core.rmem_max",
    "net.core.wmem_max",
    "net.ipv4.tcp_rmem",
    "net.ipv4.tcp_wmem",
    "net.ipv4.tcp_moderate_rcvbuf",
    "net.ipv4.tcp_mtu_probing",
    "net.ipv4.tcp_limit_output_bytes",
    "net.ipv4.tcp_slow_start_after_idle",
];

pub fn parse(text: &str) -> Result<Parameters> {
    let mut values = Parameters::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| Error::new("error.input", line))?;
        let key = key.trim();
        let value = value.trim();
        if !KEYS.contains(&key)
            || value.is_empty()
            || value.chars().any(|c| c.is_control() && c != '\t')
            || values.contains_key(key)
        {
            return Err(Error::new("error.input", line));
        }
        values.insert(key.into(), value.into());
    }
    if values.is_empty() {
        return Err(Error::new("error.data", "empty sysctl snapshot"));
    }
    Ok(values)
}
fn render(values: &Parameters) -> String {
    values.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()
}
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn read(ctx: &Context, keys: &Parameters) -> Result<Parameters> {
    keys.keys()
        .map(|key| Ok((key.clone(), ctx.output("sysctl", &["-n", key])?)))
        .collect()
}
fn verify(ctx: &Context, values: &Parameters) -> Result<()> {
    for (key, value) in values {
        let actual = ctx.output("sysctl", &["-n", key])?;
        if normalized(&actual) != normalized(value) {
            return Err(Error::new(
                "error.verify",
                format!("{key}: {value} != {actual}"),
            ));
        }
    }
    Ok(())
}
fn restore_values(ctx: &Context, values: &Parameters) -> Result<()> {
    let mut errors = Vec::new();
    for (key, value) in values {
        match ctx.output("sysctl", &["-n", key]) {
            Ok(current) if normalized(&current) == normalized(value) => (),
            Ok(_) => {
                if let Err(e) = ctx.run("sysctl", &["-w", &format!("{key}={value}")]) {
                    errors.push(e.to_string());
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    if let Err(e) = verify(ctx, values) {
        errors.push(e.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::new("error.verify", errors.join("; ")))
    }
}
fn baseline(backup: &Path) -> Result<(Parameters, Option<Saved>)> {
    storage::check_path(backup, false)?;
    for name in ["runtime.conf", "original.conf", "no-original"] {
        storage::check_path(&backup.join(name), true)?;
    }
    let original = Saved::capture(&backup.join("original.conf"))?;
    if original.is_some() == backup.join("no-original").exists() {
        return Err(Error::new("error.snapshot", backup.display()));
    }
    Ok((
        parse(&fs::read_to_string(backup.join("runtime.conf"))?)?,
        original,
    ))
}
fn remove_baseline(backup: &Path) -> Result<()> {
    for name in ["runtime.conf", "original.conf", "no-original"] {
        storage::remove_file(&backup.join(name))?;
    }
    fs::remove_dir(backup)?;
    storage::sync_parent(backup)
}

pub fn apply(ctx: &Context, config: &Path, desired: &Parameters) -> Result<()> {
    let desired = parse(&render(desired))?;
    storage::check_path(config, true)?;
    let _lock = Lock::acquire(&storage::suffix(config, ".cqs-lock"))?;
    let pending = storage::suffix(config, ".cqs-pending.json");
    storage::check_path(&pending, true)?;
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let backup = storage::suffix(config, ".cqs-backup");
    storage::check_path(&backup, false)?;
    let new_baseline = !backup.exists();
    if !new_baseline && baseline(&backup)?.0.keys().ne(desired.keys()) {
        return Err(Error::new("error.snapshot", backup.display()));
    }
    let before = read(ctx, &desired)?;
    let original = Saved::capture(config)?;
    storage::atomic_write(
        &pending,
        &serde_json::to_vec_pretty(
            &serde_json::json!({"runtime": before, "file": original, "new_baseline": new_baseline}),
        )?,
        0o600,
    )?;
    let result = (|| {
        if new_baseline {
            let staged = tempfile::Builder::new()
                .prefix(".cqs-baseline-")
                .tempdir_in(config.parent().unwrap())?;
            storage::atomic_write(
                &staged.path().join("runtime.conf"),
                render(&before).as_bytes(),
                0o600,
            )?;
            if let Some(saved) = &original {
                Saved::write(&staged.path().join("original.conf"), &Some(saved.clone()))?;
            } else {
                storage::atomic_write(&staged.path().join("no-original"), b"", 0o600)?;
            }
            fs::rename(staged.path(), &backup)?;
            storage::sync_parent(&backup)?;
        }
        ctx.check()?;
        let candidate = tempfile::NamedTempFile::new_in(config.parent().unwrap())?;
        fs::write(candidate.path(), render(&desired))?;
        ctx.run("sysctl", &["-p", path(candidate.path())?])?;
        verify(ctx, &desired)?;
        ctx.check()?;
        storage::atomic_write(config, render(&desired).as_bytes(), 0o644)
    })();
    if let Err(error) = result {
        let recovery = ctx.recovery();
        let restored =
            restore_values(&recovery, &before).and_then(|_| Saved::write(config, &original));
        if let Err(e) = restored {
            return Err(error.with_recovery(e, &pending));
        }
        if new_baseline && backup.exists() {
            remove_baseline(&backup)?;
        }
        storage::remove_file(&pending)?;
        return Err(error);
    }
    storage::remove_file(&pending)?;
    ctx.message("notice.saved", config.display());
    Ok(())
}

pub fn restore(ctx: &Context, config: &Path) -> Result<()> {
    let _lock = Lock::acquire(&storage::suffix(config, ".cqs-lock"))?;
    let pending = storage::suffix(config, ".cqs-pending.json");
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let backup = storage::suffix(config, ".cqs-backup");
    let (target, original) = baseline(&backup)?;
    let before = read(ctx, &target)?;
    let current = Saved::capture(config)?;
    storage::atomic_write(
        &pending,
        &serde_json::to_vec_pretty(&serde_json::json!({"runtime": before, "file": current}))?,
        0o600,
    )?;
    let result = restore_values(ctx, &target).and_then(|_| {
        ctx.check()?;
        Saved::write(config, &original)
    });
    if let Err(error) = result {
        if let Err(e) =
            restore_values(&ctx.recovery(), &before).and_then(|_| Saved::write(config, &current))
        {
            return Err(error.with_recovery(e, &pending));
        }
        storage::remove_file(&pending)?;
        return Err(error);
    }
    remove_baseline(&backup)?;
    storage::remove_file(&pending)?;
    ctx.message("notice.restored", config.display());
    Ok(())
}

pub fn enable_bbr(ctx: &Context, qdisc: &str) -> Result<()> {
    if !["fq", "fq_codel", "fq_pie", "cake"].contains(&qdisc) {
        return Err(Error::new("error.input", qdisc));
    }
    let available = ctx.output(
        "sysctl",
        &["-n", "net.ipv4.tcp_available_congestion_control"],
    )?;
    if !available.split_whitespace().any(|a| a == "bbr") {
        ctx.run("modprobe", &["tcp_bbr"])?;
        if !ctx
            .output(
                "sysctl",
                &["-n", "net.ipv4.tcp_available_congestion_control"],
            )?
            .split_whitespace()
            .any(|a| a == "bbr")
        {
            return Err(Error::new("error.bbr", "tcp_bbr"));
        }
    }
    apply(
        ctx,
        &ctx.paths.bbr,
        &BTreeMap::from([
            ("net.core.default_qdisc".into(), qdisc.into()),
            ("net.ipv4.tcp_congestion_control".into(), "bbr".into()),
        ]),
    )?;
    ctx.message("notice.qdisc", qdisc);
    Ok(())
}
pub fn tune(ctx: &Context, bandwidth: &str, rtt: &str) -> Result<()> {
    let memory = super::memory::effective_kb(Path::new("/proc"))?;
    let buffer = super::memory::tcp_buffer(bandwidth, rtt, memory)?;
    ctx.message(
        "notice.buffer",
        format!("{} MiB; {} MiB", buffer / 1048576, memory / 1024),
    );
    let text = format!(
        "net.core.rmem_max = {buffer}\nnet.core.wmem_max = {buffer}\nnet.ipv4.tcp_rmem = 4096 131072 {buffer}\nnet.ipv4.tcp_wmem = 4096 16384 {buffer}\nnet.ipv4.tcp_moderate_rcvbuf = 1\nnet.ipv4.tcp_mtu_probing = 1\nnet.ipv4.tcp_limit_output_bytes = 1048576\nnet.ipv4.tcp_slow_start_after_idle = 0\n"
    );
    apply(ctx, &ctx.paths.tcp, &parse(&text)?)
}
pub fn status(ctx: &Context) -> Result<()> {
    ctx.log(ctx.output("uname", &["-r"])?);
    ctx.run(
        "sysctl",
        &[
            "net.ipv4.tcp_congestion_control",
            "net.ipv4.tcp_available_congestion_control",
            "net.core.default_qdisc",
        ],
    )?;
    match fs::read_to_string("/sys/module/tcp_bbr/version") {
        Ok(v) if ["1", "2", "3"].contains(&v.trim()) => ctx.message("notice.bbr_version", v.trim()),
        Ok(_) => ctx.log(ctx.i18n.text("notice.bbr_unknown")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ctx.log(ctx.i18n.text("notice.bbr_unknown"))
        }
        Err(e) => return Err(e.into()),
    }
    ctx.run("tc", &["qdisc", "show"])
}
