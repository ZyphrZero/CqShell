//! Docker daemon configuration, installation/uninstallation, and native CLI management backend.

pub mod actions;
pub mod client;
pub mod model;
pub mod monitor;
#[cfg(test)]
mod tests;

use crate::{
    error::{Error, Result},
    runtime::{Context, path},
    storage::{self, Lock},
};
use serde_json::Value;
use std::{collections::BTreeSet, fs, io::Read, path::Path};

/// Curated public Docker Hub mirrors in China, ordered for automatic failover.
pub const PRESET_RECOMMENDED: &[&str] = &[
    "https://docker.1ms.run",
    "https://docker.xuanyuan.me",
    "https://docker.m.daocloud.io",
    "https://docker.1panel.live",
];

/// Choices offered in the `docker.mirror` form and CLI help (`allow_custom = true`).
pub const MIRROR_PRESET_CHOICES: &[&str] = &[
    "recommended",
    "1ms",
    "xuanyuan",
    "daocloud",
    "1panel",
    "clear",
];

pub enum Change<'a> {
    Ipv6(bool),
    Mirror(&'a str),
}

fn validate_mirror_url(address: &str) -> Result<String> {
    if address.is_empty() || address.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::new(
            "error.url",
            "mirror: expected an HTTP(S) URL without credentials, query or fragment",
        ));
    }
    let url = url::Url::parse(address).map_err(|_| {
        Error::new(
            "error.url",
            "mirror: expected an HTTP(S) URL without credentials, query or fragment",
        )
    })?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(Error::new(
            "error.url",
            "mirror: expected an HTTP(S) URL without credentials, query or fragment",
        ));
    }
    Ok(address.to_owned())
}

/// Parses a `docker.mirror` specification into a deduplicated, ordered list of mirror URLs.
///
/// Supports:
/// - Clear keywords (`clear`, `off`, `none`, `direct`) -> empty `Vec` (removes `registry-mirrors`)
/// - Preset keywords (`recommended`/`china`/`default`, `1ms`, `xuanyuan`, `daocloud`, `1panel`)
/// - One or more custom HTTP(S) URLs separated by commas, semicolons, spaces, or newlines
/// - Mixed lists combining presets and custom URLs (e.g. `1ms,https://mirror.example.com`)
pub fn parse_mirrors(raw: &str) -> Result<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::new("error.required", "mirror"));
    }
    let tokens: Vec<&str> = trimmed
        .split(|c: char| matches!(c, ',' | ';' | '\n' | '\r') || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(Error::new("error.required", "mirror"));
    }
    let is_clear = |token: &str| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "clear" | "off" | "none" | "direct"
        )
    };
    if tokens.len() == 1 && is_clear(tokens[0]) {
        return Ok(Vec::new());
    }
    if tokens.iter().any(|t| is_clear(t)) {
        return Err(Error::new(
            "error.input",
            "mirror: cannot combine clear/off/direct with mirror URLs",
        ));
    }

    let mut mirrors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push_unique = |url: String| {
        let key = url.trim_end_matches('/').to_ascii_lowercase();
        if seen.insert(key) {
            mirrors.push(url);
        }
    };

    for token in tokens {
        match token.to_ascii_lowercase().as_str() {
            "recommended" | "china" | "default" => {
                for &preset in PRESET_RECOMMENDED {
                    push_unique(preset.to_owned());
                }
            }
            "1ms" => push_unique(PRESET_RECOMMENDED[0].to_owned()),
            "xuanyuan" => push_unique(PRESET_RECOMMENDED[1].to_owned()),
            "daocloud" => push_unique(PRESET_RECOMMENDED[2].to_owned()),
            "1panel" => push_unique(PRESET_RECOMMENDED[3].to_owned()),
            _ => push_unique(validate_mirror_url(token)?),
        }
    }
    Ok(mirrors)
}

