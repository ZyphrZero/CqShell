use crate::{
    error::{Error, Result},
    runtime::{Context, path, sha256},
    storage::{self, Lock, Saved},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const REPOSITORY: &str = "https://api.github.com/repos/byJoey/Actions-bbr-v3";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub kernel: String,
    pub assets: Vec<Asset>,
}

pub fn manifest(
    json: &str,
    architecture: &str,
    profile: &str,
    requested: Option<&str>,
) -> Result<Release> {
    let prefix = match architecture {
        "amd64" => "x86_64",
        "arm64" => "arm64",
        _ => return Err(Error::new("error.input", architecture)),
    };
    let suffix = match profile {
        "standard" => "",
        "max" => "-max",
        _ => return Err(Error::new("error.input", profile)),
    };
    let pattern = Regex::new(&format!(r"^{prefix}-(\d+)\.(\d+)\.(\d+){suffix}$")).unwrap();
    let value: serde_json::Value = serde_json::from_str(json)?;
    let releases = match value {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => vec![value],
        _ => return Err(Error::new("error.release", "expected release array/object")),
    };
    let mut candidates = Vec::new();
    for release in releases {
        let tag = release
            .get("tag_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(captures) = pattern.captures(tag) else {
            continue;
        };
        if release.get("draft").and_then(|v| v.as_bool()) != Some(false)
            || release.get("prerelease").and_then(|v| v.as_bool()) != Some(false)
        {
            continue;
        }
        if requested.is_some_and(|r| r != tag) {
            continue;
        }
        let version = [
            captures[1].parse::<u64>(),
            captures[2].parse(),
            captures[3].parse(),
        ];
        let version = version
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::new("error.release", e))?;
        candidates.push((version, release));
    }
    let (version, release) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .ok_or_else(|| Error::new("error.release", "no matching stable release"))?;
    let tag = release["tag_name"].as_str().unwrap().to_owned();
    let kernel = format!(
        "{}.{}.{}-joeyblog-bbrv3{suffix}",
        version[0], version[1], version[2]
    );
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "assets"))?;
    let mut selected = Vec::new();
    for kind in ["image", "headers"] {
        let name_pattern = Regex::new(&format!(
            r"^linux-{kind}-{}_[A-Za-z0-9.+~\-]+_{architecture}\.deb$",
            regex::escape(&kernel)
        ))
        .unwrap();
        let found: Vec<_> = assets
            .iter()
            .filter(|v| v["name"].as_str().is_some_and(|n| name_pattern.is_match(n)))
            .collect();
        if found.len() != 1 {
            return Err(Error::new(
                "error.release",
                format!("{kind}: {} assets", found.len()),
            ));
        }
        let asset = found[0];
        let name = asset["name"].as_str().unwrap();
        let url = asset["browser_download_url"].as_str().unwrap_or("");
        let digest = asset["digest"].as_str().unwrap_or("");
        if url != format!("https://github.com/byJoey/Actions-bbr-v3/releases/download/{tag}/{name}")
            || !digest.starts_with("sha256:")
            || digest.len() != 71
            || !digest[7..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::new("error.release", name));
        }
        selected.push(Asset {
            name: name.into(),
            url: url.into(),
            digest: digest[7..].to_ascii_lowercase(),
        });
    }
    Ok(Release {
        tag,
        kernel,
        assets: selected,
    })
}

pub fn supported_system(id: &str, version: &str) -> bool {
    let parts = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>();
    let Ok(parts) = parts else { return false };
    match id {
        "debian" => parts.first().is_some_and(|n| *n >= 12),
        "ubuntu" => parts
            .first()
            .is_some_and(|n| *n > 24 || (*n == 24 && parts.get(1).copied().unwrap_or(0) >= 4)),
        _ => false,
    }
}

fn valid_kernel_release(release: &str) -> bool {
    !release.is_empty()
        && release.len() <= 128
        && release.as_bytes()[0].is_ascii_alphanumeric()
        && release
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+_~-".contains(&byte))
}

