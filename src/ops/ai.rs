use crate::{
    error::{Error, Result},
    runtime::{Context, path, sha256},
    storage::{self, Lock},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const MAX_FILE: u64 = 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 2 * MAX_FILE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pi,
    Claude,
    Codex,
}

impl Tool {
    fn command(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
    fn repository(self) -> Option<&'static str> {
        match self {
            Self::Pi => Some("earendil-works/pi"),
            Self::Codex => Some("openai/codex"),
            Self::Claude => None,
        }
    }
    fn binary(self) -> &'static str {
        match self {
            Self::Pi => "pi/pi",
            Self::Claude => "claude",
            Self::Codex => "bin/codex",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Architecture {
    X64,
    Arm64,
}

impl Architecture {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "x86_64" => Ok(Self::X64),
            "aarch64" => Ok(Self::Arm64),
            _ => Err(Error::new("error.ai_arch", value)),
        }
    }
    fn platform(self) -> &'static str {
        match self {
            Self::X64 => "linux-x64",
            Self::Arm64 => "linux-arm64",
        }
    }
    fn codex_target(self) -> &'static str {
        match self {
            Self::X64 => "x86_64-unknown-linux-musl",
            Self::Arm64 => "aarch64-unknown-linux-musl",
        }
    }
    fn elf_machine(self) -> u16 {
        match self {
            Self::X64 => 62,
            Self::Arm64 => 183,
        }
    }
}

#[derive(Debug)]
struct Package {
    tool: Tool,
    arch: Architecture,
    version: String,
    url: String,
    digest: String,
    size: Option<u64>,
}

fn version(value: &str) -> Result<String> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty() || part.len() > 6 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(Error::new("error.release", value));
    }
    Ok(value.to_owned())
}

fn digest(value: &str) -> Result<String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new("error.release", "SHA-256"));
    }
    Ok(value.to_ascii_lowercase())
}

fn size(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|size| (1..=MAX_FILE).contains(size))
        .ok_or_else(|| Error::new("error.release", "asset size"))
}

fn github_package(tool: Tool, arch: Architecture, release: &Value) -> Result<Package> {
    let repo = tool
        .repository()
        .ok_or_else(|| Error::new("error.release", tool.command()))?;
    let tag = release["tag_name"].as_str().unwrap_or("");
    let prefix = if tool == Tool::Codex { "rust-v" } else { "v" };
    let version = version(tag.strip_prefix(prefix).unwrap_or(""))?;
    if release["draft"] != false || release["prerelease"] != false {
        return Err(Error::new("error.release", tag));
    }
    let name = match tool {
        Tool::Pi => format!("pi-{}.tar.gz", arch.platform()),
        Tool::Codex => format!("codex-package-{}.tar.gz", arch.codex_target()),
        Tool::Claude => unreachable!(),
    };
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "assets"))?;
    let assets: Vec<_> = assets
        .iter()
        .filter(|asset| asset["name"] == name)
        .collect();
    if assets.len() != 1 {
        return Err(Error::new("error.release", name));
    }
    let asset = assets[0];
    let url = format!("https://github.com/{repo}/releases/download/{tag}/{name}");
    if asset["browser_download_url"] != url {
        return Err(Error::new("error.release", name));
    }
    let digest = digest(
        asset["digest"]
            .as_str()
            .and_then(|value| value.strip_prefix("sha256:"))
            .unwrap_or(""),
    )?;
    Ok(Package {
        tool,
        arch,
        version,
        url,
        digest,
        size: Some(size(&asset["size"])?),
    })
}

fn claude_package(arch: Architecture, release: &str, manifest: &Value) -> Result<Package> {
    let version = version(release)?;
    if manifest.get("version").is_some() && manifest["version"] != version {
        return Err(Error::new("error.release", "Claude version"));
    }
    let platform = &manifest["platforms"][arch.platform()];
    let digest = digest(platform["checksum"].as_str().unwrap_or(""))?;
    let size = platform.get("size").map(size).transpose()?;
    Ok(Package {
        tool: Tool::Claude,
        arch,
        url: format!("{CLAUDE_RELEASES}/{version}/{}/claude", arch.platform()),
        version,
        digest,
        size,
    })
}

