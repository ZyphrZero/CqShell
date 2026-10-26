use crate::{
    error::{Error, Result},
    runtime::{Context, available},
    storage::{self, Lock, Saved},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn info(ctx: &Context) -> Result<()> {
    for (key, program, args) in [
        ("info.host", "hostname", &[][..]),
        ("info.kernel", "uname", &["-r"][..]),
        ("info.arch", "uname", &["-m"][..]),
        ("info.cpu", "lscpu", &[][..]),
        ("info.memory", "free", &["-m"][..]),
        ("info.disk", "df", &["-h", "/"][..]),
        ("info.uptime", "uptime", &["-p"][..]),
        (
            "info.timezone",
            "timedatectl",
            &["show", "--property=Timezone", "--value"][..],
        ),
    ] {
        ctx.log(ctx.i18n.text(key));
        ctx.run(program, args)?;
    }
    ctx.log(fs::read_to_string("/etc/os-release")?);
    let before = fs::read_to_string("/proc/stat")?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    let after = fs::read_to_string("/proc/stat")?;
    ctx.message(
        "notice.cpu_usage",
        format!("{:.1}%", cpu_usage(&before, &after)?),
    );
    ctx.message(
        "notice.tcp_config",
        ctx.i18n.text(if ctx.paths.tcp.exists() {
            "status.enabled"
        } else {
            "status.disabled"
        }),
    );
    ctx.run("ip", &["-brief", "address"])?;
    super::network::status(ctx)
}
pub fn cpu_usage(before: &str, after: &str) -> Result<f64> {
    let sample = |text: &str| -> Result<(u64, u64)> {
        let line = text
            .lines()
            .find(|l| l.starts_with("cpu "))
            .ok_or_else(|| Error::new("error.data", "/proc/stat: cpu"))?;
        let values = line
            .split_whitespace()
            .skip(1)
            .take(8)
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::new("error.data", e))?;
        if values.len() < 4 {
            return Err(Error::new("error.data", line));
        }
        Ok((
            values.iter().sum(),
            values[3] + values.get(4).copied().unwrap_or(0),
        ))
    };
    let (old_total, old_idle) = sample(before)?;
    let (total, idle) = sample(after)?;
    if total <= old_total || idle < old_idle || idle - old_idle > total - old_total {
        return Err(Error::new("error.data", "/proc/stat: invalid delta"));
    }
    Ok(100.0 * (1.0 - (idle - old_idle) as f64 / (total - old_total) as f64))
}
pub fn public_ip(ctx: &Context) -> Result<()> {
    let mut failures = Vec::new();
    for (family, endpoint) in [
        ("IPv4", "https://ipv4.ip.sb"),
        ("IPv6", "https://ipv6.ip.sb"),
        ("ISP", "https://ipinfo.io/json"),
    ] {
        match ctx.output(
            "curl",
            &["--proto", "=https", "-fSs", "--max-time", "5", endpoint],
        ) {
            Ok(data) => {
                if family == "ISP" {
                    let info: serde_json::Value = serde_json::from_str(&data)?;
                    ctx.message(
                        "notice.public_ip",
                        format!(
                            "ISP: {}",
                            info["org"]
                                .as_str()
                                .ok_or_else(|| Error::new("error.data", "ipinfo.org"))?
                        ),
                    );
                } else {
                    ctx.message("notice.public_ip", format!("{family}: {data}"));
                }
            }
            Err(e) => {
                ctx.log(ctx.i18n.error(&e));
                failures.push(family);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new("error.verify", failures.join(", ")))
    }
}
pub fn update(ctx: &Context) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    ctx.run("apt-get", &["update"])?;
    ctx.run("apt-get", &["upgrade", "-y"])
}
pub fn clean(ctx: &Context, scope: &str) -> Result<()> {
    if !["standard", "full"].contains(&scope) {
        return Err(Error::new("error.input", scope));
    }
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    ctx.run("apt-get", &["clean"])?;
    ctx.run("journalctl", &["--vacuum-time=7d"])?;
    // Kernel removal is always explicit through the kernel module; autoremove may remove fallback kernels.
    ctx.log(ctx.i18n.text("notice.preserve_kernels"));
    if scope == "full" {
        ctx.confirm(
            "confirm.clean_full",
            "/var/log, /root, /home, /ql; /tmp, /var/tmp; Docker",
            "CLEAN",
        )?;
        for root in ["/var/log", "/root", "/home", "/ql"] {
            if Path::new(root).exists() {
                ctx.run(
                    "find",
                    &[
                        root, "-xdev", "-type", "f", "-name", "*.log", "-exec", "truncate", "-s",
                        "0", "{}", ";",
                    ],
                )?;
            }
        }
        for root in ["/tmp", "/var/tmp"] {
            ctx.run(
                "find",
                &[root, "-xdev", "-type", "f", "-mtime", "+1", "-delete"],
            )?;
        }
        let mut homes = vec![PathBuf::from("/root")];
        if Path::new("/home").is_dir() {
            for entry in fs::read_dir("/home")? {
                homes.push(entry?.path());
            }
        }
        for home in homes {
            let cache = home.join(".cache");
            storage::check_path(&cache, false)?;
            if cache.is_dir() {
                for entry in fs::read_dir(&cache)? {
                    ctx.check()?;
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        fs::remove_dir_all(entry.path())?;
                    } else {
                        fs::remove_file(entry.path())?;
                    }
                }
            }
        }
        if available("docker") {
            ctx.run("docker", &["system", "prune", "-a", "-f", "--volumes"])?;
        }
    }
    Ok(())
}
pub fn timezone(ctx: &Context, value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('/')
        || value.split('/').any(|p| matches!(p, ".." | "." | ""))
        || value.chars().any(|c| c.is_control())
    {
        return Err(Error::new("error.input", value));
    }
    ctx.run("timedatectl", &["set-timezone", value])
}