fn grub_id<'a>(line: &'a str, kind: &str) -> Option<&'a str> {
    let header = line.trim_start();
    let rest = header.strip_prefix(kind)?;
    if !rest.starts_with(' ') {
        return None;
    }
    let (_, value) = header.split_once("$menuentry_id_option")?;
    let value = value.trim_start();
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let value = value.strip_prefix(quote)?;
    let id = &value[..value.find(quote)?];
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+_~-".contains(&byte)))
    .then_some(id)
}

fn grub_title<'a>(line: &'a str, kind: &str) -> Option<&'a str> {
    let header = line.trim_start();
    let rest = header.strip_prefix(kind)?;
    if !rest.starts_with(' ') {
        return None;
    }
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let title = rest.strip_prefix(quote)?;
    let title = &title[..title.find(quote)?];
    (!title.is_empty() && !title.contains('>')).then_some(title)
}

pub(crate) fn grub_boot_entry(config: &str, release: &str) -> Result<String> {
    if !valid_kernel_release(release) {
        return Err(Error::new("error.input", release));
    }
    let prefix = format!("gnulinux-{release}-advanced-");
    let title_suffix = format!("with Linux {release}");
    let candidates: Vec<_> = config
        .lines()
        .filter_map(|line| {
            let title = grub_title(line, "menuentry")?;
            if !title.ends_with(&title_suffix) {
                return None;
            }
            let id = grub_id(line, "menuentry")?;
            let suffix = id.strip_prefix(&prefix)?;
            (!suffix.is_empty()).then_some((line, title, suffix))
        })
        .collect();
    if candidates.len() != 1 {
        return Err(Error::new("error.kernel_boot_entry", release));
    }
    let (line, title, suffix) = candidates[0];
    let submenu_id = format!("gnulinux-advanced-{suffix}");
    let submenus: Vec<_> = config
        .lines()
        .filter_map(|line| {
            (grub_id(line, "submenu")? == submenu_id)
                .then(|| grub_title(line, "submenu"))
                .flatten()
        })
        .collect();
    if submenus.len() == 1 {
        return Ok(format!("{}>{title}", submenus[0]));
    }
    if submenus.is_empty() && line.len() == line.trim_start().len() {
        return Ok(title.to_owned());
    }
    Err(Error::new("error.kernel_boot_entry", release))
}

fn nonempty(path: &Path) -> Result<()> {
    storage::check_path(path, true)?;
    if fs::metadata(path)?.len() == 0 {
        return Err(Error::new("error.kernel_host", path.display()));
    }
    Ok(())
}
fn preflight(ctx: &Context, install: bool) -> Result<String> {
    ctx.require(&[
        "dpkg",
        "dpkg-query",
        "apt-get",
        "update-grub",
        "systemd-detect-virt",
    ])?;
    let virt = ctx.command("systemd-detect-virt", &["--container"], false)?;
    let running = ctx.output("uname", &["-r"])?;
    if virt.status.code() != Some(1)
        || Path::new("/.dockerenv").exists()
        || Path::new("/proc/vz").exists()
        || running.to_ascii_lowercase().contains("microsoft")
    {
        return Err(Error::new(
            "error.kernel_host",
            format!("{running}; {}", String::from_utf8_lossy(&virt.stdout)),
        ));
    }
    for p in [
        ctx.paths.boot.join("grub/grub.cfg"),
        ctx.paths.boot.join(format!("vmlinuz-{running}")),
        ctx.paths.boot.join(format!("initrd.img-{running}")),
    ] {
        nonempty(&p)?;
    }
    if install {
        ctx.require(&["curl", "dpkg-deb", "lsinitramfs"])?;
        let os = fs::read_to_string("/etc/os-release")?;
        let field = |name: &str| {
            os.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}=")))
                .unwrap_or("")
                .trim_matches('"')
        };
        if !supported_system(field("ID"), field("VERSION_ID")) {
            return Err(Error::new("error.kernel_system", os));
        }
        if Path::new("/sys/firmware/efi").exists()
            && !ctx
                .output("mokutil", &["--sb-state"])?
                .contains("SecureBoot disabled")
        {
            return Err(Error::new("error.secure_boot", "mokutil --sb-state"));
        }
        let audit = ctx.output("dpkg", &["--audit"])?;
        if !audit.trim().is_empty() {
            return Err(Error::new("error.kernel_audit", audit));
        }
        if crate::runtime::available("dkms") {
            let status = ctx.output("dkms", &["status"])?;
            if Regex::new(r"mimic[/,]\s*0\.7\.1([/,\s]|$)")
                .unwrap()
                .is_match(&status)
            {
                return Err(Error::new("error.kernel_audit", status));
            }
            ctx.log(status);
        }
    }
    Ok(running)
}