fn read_metadata(file: &Path, maximum: u64) -> Result<Vec<u8>> {
    if fs::metadata(file)?.len() > maximum {
        return Err(Error::new("error.release", file.display()));
    }
    Ok(fs::read(file)?)
}

fn resolve(ctx: &Context, tool: Tool, arch: Architecture, work: &Path) -> Result<Package> {
    let metadata = work.join("release.json");
    if let Some(repo) = tool.repository() {
        ctx.download(
            &format!("https://api.github.com/repos/{repo}/releases/latest"),
            &metadata,
            60,
        )?;
        github_package(
            tool,
            arch,
            &serde_json::from_slice(&read_metadata(&metadata, 8 * 1024 * 1024)?)?,
        )
    } else {
        let latest = work.join("latest");
        ctx.download(&format!("{CLAUDE_RELEASES}/latest"), &latest, 60)?;
        let text = String::from_utf8(read_metadata(&latest, 64)?)
            .map_err(|error| Error::new("error.release", error))?;
        let version = version(text.trim())?;
        ctx.download(
            &format!("{CLAUDE_RELEASES}/{version}/manifest.json"),
            &metadata,
            60,
        )?;
        claude_package(
            arch,
            &version,
            &serde_json::from_slice(&read_metadata(&metadata, 1024 * 1024)?)?,
        )
    }
}

fn validate_download(package: &Package, archive: &Path) -> Result<()> {
    let actual_size = fs::metadata(archive)?.len();
    if actual_size == 0
        || actual_size > MAX_FILE
        || package.size.is_some_and(|expected| actual_size != expected)
    {
        return Err(Error::new("error.release", "asset size"));
    }
    if sha256(archive)? != package.digest {
        return Err(Error::new("error.digest", &package.url));
    }
    Ok(())
}

fn unpack(ctx: &Context, archive: &Path, destination: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(File::open(archive)?);
    let mut archive = tar::Archive::new(decoder);
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for entry in archive.entries()? {
        ctx.check()?;
        let mut entry = entry?;
        let name = entry.path()?.into_owned();
        if name.as_os_str().is_empty()
            || name
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || name
                .to_string_lossy()
                .chars()
                .any(|c| c.is_control() || matches!(c, '\\' | ':'))
            || !seen.insert(name.clone())
            || seen.len() > 20_000
        {
            return Err(Error::new("error.release", name.display()));
        }
        let target = destination.join(&name);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            let size = entry.size();
            total = total
                .checked_add(size)
                .ok_or_else(|| Error::new("error.release", "archive size"))?;
            if size > MAX_FILE || total > MAX_UNPACKED {
                return Err(Error::new("error.release", "archive size"));
            }
            fs::create_dir_all(
                target
                    .parent()
                    .ok_or_else(|| Error::new("error.path", target.display()))?,
            )?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            let mut buffer = [0u8; 65536];
            loop {
                ctx.check()?;
                let count = entry.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                file.write_all(&buffer[..count])?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if entry.header().mode()? & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                };
                file.set_permissions(fs::Permissions::from_mode(mode))?;
            }
            file.sync_all()?;
        } else {
            // Never extract links, devices, FIFOs or sparse entries from remote packages.
            return Err(Error::new("error.release", name.display()));
        }
    }
    Ok(())
}