pub fn fstab_without_swap(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|line| line.split_whitespace().next() != Some("/swapfile"))
        .collect()
}
pub fn swap_size(memory_mb: u64) -> u64 {
    match memory_mb / 1024 {
        0..=4 => (memory_mb * 2).min(4096),
        5..=8 => memory_mb,
        9..=64 => 8192,
        _ => 16384,
    }
}
pub fn swap(ctx: &Context, operation: &str, size: &str) -> Result<()> {
    if Path::new("/proc/vz").exists() {
        return Err(Error::new("error.platform", "OpenVZ"));
    }
    let megabytes = match operation {
        "remove" => 0,
        "auto" => swap_size(super::memory::physical_kb(Path::new("/proc"))? / 1024),
        "create" => size
            .parse::<u64>()
            .map_err(|e| Error::new("error.input", e))?,
        _ => return Err(Error::new("error.input", operation)),
    };
    if operation != "remove" && !(1..=1_048_576).contains(&megabytes) {
        return Err(Error::new("error.input", size));
    }
    let _lock = Lock::acquire(Path::new("/run/lock/cqs-swap.lock"))?;
    let target = Path::new("/swapfile");
    let fstab = Path::new("/etc/fstab");
    storage::check_path(target, true)?;
    let before = Saved::capture(fstab)?.ok_or_else(|| Error::new("error.path", fstab.display()))?;
    let text = String::from_utf8(before.bytes()?).map_err(|e| Error::new("error.data", e))?;
    let existed = target.exists();
    let registered = text
        .lines()
        .any(|l| l.split_whitespace().next() == Some("/swapfile"));
    if operation == "create" && (existed || registered) {
        return Err(Error::new("error.swap_exists", "/swapfile"));
    }
    if registered && !existed {
        return Err(Error::new("error.snapshot", "/etc/fstab: /swapfile"));
    }
    if operation == "remove" && !existed {
        return Err(Error::new("error.path", "/swapfile"));
    }
    let active = fs::read_to_string("/proc/swaps")?
        .lines()
        .any(|l| l.split_whitespace().next() == Some("/swapfile"));
    let pending = Path::new("/etc/fstab.cqs-swap-pending.json");
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let backup = if existed {
        let temporary = tempfile::Builder::new()
            .prefix(".cqs-swap-backup-")
            .tempfile_in("/")?;
        let (file, path) = temporary.keep().map_err(|e| Error::from(e.error))?;
        drop(file);
        Some(path)
    } else {
        None
    };
    storage::atomic_write(
        pending,
        &serde_json::to_vec_pretty(
            &serde_json::json!({"fstab": before, "active": active, "backup": backup}),
        )?,
        0o600,
    )?;
    let mut moved = false;
    let mut created = false;
    let result = (|| {
        if active {
            ctx.run("swapoff", &["/swapfile"])?;
        }
        if let Some(backup) = &backup {
            fs::rename(target, backup)?;
            moved = true;
        }
        if operation != "remove" {
            created = true;
            // fallocate can create the file before failing; mark it before invoking the tool.
            ctx.run("fallocate", &["-l", &format!("{megabytes}M"), "/swapfile"])?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(target, fs::Permissions::from_mode(0o600))?;
            }
            ctx.run("mkswap", &["/swapfile"])?;
            ctx.run("swapon", &["/swapfile"])?;
        }
        let mut new_fstab = fstab_without_swap(&text);
        if operation != "remove" {
            if !new_fstab.ends_with('\n') {
                new_fstab.push('\n');
            }
            new_fstab.push_str("/swapfile none swap defaults 0 0\n");
        }
        ctx.check()?;
        storage::atomic_write(fstab, new_fstab.as_bytes(), before.mode)
    })();
    if let Err(error) = result {
        let recovery = ctx.recovery();
        let restore = (|| {
            if created {
                let active_now = fs::read_to_string("/proc/swaps")?
                    .lines()
                    .any(|l| l.split_whitespace().next() == Some("/swapfile"));
                if active_now {
                    recovery.run("swapoff", &["/swapfile"])?;
                }
                storage::remove_file(target)?;
            }
            if moved {
                fs::rename(backup.as_ref().unwrap(), target)?;
            }
            if active
                && !fs::read_to_string("/proc/swaps")?
                    .lines()
                    .any(|l| l.split_whitespace().next() == Some("/swapfile"))
            {
                recovery.run("swapon", &["/swapfile"])?;
            }
            Saved::write(fstab, &Some(before))
        })();
        if let Err(e) = restore {
            return Err(error.with_recovery(e, pending));
        }
        if let Some(backup) = &backup {
            storage::remove_file(backup)?;
        }
        storage::remove_file(pending)?;
        return Err(error);
    }
    if let Some(backup) = &backup {
        storage::remove_file(backup)?;
    }
    storage::remove_file(pending)?;
    ctx.log(fs::read_to_string("/proc/swaps")?);
    Ok(())
}