fn boot_preflight(ctx: &Context) -> Result<String> {
    ctx.require(&[
        "dpkg-query",
        "grub-reboot",
        "grub-editenv",
        "reboot",
        "systemd-detect-virt",
    ])?;
    let virt = ctx.command("systemd-detect-virt", &["--container"], false)?;
    let running = ctx.output("uname", &["-r"])?;
    if virt.status.code() != Some(1)
        || Path::new("/.dockerenv").exists()
        || Path::new("/proc/vz").exists()
        || running.to_ascii_lowercase().contains("microsoft")
    {
        return Err(Error::new(
            "error.kernel_host",
            format!("{running}; {}", String::from_utf8_lossy(&virt.stdout)),
        ));
    }
    for p in [
        ctx.paths.boot.join("grub/grub.cfg"),
        ctx.paths.boot.join(format!("vmlinuz-{running}")),
        ctx.paths.boot.join(format!("initrd.img-{running}")),
    ] {
        nonempty(&p)?;
    }
    Ok(running)
}

fn check_hyperv(ctx: &Context, config: &str, initrd: Option<&str>) -> Result<()> {
    if !Path::new("/sys/bus/vmbus").exists() {
        return Ok(());
    }
    let listing = match initrd {
        Some(p) => Some(ctx.output("lsinitramfs", &[p])?),
        None => None,
    };
    let mut symbols = vec![
        ("HYPERV", "hv[-_]vmbus"),
        ("HYPERV_STORAGE", "hv[-_]storvsc"),
        ("HYPERV_NET", "hv[-_]netvsc"),
        ("PCI_HYPERV", "pci[-_]hyperv"),
        ("BLK_DEV_SD", "sd[-_]mod"),
    ];
    if config.contains("CONFIG_HYPERV_VMBUS") {
        symbols.push(("HYPERV_VMBUS", "hv[-_]vmbus"));
    }
    for (symbol, module) in symbols {
        let value = config
            .lines()
            .find_map(|l| l.strip_prefix(&format!("CONFIG_{symbol}=")))
            .unwrap_or("");
        if !matches!(value, "y" | "m") {
            return Err(Error::new("error.kernel_driver", symbol));
        }
        if symbol == "HYPERV"
            && config
                .lines()
                .any(|l| l == "CONFIG_HYPERV_VMBUS=y" || l == "CONFIG_HYPERV_VMBUS=m")
        {
            continue;
        }
        if value == "m"
            && let Some(listing) = &listing
            && !Regex::new(&format!(r"(?m)/{module}\.ko(\.(xz|gz|zst))?$"))
                .unwrap()
                .is_match(listing)
        {
            return Err(Error::new("error.kernel_driver", module));
        }
    }
    Ok(())
}
pub(crate) fn validate_packages(
    ctx: &Context,
    directory: &Path,
    release: &Release,
    arch: &str,
) -> Result<String> {
    for kind in ["image", "headers"] {
        let package = directory.join(format!("{kind}.deb"));
        let name = ctx.output("dpkg-deb", &["-f", path(&package)?, "Package"])?;
        let architecture = ctx.output("dpkg-deb", &["-f", path(&package)?, "Architecture"])?;
        if name != format!("linux-{kind}-{}", release.kernel) || architecture != arch {
            return Err(Error::new(
                "error.release",
                format!("{name} / {architecture}"),
            ));
        }
    }
    let extracted = directory.join("image");
    ctx.run(
        "dpkg-deb",
        &[
            "--extract",
            path(&directory.join("image.deb"))?,
            path(&extracted)?,
        ],
    )?;
    let config_path = extracted.join(format!("boot/config-{}", release.kernel));
    storage::check_path(&config_path, true)?;
    let config = fs::read_to_string(config_path)?;
    for symbol in ["TCP_CONG_BBR", "NET_SCH_FQ"] {
        if !config
            .lines()
            .any(|l| l == format!("CONFIG_{symbol}=y") || l == format!("CONFIG_{symbol}=m"))
        {
            return Err(Error::new("error.kernel_driver", symbol));
        }
    }
    check_hyperv(ctx, &config, None)?;
    Ok(config)
}
fn backup_grub(ctx: &Context, running: &str) -> Result<PathBuf> {
    storage::private_dir(&ctx.paths.kernel_backup)?;
    let backup = tempfile::Builder::new()
        .prefix("boot.")
        .tempdir_in(&ctx.paths.kernel_backup)?
        .keep();
    for name in ["grub.cfg", "grubenv"] {
        let original = Saved::capture(&ctx.paths.boot.join("grub").join(name))?;
        if original.is_some() {
            Saved::write(&backup.join(name), &original)?;
        } else {
            storage::atomic_write(&backup.join(format!("no-{name}")), b"", 0o600)?;
        }
    }
    storage::atomic_write(&backup.join("running-kernel"), running.as_bytes(), 0o600)?;
    Ok(backup)
}
fn restore_grub(ctx: &Context, backup: &Path) -> Result<()> {
    nonempty(&backup.join("grub.cfg"))?;
    for name in ["grub.cfg", "grubenv"] {
        let saved = Saved::capture(&backup.join(name))?;
        if saved.is_none() && !backup.join(format!("no-{name}")).is_file() {
            return Err(Error::new("error.snapshot", backup.display()));
        }
        Saved::write(&ctx.paths.boot.join("grub").join(name), &saved)?;
    }
    Ok(())
}
fn package_complete(ctx: &Context, name: &str) -> Result<()> {
    let status = ctx.output("dpkg-query", &["-W", "-f=${Status}", name])?;
    if !["install ok installed", "hold ok installed"].contains(&status.as_str()) {
        return Err(Error::new(
            "error.kernel_audit",
            format!("{name}: {status}"),
        ));
    }
    Ok(())
}

