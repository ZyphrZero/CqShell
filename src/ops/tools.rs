use crate::{
    error::{Error, Result},
    runtime::{Context, available, path, sha256},
    storage::{self, Lock},
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::Instant,
};

pub const APT_PACKAGES: &[&str] = &[
    "curl", "wget", "nano", "unzip", "tar", "tmux", "iftop", "btop", "gdu", "fzf", "git",
];

pub const PACKAGES: &[&str] = &[
    "curl", "wget", "nano", "unzip", "tar", "tmux", "iftop", "btop", "gdu", "fzf", "git", "gh",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GhRelease {
    pub name: String,
    pub url: String,
    pub digest: String,
}

pub(crate) fn gh_deb_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" | "amd64" => Ok("amd64"),
        "aarch64" | "arm64" => Ok("arm64"),
        _ => Err(Error::new("error.input", arch)),
    }
}

pub(crate) fn parse_gh_release(metadata: &[u8], deb_arch: &str) -> Result<GhRelease> {
    let value: serde_json::Value = serde_json::from_slice(metadata)?;
    if value.get("draft").is_some_and(|v| v != false)
        || value.get("prerelease").is_some_and(|v| v != false)
    {
        return Err(Error::new("error.release", "gh draft/prerelease"));
    }
    let tag = value["tag_name"]
        .as_str()
        .ok_or_else(|| Error::new("error.release", "gh tag"))?;
    let version = tag
        .strip_prefix('v')
        .filter(|v| {
            !v.is_empty()
                && !v.starts_with('.')
                && !v.contains("..")
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        })
        .ok_or_else(|| Error::new("error.release", tag))?;
    let name = format!("gh_{version}_linux_{deb_arch}.deb");
    let assets = value["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "gh assets"))?;
    let found: Vec<_> = assets.iter().filter(|a| a["name"] == name).collect();
    if found.len() != 1 {
        return Err(Error::new("error.release", name));
    }
    let url = found[0]["browser_download_url"].as_str().unwrap_or("");
    let digest = found[0]["digest"].as_str().unwrap_or("");
    let expected_url = format!("https://github.com/cli/cli/releases/download/{tag}/{name}");
    if url != expected_url
        || !digest.starts_with("sha256:")
        || digest.len() != 71
        || !digest[7..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::new("error.release", name));
    }
    Ok(GhRelease {
        name,
        url: expected_url,
        digest: digest[7..].to_ascii_lowercase(),
    })
}

pub fn packages(ctx: &Context, package: &str) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    if package == "all" {
        let mut args = vec!["install", "-y"];
        args.extend_from_slice(APT_PACKAGES);
        super::packages::run(ctx, &args)?;
        gh_install_unlocked(ctx)?;
        terminal_install_unlocked(ctx)?;
        return Ok(());
    }
    if package == "gh" {
        return gh_install_unlocked(ctx);
    }
    if !APT_PACKAGES.contains(&package) {
        return Err(Error::new("error.input", package));
    }
    super::packages::run(ctx, &["install", "-y", package])
}

fn gh_install_unlocked(ctx: &Context) -> Result<()> {
    if ctx.require(&["curl"]).is_err() {
        super::packages::run(ctx, &["install", "-y", "curl", "ca-certificates"])?;
    }
    ctx.require(&["dpkg-deb"])?;
    let arch = ctx.output("uname", &["-m"])?;
    let deb_arch = gh_deb_arch(&arch)?;
    let work = tempfile::tempdir()?;
    let metadata = work.path().join("release.json");
    ctx.download(
        "https://api.github.com/repos/cli/cli/releases/latest",
        &metadata,
        60,
    )?;
    let release = parse_gh_release(&fs::read(metadata)?, deb_arch)?;
    let deb = work.path().join(&release.name);
    ctx.download(&release.url, &deb, 300)?;
    if sha256(&deb)? != release.digest {
        return Err(Error::new("error.digest", release.name));
    }
    let pkg_name = ctx.output("dpkg-deb", &["-f", path(&deb)?, "Package"])?;
    let pkg_arch = ctx.output("dpkg-deb", &["-f", path(&deb)?, "Architecture"])?;
    if pkg_name != "gh" || pkg_arch != deb_arch {
        return Err(Error::new(
            "error.release",
            format!("{pkg_name} / {pkg_arch}"),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(work.path(), fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&deb, fs::Permissions::from_mode(0o644))?;
    }
    super::packages::run(ctx, &["install", "-y", path(&deb)?])?;
    ctx.run("gh", &["--version"])
}
pub fn terminal_install(ctx: &Context) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    terminal_install_unlocked(ctx)
}
fn terminal_install_unlocked(ctx: &Context) -> Result<()> {
    super::packages::run(ctx, &["install", "-y", "zsh", "curl", "ca-certificates"])?;
    if !available("starship") {
        let arch = ctx.output("uname", &["-m"])?;
        let target = match arch.as_str() {
            "x86_64" => "x86_64-unknown-linux-musl",
            "aarch64" => "aarch64-unknown-linux-musl",
            _ => return Err(Error::new("error.input", arch)),
        };
        let work = tempfile::tempdir()?;
        let metadata = work.path().join("release.json");
        ctx.download(
            "https://api.github.com/repos/starship/starship/releases/latest",
            &metadata,
            60,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&fs::read(metadata)?)?;
        let name = format!("starship-{target}.tar.gz");
        let assets = value["assets"]
            .as_array()
            .ok_or_else(|| Error::new("error.release", "Starship assets"))?;
        let found: Vec<_> = assets.iter().filter(|a| a["name"] == name).collect();
        if found.len() != 1 {
            return Err(Error::new("error.release", name));
        }
        let tag = value["tag_name"]
            .as_str()
            .ok_or_else(|| Error::new("error.release", "Starship tag"))?;
        let url = found[0]["browser_download_url"].as_str().unwrap_or("");
        let digest = found[0]["digest"].as_str().unwrap_or("");
        if url != format!("https://github.com/starship/starship/releases/download/{tag}/{name}")
            || !digest.starts_with("sha256:")
            || digest.len() != 71
            || !digest[7..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::new("error.release", name));
        }
        let archive = work.path().join("starship.tar.gz");
        ctx.download(url, &archive, 300)?;
        if sha256(&archive)? != digest[7..].to_ascii_lowercase() {
            return Err(Error::new("error.digest", name));
        }
        let decoder = flate2::read::GzDecoder::new(fs::File::open(&archive)?);
        let mut tar = tar::Archive::new(decoder);
        let mut binary = None;
        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.path()?.as_ref() == Path::new("starship") {
                if binary.is_some()
                    || !entry.header().entry_type().is_file()
                    || entry.size() > 128 * 1024 * 1024
                {
                    return Err(Error::new("error.release", "Starship archive"));
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                binary = Some(bytes);
            }
        }
        storage::atomic_write(
            Path::new("/usr/local/bin/starship"),
            &binary.ok_or_else(|| Error::new("error.release", "Starship executable"))?,
            0o755,
        )?;
    }
    ctx.run("starship", &["--version"])
}