fn elf(binary: &Path, arch: Architecture) -> Result<()> {
    let mut header = [0u8; 20];
    File::open(binary)?.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF"
        || header[4..7] != [2, 1, 1]
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || u16::from_le_bytes([header[18], header[19]]) != arch.elf_machine()
    {
        return Err(Error::new("error.release", binary.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn validate_layout(package: &Package, root: &Path) -> Result<PathBuf> {
    let binary = root.join(package.tool.binary());
    elf(&binary, package.arch)?;
    match package.tool {
        Tool::Pi => {
            let metadata: Value = serde_json::from_slice(&read_metadata(
                &root.join("pi/package.json"),
                1024 * 1024,
            )?)?;
            if metadata["version"] != package.version
                || !root.join("pi/photon_rs_bg.wasm").is_file()
                || !root.join("pi/theme").is_dir()
                || !root.join("pi/assets").is_dir()
                || !root.join("pi/export-html").is_dir()
            {
                return Err(Error::new("error.release", "Pi resources"));
            }
        }
        Tool::Codex => {
            let metadata: Value = serde_json::from_slice(&read_metadata(
                &root.join("codex-package.json"),
                1024 * 1024,
            )?)?;
            if metadata["layoutVersion"] != 1
                || metadata["version"] != package.version
                || metadata["target"] != package.arch.codex_target()
                || metadata["variant"] != "codex"
                || metadata["entrypoint"] != "bin/codex"
                || metadata["resourcesDir"] != "codex-resources"
                || metadata["pathDir"] != "codex-path"
            {
                return Err(Error::new("error.release", "Codex package layout"));
            }
            for helper in [
                "bin/codex-code-mode-host",
                "codex-resources/bwrap",
                "codex-path/rg",
            ] {
                elf(&root.join(helper), package.arch)?;
            }
        }
        Tool::Claude => (),
    }
    Ok(binary)
}

fn environment(ctx: &Context, name: &str) -> Option<String> {
    ctx.environment
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
}

fn user_context(ctx: &Context) -> Result<Context> {
    storage::check_path(&ctx.paths.home, false)?;
    let mut ctx = ctx.clone();
    ctx.environment
        .insert("HOME".into(), path(&ctx.paths.home)?.into());
    Ok(ctx)
}

fn launcher(ctx: &Context, tool: Tool) -> PathBuf {
    let directory = if tool == Tool::Codex {
        environment(ctx, "CODEX_INSTALL_DIR").map(PathBuf::from)
    } else {
        None
    };
    directory
        .unwrap_or_else(|| ctx.paths.home.join(".local/bin"))
        .join(tool.command())
}

fn check_launcher(target: &Path) -> Result<()> {
    storage::check_path(
        target
            .parent()
            .ok_or_else(|| Error::new("error.path", target.display()))?,
        false,
    )?;
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(Error::new("error.path", target.display())),
        Err(error) => Err(error.into()),
    }
}

fn verify_version(ctx: &Context, binary: &Path, expected: Option<&str>) -> Result<()> {
    let output = ctx.output(path(binary)?, &["--version"])?;
    if output.trim().is_empty()
        || expected.is_some_and(|version| !output.split_whitespace().any(|token| token == version))
    {
        return Err(Error::new(
            "error.verify",
            format!(
                "{}: expected {}, got {output:?}",
                binary.display(),
                expected.unwrap_or("--version")
            ),
        ));
    }
    ctx.message(
        "notice.ai_version",
        format!("{}: {output}", binary.display()),
    );
    Ok(())
}

#[cfg(unix)]
fn publish(ctx: &Context, binary: &Path, target: &Path, version: &str) -> Result<()> {
    use std::os::unix::fs::symlink;
    check_launcher(target)?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::new("error.path", target.display()))?;
    fs::create_dir_all(parent)?;
    let recovery = tempfile::Builder::new()
        .prefix(".cqs-ai-backup-")
        .tempdir_in(parent)?;
    let previous = recovery.path().join("previous");
    let had_previous = match fs::symlink_metadata(target) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                symlink(fs::read_link(target)?, &previous)?;
            } else {
                fs::hard_link(target, &previous)?;
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let next = recovery.path().join("next");
    symlink(binary, &next)?;
    storage::sync_parent(&next)?;
    ctx.check()?;
    // Replace the entrypoint itself, never the target of an existing symlink.
    fs::rename(&next, target)?;
    let result =
        storage::sync_parent(target).and_then(|()| verify_version(ctx, target, Some(version)));
    if let Err(error) = result {
        let restored = if had_previous {
            fs::rename(&previous, target)
        } else {
            fs::remove_file(target)
        };
        if let Err(restore) = restored
            .map_err(Error::from)
            .and_then(|()| storage::sync_parent(target))
        {
            let recovery = recovery.keep();
            return Err(Error::new(
                "error.recovery",
                format!("{error}; {restore}; {}", recovery.display()),
            ));
        }
        return Err(error);
    }
    if had_previous {
        ctx.message("notice.ai_backup", recovery.keep().display());
    }
    Ok(())
}

#[cfg(not(unix))]
fn publish(_ctx: &Context, _binary: &Path, _target: &Path, _version: &str) -> Result<()> {
    Err(Error::new("error.platform", std::env::consts::OS))
}

pub fn install(ctx: &Context, tool: Tool) -> Result<()> {
    ctx.check()?;
    ctx.platform()?;
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0
        && environment(ctx, "SUDO_USER").is_some_and(|user| user != "root")
    {
        return Err(Error::new("error.ai_sudo", tool.command()));
    }
    let arch = Architecture::parse(std::env::consts::ARCH)?;
    let ctx = user_context(ctx)?;
    let target = launcher(&ctx, tool);
    check_launcher(&target)?;
    ctx.require(&["curl"])?;
    let _lock = Lock::acquire(&ctx.paths.state.join("ai/install.lock"))?;
    let work = tempfile::tempdir()?;
    let package = resolve(&ctx, tool, arch, work.path())?;
    ctx.confirm(
        "confirm.ai_install",
        format!(
            "{} {}\n{}\nSHA-256: {}\n{}",
            tool.command(),
            package.version,
            package.url,
            package.digest,
            target.display()
        ),
        "YES",
    )?;
    let archive = work.path().join("package");
    ctx.download(&package.url, &archive, 900)?;
    validate_download(&package, &archive)?;
    let releases = ctx
        .paths
        .home
        .join(".local/share/cqs/ai")
        .join(tool.command());
    storage::private_dir(&releases)?;
    let staged = tempfile::Builder::new()
        .prefix(&format!("{}-{}-", package.version, arch.platform()))
        .tempdir_in(&releases)?;
    if tool == Tool::Claude {
        fs::copy(&archive, staged.path().join(tool.binary()))?;
    } else {
        unpack(&ctx, &archive, staged.path())?;
    }
    let binary = validate_layout(&package, staged.path())?;
    File::open(&binary)?.sync_all()?;
    verify_version(&ctx, &binary, Some(&package.version))?;
    ctx.check()?;
    // Keep immutable files before publishing: failed recovery must never drop
    // a directory to which a live entrypoint or running process may refer.
    let _release = staged.keep();
    publish(&ctx, &binary, &target, &package.version)?;
    ctx.message("notice.ai_ready", target.display());
    Ok(())
}

fn installed_command(ctx: &Context, tool: Tool) -> Result<Option<PathBuf>> {
    let mut candidates = vec![launcher(ctx, tool)];
    if tool == Tool::Pi {
        let agent = environment(ctx, "PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.paths.home.join(".pi/agent"));
        candidates.push(agent.join("bin/pi"));
    }
    for candidate in candidates {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    match ctx.require(&[tool.command()]) {
        Ok(()) => Ok(Some(tool.command().into())),
        Err(error) if error.key == "error.dependency" => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn status(ctx: &Context) -> Result<()> {
    let ctx = user_context(ctx)?;
    let mut failures = Vec::new();
    for tool in [Tool::Pi, Tool::Claude, Tool::Codex] {
        ctx.check()?;
        let Some(binary) = installed_command(&ctx, tool)? else {
            ctx.message("notice.ai_missing", tool.command());
            continue;
        };
        if let Err(error) = verify_version(&ctx, &binary, None) {
            if error.key == "error.cancelled" {
                return Err(error);
            }
            ctx.log(ctx.i18n.error(&error));
            failures.push(tool.command());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new("error.verify", failures.join(", ")))
    }
}

#[cfg(test)]
mod tests;