pub fn install(ctx: &Context, profile: &str, version: &str) -> Result<()> {
    if !["standard", "max"].contains(&profile)
        || (!version.is_empty() && !Regex::new(r"^\d+\.\d+\.\d+$").unwrap().is_match(version))
    {
        return Err(Error::new("error.input", format!("{profile} {version}")));
    }
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let running = preflight(ctx, true)?;
    let arch = ctx.output("dpkg", &["--print-architecture"])?;
    let prefix = match arch.as_str() {
        "amd64" => "x86_64",
        "arm64" => "arm64",
        _ => return Err(Error::new("error.input", arch)),
    };
    let tag = if version.is_empty() {
        None
    } else {
        Some(format!(
            "{prefix}-{version}{}",
            if profile == "max" { "-max" } else { "" }
        ))
    };
    let url = match &tag {
        Some(tag) => format!("{REPOSITORY}/releases/tags/{tag}"),
        None => format!("{REPOSITORY}/releases?per_page=100"),
    };
    let work = tempfile::Builder::new()
        .prefix("cqs-kernel-")
        .tempdir_in("/var/tmp")?;
    let metadata = work.path().join("releases.json");
    ctx.download(&url, &metadata, 60)?;
    let release = manifest(
        &fs::read_to_string(metadata)?,
        &arch,
        profile,
        tag.as_deref(),
    )?;
    if release.kernel == running {
        return Err(Error::new("error.running_kernel", running));
    }
    ctx.confirm(
        "confirm.kernel_download",
        format!("byJoey/Actions-bbr-v3 / {} / {profile}", release.kernel),
        "DOWNLOAD",
    )?;
    for (asset, kind) in release.assets.iter().zip(["image", "headers"]) {
        let target = work.path().join(format!("{kind}.deb"));
        ctx.download(&asset.url, &target, 900)?;
        if sha256(&target)? != asset.digest {
            return Err(Error::new("error.digest", &asset.name));
        }
    }
    let config = validate_packages(ctx, work.path(), &release, &arch)?;
    ctx.confirm("confirm.kernel_install", &release.kernel, "INSTALL")?;
    install_verified(ctx, work.path(), &release, &config, &running)
}