/// Reads currently configured `registry-mirrors` from `daemon.json`, returning an empty list if
/// the file does not exist or cannot be parsed.
pub fn configured_mirrors(path: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("registry-mirrors")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn candidate(text: &str, change: Change<'_>, subnet: &str) -> Result<String> {
    let mut value: Value = serde_json::from_str(text)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::new("error.data", "daemon.json: expected object"))?;
    match change {
        Change::Ipv6(on) => {
            object.insert("ipv6".into(), Value::Bool(on));
            if on {
                if object
                    .get("fixed-cidr-v6")
                    .is_none_or(|v| v.as_str().is_none_or(str::is_empty))
                    && object
                        .get("bip6")
                        .is_none_or(|v| v.as_str().is_none_or(str::is_empty))
                {
                    object.insert("fixed-cidr-v6".into(), Value::String(subnet.into()));
                }
            } else {
                for key in ["fixed-cidr-v6", "bip6", "default-gateway-v6"] {
                    object.remove(key);
                }
            }
        }
        Change::Mirror(raw) => {
            let mirrors = parse_mirrors(raw)?;
            if mirrors.is_empty() {
                object.remove("registry-mirrors");
            } else {
                object.insert("registry-mirrors".into(), serde_json::json!(mirrors));
            }
        }
    }
    Ok(serde_json::to_string_pretty(&value)? + "\n")
}

pub fn configure(ctx: &Context, change: Change<'_>) -> Result<()> {
    ctx.require(&["dockerd", "systemctl"])?;
    let target = &ctx.paths.docker;
    let _lock = Lock::acquire(&storage::suffix(target, ".cqs-lock"))?;
    storage::check_path(target, true)?;
    let old = match fs::read_to_string(target) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.into()),
    };
    let mut random = [0u8; 5];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let subnet = format!(
        "fd{:02x}:{:02x}{:02x}:{:02x}{:02x}:1::/64",
        random[0], random[1], random[2], random[3], random[4]
    );
    let mirror_summary = match change {
        Change::Mirror(raw) => Some(parse_mirrors(raw)?),
        Change::Ipv6(_) => None,
    };
    let data = candidate(&old, change, &subnet)?;
    let active = ctx.active("docker")?;
    super::service::update(
        ctx,
        target,
        data.as_bytes(),
        "docker",
        active,
        |p| ctx.run("dockerd", &["--validate", "--config-file", path(p)?]),
        || {
            ctx.run("systemctl", &["restart", "docker"])?;
            if !ctx.active("docker")? {
                return Err(Error::new("error.verify", "docker"));
            }
            Ok(())
        },
    )?;
    if let Some(mirrors) = mirror_summary {
        if mirrors.is_empty() {
            ctx.message("notice.docker_mirror_cleared", target.display());
        } else {
            ctx.message("notice.docker_mirror_set", mirrors.join(", "));
        }
    }
    Ok(())
}

pub fn status(ctx: &Context) -> Result<()> {
    for args in [
        &["version"][..],
        &["compose", "version"],
        &["image", "ls"],
        &["ps", "-a"],
        &["volume", "ls"],
        &["network", "ls"],
    ] {
        ctx.run("docker", args)?;
    }
    match fs::read_to_string(&ctx.paths.docker) {
        Ok(data) => {
            let v: Value = serde_json::from_str(&data)?;
            if let Some(mirrors) = v.get("registry-mirrors") {
                ctx.log(mirrors.to_string());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

pub fn install(ctx: &Context, source: &str) -> Result<()> {
    if crate::runtime::available("docker") {
        return ctx.run("docker", &["version"]);
    }
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let url = match source {
        "official" => "https://get.docker.com",
        "china" => "https://linuxmirrors.cn/docker.sh",
        _ => return Err(Error::new("error.input", source)),
    };
    super::tools::remote(ctx, url)?;
    ctx.run("docker", &["version"])
}

pub fn uninstall(ctx: &Context, purge_data: bool) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let mut packages = Vec::new();
    for package in [
        "docker-ce",
        "docker-ce-cli",
        "docker.io",
        "docker-engine",
        "docker",
        "docker-buildx-plugin",
        "docker-compose-plugin",
        "containerd.io",
    ] {
        let out = ctx.command(
            "dpkg-query",
            &["-W", "-f=${db:Status-Abbrev}", package],
            false,
        )?;
        if out.status.success()
            && String::from_utf8_lossy(&out.stdout)
                .chars()
                .nth(1)
                .is_some_and(|c| "iUFHWt".contains(c))
        {
            packages.push(package);
        }
    }
    if packages.is_empty() {
        return Err(Error::new("error.dependency", "Docker packages"));
    }
    ctx.run("systemctl", &["stop", "docker.service", "docker.socket"])?;
    let mut args = vec!["purge", "-y"];
    args.extend(packages);
    super::packages::run(ctx, &args)?;
    if purge_data {
        // Explicit data deletion is restricted to the known default locations.
        for name in ["/var/lib/docker", "/etc/docker"] {
            let target = std::path::Path::new(name);
            storage::check_path(target, false)?;
            if target.exists() {
                fs::remove_dir_all(target)?;
            }
        }
    }
    Ok(())
}
