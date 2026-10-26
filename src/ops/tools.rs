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

pub const PACKAGES: &[&str] = &[
    "curl", "wget", "nano", "unzip", "tar", "tmux", "iftop", "btop", "gdu", "fzf",
];

pub fn packages(ctx: &Context, package: &str) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let chosen = if package == "all" {
        PACKAGES.to_vec()
    } else if PACKAGES.contains(&package) {
        vec![package]
    } else {
        return Err(Error::new("error.input", package));
    };
    let mut args = vec!["install", "-y"];
    args.extend(chosen);
    ctx.run("apt-get", &args)?;
    if package == "all" {
        terminal_install_unlocked(ctx)?;
    }
    Ok(())
}
pub fn terminal_install(ctx: &Context) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    terminal_install_unlocked(ctx)
}
fn terminal_install_unlocked(ctx: &Context) -> Result<()> {
    ctx.run(
        "apt-get",
        &["install", "-y", "zsh", "curl", "ca-certificates"],
    )?;
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
    ctx.confirm("confirm.remote", url, "RUN")?;
    let work = tempfile::tempdir()?;
    let script = work.path().join("script.sh");
    ctx.download(url, &script, 300)?;
    ctx.run("bash", &["-n", path(&script)?])?;
    ctx.passthrough("bash", &[path(&script)?])
}
pub fn speedtest(ctx: &Context) -> Result<()> {
    if !available("speedtest") {
        ctx.root()?;
        let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
        remote(
            ctx,
            "https://packagecloud.io/install/repositories/ookla/speedtest-cli/script.deb.sh",
        )?;
        ctx.run("apt-get", &["install", "-y", "speedtest"])?;
    }
    ctx.confirm("confirm.speedtest", "Ookla", "ACCEPT")?;
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