// The install entry point holds the shared kernel lock through this function and recovery.
pub(crate) fn install_verified(
    ctx: &Context,
    work: &Path,
    release: &Release,
    config: &str,
    running: &str,
) -> Result<()> {
    let backup = backup_grub(ctx, running)?;
    ctx.message("notice.backup", backup.display());
    let result = (|| {
        super::packages::run(
            ctx,
            &[
                "install",
                "-y",
                "--no-remove",
                path(&work.join("image.deb"))?,
                path(&work.join("headers.deb"))?,
            ],
        )?;
        for kind in ["image", "headers"] {
            package_complete(ctx, &format!("linux-{kind}-{}", release.kernel))?;
        }
        nonempty(&ctx.paths.boot.join(format!("vmlinuz-{}", release.kernel)))?;
        let initrd = ctx
            .paths
            .boot
            .join(format!("initrd.img-{}", release.kernel));
        nonempty(&initrd)?;
        ctx.output("lsinitramfs", &[path(&initrd)?])?;
        check_hyperv(ctx, config, Some(path(&initrd)?))?;
        ctx.run("update-grub", &[])
    })();
    if let Err(error) = result {
        if let Err(recovery) = restore_grub(ctx, &backup) {
            return Err(error.with_recovery(recovery, &backup));
        }
        return Err(Error::new(
            "error.kernel_partial",
            format!("{error}; {}", backup.display()),
        ));
    }
    reboot_after_install(ctx, &release.kernel, running)
}

pub(crate) fn reboot_after_install(ctx: &Context, installed: &str, running: &str) -> Result<()> {
    ctx.message("notice.kernel_installed", installed);
    ctx.message("notice.running_kernel", running);
    ctx.message("notice.kernel_verify", installed);
    if ctx.events.is_some() || ctx.interactive {
        // Installing with --yes must not also authorize a host reboot.
        let mut prompt = ctx.clone();
        prompt.yes = false;
        match prompt.confirm("confirm.kernel_reboot", installed, "YES") {
            Ok(()) => return ctx.run("reboot", &[]),
            // Deferring a reboot does not cancel the completed installation.
            Err(error) if error.key == "error.cancelled" => (),
            Err(error) => return Err(error),
        }
    }
    ctx.log(ctx.i18n.text("notice.kernel_reboot_later"));
    Ok(())
}

pub fn validate_removal_plan(plan: &str, release: &str) -> Result<()> {
    for line in plan.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if matches!(parts.first(), Some(&"Remv" | &"Purg"))
            && parts.get(1).is_none_or(|p| {
                *p != format!("linux-image-{release}") && *p != format!("linux-headers-{release}")
            })
        {
            return Err(Error::new("error.kernel_plan", line));
        }
    }
    Ok(())
}

