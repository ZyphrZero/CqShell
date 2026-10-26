use crate::{
    error::{Error, Result},
    runtime::{Context, path},
    storage::{self, Lock},
};
use serde_json::Value;
use std::{fs, io::Read};

pub enum Change<'a> {
    Ipv6(bool),
    Mirror(&'a str),
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
                if !object
                    .get("fixed-cidr-v6")
                    .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                    && !object
                        .get("bip6")
                        .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                {
                    object.insert("fixed-cidr-v6".into(), Value::String(subnet.into()));
                }
            } else {
                for key in ["fixed-cidr-v6", "bip6", "default-gateway-v6"] {
                    object.remove(key);
                }
            }
        }
        Change::Mirror(address) => {
            let url = url::Url::parse(address).map_err(|e| Error::new("error.url", e))?;
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::new(
                    "error.url",
                    "mirror: expected an HTTP(S) URL without credentials, query or fragment",
                ));
            }
            object.insert("registry-mirrors".into(), serde_json::json!([address]));
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
    )
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