pub fn fail2ban(ctx: &Context) -> Result<()> {
    let _package_lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    ctx.run("apt-get", &["install", "-y", "fail2ban"])?;
    let target = Path::new("/etc/fail2ban/jail.d/cqs.local");
    let _lock = Lock::acquire(&storage::suffix(target, ".cqs-lock"))?;
    let effective = ctx.output("sshd", &["-T"])?;
    let ports: Vec<_> = effective
        .lines()
        .filter_map(|l| l.strip_prefix("port "))
        .collect();
    if ports.is_empty() {
        return Err(Error::new("error.verify", "sshd ports"));
    }
    let data = format!(
        "[sshd]\nenabled = true\nbackend = systemd\nport = {}\nbantime = 86400\nfindtime = 86400\nmaxretry = 3\n",
        ports.join(",")
    );
    let active = ctx.active("fail2ban")?;
    super::service::update(
        ctx,
        target,
        data.as_bytes(),
        "fail2ban",
        active,
        |_| Ok(()),
        || {
            ctx.run("fail2ban-client", &["-t"])?;
            ctx.run("systemctl", &["restart", "fail2ban"])?;
            if !ctx.active("fail2ban")? {
                return Err(Error::new("error.verify", "fail2ban"));
            }
            Ok(())
        },
    )?;
    ctx.run("systemctl", &["enable", "fail2ban"])
}
pub fn install_self(ctx: &Context) -> Result<()> {
    let executable = std::env::current_exe()?;
    let bytes = fs::read(executable)?;
    storage::atomic_write(&ctx.paths.install, &bytes, 0o755)?;
    ctx.message("notice.saved", ctx.paths.install.display());
    Ok(())
}
pub fn uninstall_self(ctx: &Context) -> Result<()> {
    storage::remove_file(&ctx.paths.install)?;
    ctx.message("notice.uninstalled", ctx.paths.install.display());
    Ok(())
}