pub(crate) fn schedule_boot(
    ctx: &Context,
    running: &str,
    release: &str,
    entry: &str,
) -> Result<()> {
    let environment = ctx.output("grub-editenv", &["list"])?;
    if let Some(pending) = environment
        .lines()
        .find_map(|line| line.strip_prefix("next_entry="))
        .filter(|value| !value.is_empty())
    {
        return Err(Error::new("error.kernel_boot_pending", pending));
    }

    let reboot_now = ctx.events.is_some() || ctx.interactive;
    if reboot_now {
        let mut prompt = ctx.clone();
        prompt.yes = false;
        prompt.confirm(
            "confirm.kernel_boot_reboot",
            format!("{running} -> {release}"),
            "REBOOT",
        )?;
    } else {
        ctx.confirm(
            "confirm.kernel_boot",
            format!("{running} -> {release}"),
            "BOOT",
        )?;
    }

    let result = (|| {
        ctx.run("grub-reboot", &[&entry])?;
        let environment = ctx.output("grub-editenv", &["list"])?;
        if !environment
            .lines()
            .any(|line| line == format!("next_entry={entry}"))
        {
            return Err(Error::new("error.kernel_boot_verify", &entry));
        }
        ctx.message("notice.kernel_boot", release);
        if reboot_now {
            ctx.run("reboot", &[])
        } else {
            ctx.log(ctx.i18n.text("notice.kernel_reboot_later"));
            Ok(())
        }
    })();
    if let Err(error) = result {
        return match ctx.recovery().run("grub-editenv", &["unset", "next_entry"]) {
            Ok(()) => Err(error),
            Err(recovery) => {
                Err(error.with_recovery(recovery, &ctx.paths.boot.join("grub/grubenv")))
            }
        };
    }
    Ok(())
}

pub fn boot(ctx: &Context, release: &str) -> Result<()> {
    if !valid_kernel_release(release) {
        return Err(Error::new("error.input", release));
    }
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let running = boot_preflight(ctx)?;
    if release == running {
        return Err(Error::new("error.kernel_boot_running", release));
    }
    package_complete(ctx, &format!("linux-image-{release}"))?;
    nonempty(&ctx.paths.boot.join(format!("vmlinuz-{release}")))?;
    nonempty(&ctx.paths.boot.join(format!("initrd.img-{release}")))?;
    let config = fs::read_to_string(ctx.paths.boot.join("grub/grub.cfg"))?;
    let entry = grub_boot_entry(&config, release)?;
    schedule_boot(ctx, &running, release, &entry)
}

pub fn remove(ctx: &Context, release: &str) -> Result<()> {
    if !Regex::new(r"^\d+\.\d+\.\d+-joeyblog-bbrv3(-max)?$")
        .unwrap()
        .is_match(release)
    {
        return Err(Error::new("error.input", release));
    }
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let running = preflight(ctx, false)?;
    if release == running {
        return Err(Error::new("error.running_kernel", release));
    }
    package_complete(ctx, &format!("linux-image-{running}"))?;
    let image = format!("linux-image-{release}");
    let headers = format!("linux-headers-{release}");
    let status = ctx.output("dpkg-query", &["-W", "-f=${db:Status-Abbrev}", &image])?;
    if !status.chars().nth(1).is_some_and(|c| "iUFHWt".contains(c)) {
        return Err(Error::new("error.input", image));
    }
    let mut packages = vec![image.as_str()];
    let status = ctx.command(
        "dpkg-query",
        &["-W", "-f=${db:Status-Abbrev}", &headers],
        false,
    )?;
    if status.status.success()
        && String::from_utf8_lossy(&status.stdout)
            .chars()
            .nth(1)
            .is_some_and(|c| "iUFHWt".contains(c))
    {
        packages.push(&headers);
    }
    let mut args = vec!["-s", "purge"];
    args.extend(&packages);
    let plan = super::packages::output(ctx, &args)?;
    validate_removal_plan(&plan, release)?;
    ctx.log(plan);
    ctx.confirm("confirm.kernel_remove", release, "REMOVE")?;
    let backup = backup_grub(ctx, &running)?;
    let mut args = vec!["purge", "-y"];
    args.extend(packages);
    if let Err(error) = super::packages::run(ctx, &args).and_then(|_| ctx.run("update-grub", &[])) {
        // Restoring a menu referencing deleted packages would make recovery less reliable.
        return Err(Error::new(
            "error.kernel_partial",
            format!("{error}; {}", backup.display()),
        ));
    }
    Ok(())
}
pub fn list(ctx: &Context) -> Result<()> {
    ctx.message("notice.running_kernel", ctx.output("uname", &["-r"])?);
    ctx.run(
        "dpkg-query",
        &[
            "-W",
            "-f=${db:Status-Abbrev}\t${Package}\n",
            "linux-image-*",
        ],
    )
}