pub fn remote(ctx: &Context, url: &str) -> Result<()> {
    ctx.confirm("confirm.remote", url)?;
    run_remote_script(ctx, url)
}
fn run_remote_script(ctx: &Context, url: &str) -> Result<()> {
    let work = tempfile::tempdir()?;
    let script = work.path().join("script.sh");
    ctx.download(url, &script, 300)?;
    ctx.run("bash", &["-n", path(&script)?])?;
    ctx.passthrough("bash", &[path(&script)?])
}
pub fn speedtest(ctx: &Context) -> Result<()> {
    const OOKLA_REPO_SCRIPT: &str =
        "https://packagecloud.io/install/repositories/ookla/speedtest-cli/script.deb.sh";
    let needs_install = !available("speedtest");
    if needs_install {
        ctx.root()?;
    }
    let detail = if needs_install {
        format!("Ookla\n{OOKLA_REPO_SCRIPT}")
    } else {
        "Ookla".into()
    };
    ctx.confirm("confirm.speedtest", detail)?;
    if needs_install {
        let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
        run_remote_script(ctx, OOKLA_REPO_SCRIPT)?;
        super::packages::run(ctx, &["install", "-y", "speedtest"])?;
    }
    let output = ctx.output(
        "speedtest",
        &["--accept-license", "--accept-gdpr", "-f", "json"],
    )?;
    let value: serde_json::Value = serde_json::from_str(&output)?;
    for key in ["download", "upload"] {
        let bandwidth = value[key]["bandwidth"]
            .as_f64()
            .ok_or_else(|| Error::new("error.data", key))?;
        ctx.message(
            if key == "download" {
                "notice.download_speed"
            } else {
                "notice.upload_speed"
            },
            format!("{:.2} Mbit/s", bandwidth * 8.0 / 1_000_000.0),
        );
    }
    if let Some(url) = value["result"]["url"].as_str() {
        ctx.log(url);
    }
    Ok(())
}
pub fn connectivity(ctx: &Context) -> Result<()> {
    let mut failed = false;
    for (flag, url) in [
        ("-4", "https://ipv4.icanhazip.com"),
        ("-6", "https://ipv6.icanhazip.com"),
    ] {
        match ctx.output(
            "curl",
            &[flag, "--proto", "=https", "-fSs", "--max-time", "5", url],
        ) {
            Ok(ip) => ctx.message("notice.connectivity", format!("{flag}: {ip}")),
            Err(e) => {
                ctx.log(ctx.i18n.error(&e));
                failed = true;
            }
        }
    }
    match fs::read_to_string("/etc/gai.conf") {
        Ok(config) => {
            for line in config
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("precedence "))
            {
                ctx.log(line);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    ctx.log(ctx.i18n.text("notice.ip_priority"));
    if failed {
        Err(Error::new("error.verify", "IPv4/IPv6"))
    } else {
        Ok(())
    }
}
pub fn io_test(ctx: &Context, directory: &Path) -> Result<()> {
    storage::check_path(directory, false)?;
    let mut speeds = Vec::new();
    let buffer = vec![0u8; 1024 * 1024];
    for round in 1..=3 {
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        let start = Instant::now();
        for _ in 0..1024 {
            ctx.check()?;
            file.write_all(&buffer)?;
        }
        file.as_file().sync_all()?;
        let speed = 1024.0 / start.elapsed().as_secs_f64();
        speeds.push(speed);
        ctx.message("notice.io", format!("{round}/3: {speed:.2} MiB/s"));
    }
    ctx.message(
        "notice.io_average",
        format!("{:.2} MiB/s", speeds.iter().sum::<f64>() / 3.0),
    );
    Ok(())
}
