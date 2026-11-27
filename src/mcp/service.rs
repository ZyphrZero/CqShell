//! Optional systemd ownership for CqShell's Streamable HTTP MCP server (`cqs-mcp.service`).
//!
//! Mirrors `src/ops/clash/service.rs`: supports `user` and `system` scopes, embeds an
//! instance-path SHA-256 ownership marker, sets `UMask=0077`, and never passes Bearer tokens in
//! unit arguments (`cqs mcp serve` reads `0600` `mcp.token` directly from `CQS_STATE_DIR/mcp/`).

use super::config::{ensure_token, load_config, lock_path, mcp_dir};
use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock},
};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const UNIT: &str = "cqs-mcp.service";
const MARKER: &str = "# CqShell managed MCP HTTP service v1\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            _ => Err(Error::new(
                "error.input",
                "service scope must be user or system",
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::System => "--system",
        }
    }

    pub fn path(self, ctx: &Context) -> PathBuf {
        match self {
            Self::User => ctx.paths.home.join(".config/systemd/user").join(UNIT),
            Self::System => Path::new("/etc/systemd/system").join(UNIT),
        }
    }
}

fn environment(ctx: &Context) -> [(&'static str, &Path); 6] {
    let p = &ctx.paths;
    [
        ("HOME", &p.home),
        ("CQS_STATE_DIR", &p.state),
        ("CQS_CLASH_HOME", &p.clash_home),
        ("CQS_CLASH_DATA_DIR", &p.clash_data),
        ("CQS_CLASH_RESOURCES_DIR", &p.clash_resources),
        ("CQS_CLASH_BINARY", &p.clash_binary),
    ]
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("error.path", path.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn identity(ctx: &Context) -> Result<String> {
    let entries = environment(ctx);
    for (_, p) in entries {
        storage::check_path(p, false)?;
    }
    let instance = [
        ("HOME", path_string(&ctx.paths.home)?),
        ("CQS_STATE_DIR", path_string(&ctx.paths.state)?),
    ];
    Ok(sha256_hex(&serde_json::to_vec(&instance)?))
}

fn header(ctx: &Context) -> Result<String> {
    Ok(format!("{MARKER}# instance-sha256: {}\n", identity(ctx)?))
}

fn owned(ctx: &Context, bytes: &[u8]) -> Result<bool> {
    Ok(bytes.starts_with(header(ctx)?.as_bytes()))
}

fn quote(value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(Error::new("error.path", "invalid systemd argument"));
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

pub(crate) fn render_unit(ctx: &Context, executable: &Path, scope: Scope) -> Result<String> {
    storage::check_path(executable, true)?;
    let exec = quote(&path_string(executable)?)?;
    let mut unit = header(ctx)?;
    unit.push_str(
        "[Unit]\nDescription=CqShell managed MCP Streamable HTTP server\nAfter=network.target\n\n\
         [Service]\nType=simple\nRestart=on-failure\nRestartSec=3s\nTimeoutStopSec=15s\n\
         KillMode=control-group\nUMask=0077\nEnvironment=\"CQS_LANG=en-US\"\n",
    );
    for (k, p) in environment(ctx) {
        unit.push_str(&format!(
            "Environment={}\n",
            quote(&format!("{k}={}", path_string(p)?))?
        ));
    }
    unit.push_str(&format!(
        "ExecStart={exec} --lang en-US --config /dev/null mcp serve\n\n\
         [Install]\nWantedBy={}\n",
        match scope {
            Scope::User => "default.target",
            Scope::System => "multi-user.target",
        }
    ));
    Ok(unit)
}

fn read_unit(ctx: &Context, scope: Scope) -> Result<Option<Vec<u8>>> {
    let path = scope.path(ctx);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    storage::check_path(&path, true)?;
    match fs::read(&path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn preflight(ctx: &Context, scope: Scope) -> Result<()> {
    ctx.platform()?;
    if scope == Scope::System {
        ctx.root()?;
    }
    let release = fs::read_to_string("/proc/sys/kernel/osrelease")?;
    let init = fs::read_to_string("/proc/1/comm")?;
    if release.to_ascii_lowercase().contains("microsoft")
        || init.trim() != "systemd"
        || !Path::new("/run/systemd/system").is_dir()
    {
        return Err(Error::new(
            "error.platform",
            "MCP systemd service management requires native Linux with systemd; run 'cqs mcp serve' directly in this environment",
        ));
    }
    ctx.require(&["systemctl"])?;
    ctx.output(
        "systemctl",
        &[scope.flag(), "show", "--property=Version", "--value"],
    )?;
    Ok(())
}

fn systemctl(ctx: &Context, scope: Scope, args: &[&str]) -> Result<()> {
    let mut cmd_args = vec![scope.flag()];
    cmd_args.extend_from_slice(args);
    ctx.output("systemctl", &cmd_args).map(|_| ())
}

fn systemctl_prop(ctx: &Context, scope: Scope, prop: &str) -> Option<String> {
    ctx.output(
        "systemctl",
        &[
            scope.flag(),
            "show",
            UNIT,
            &format!("--property={prop}"),
            "--value",
        ],
    )
    .ok()
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
}

fn resolve_executable(ctx: &Context) -> Result<PathBuf> {
    if ctx.paths.install.is_file() {
        return Ok(ctx.paths.install.clone());
    }
    let exe = std::env::current_exe()?;
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(Error::new("error.path", exe.display()))
    }
}

/// Non-intrusive, best-effort single-line summary for TUI cards and `ai.mcp.status`.
pub fn brief_status(ctx: &Context) -> String {
    if !Path::new("/run/systemd/system").is_dir() {
        return "foreground-only (no systemd)".into();
    }
    for scope in [Scope::System, Scope::User] {
        let path = scope.path(ctx);
        if path.is_file() {
            let active =
                systemctl_prop(ctx, scope, "ActiveState").unwrap_or_else(|| "installed".into());
            return format!("{} ({})", active, scope.name());
        }
    }
    "not installed".into()
}

pub fn status(ctx: &Context, scope_raw: &str) -> Result<()> {
    let scope = Scope::parse(scope_raw)?;
    let path = scope.path(ctx);
    let unit = read_unit(ctx, scope)?;
    let installed = unit.is_some();
    let is_owned = unit
        .as_deref()
        .is_some_and(|b| owned(ctx, b).unwrap_or(false));

    let mut summary = if !installed {
        format!("not installed ({}, {})", scope.name(), path.display())
    } else if !is_owned {
        format!(
            "foreign or mismatched unit ({}, {})",
            scope.name(),
            path.display()
        )
    } else {
        let active = systemctl_prop(ctx, scope, "ActiveState").unwrap_or_else(|| "unknown".into());
        let enabled =
            systemctl_prop(ctx, scope, "UnitFileState").unwrap_or_else(|| "unknown".into());
        format!(
            "active={active} enabled={enabled} ({}, {})",
            scope.name(),
            path.display()
        )
    };
    if let Ok(cfg) = load_config(ctx) {
        summary.push_str(&format!(
            " | {}{} [{}]",
            cfg.listen_endpoint(),
            cfg.endpoint_path,
            cfg.policy_mode.as_str()
        ));
    }
    ctx.message("notice.mcp_service", summary);
    Ok(())
}

pub fn install(ctx: &Context, scope_raw: &str) -> Result<()> {
    let scope = Scope::parse(scope_raw)?;
    preflight(ctx, scope)?;
    let cfg = load_config(ctx)?;
    cfg.validate()?;
    if cfg.auth_enabled {
        let (_, created) = ensure_token(ctx)?;
        if created {
            ctx.message(
                "notice.mcp_token_ready",
                super::config::token_path(ctx).display(),
            );
        }
    }
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;

    let path = scope.path(ctx);
    if let Some(existing) = read_unit(ctx, scope)?
        && !owned(ctx, &existing)?
    {
        return Err(Error::new(
            "error.config",
            format!("refusing to overwrite non-CqShell unit: {}", path.display()),
        ));
    }
    let exec = resolve_executable(ctx)?;
    let content = render_unit(ctx, &exec, scope)?;
    storage::atomic_write(&path, content.as_bytes(), 0o644)?;
    systemctl(ctx, scope, &["daemon-reload"])?;
    systemctl(ctx, scope, &["enable", "--now", UNIT])?;
    systemctl(ctx, scope, &["restart", UNIT])?;
    ctx.message(
        "notice.mcp_service",
        format!(
            "installed & started ({}, {}) -> {}{}",
            scope.name(),
            path.display(),
            cfg.listen_endpoint(),
            cfg.endpoint_path
        ),
    );
    Ok(())
}

pub fn control(ctx: &Context, scope_raw: &str, verb: &str) -> Result<()> {
    let scope = Scope::parse(scope_raw)?;
    if !matches!(verb, "start" | "stop" | "restart") {
        return Err(Error::new("error.input", verb));
    }
    preflight(ctx, scope)?;
    let existing = read_unit(ctx, scope)?.ok_or_else(|| {
        Error::new(
            "error.config",
            format!("{} is not installed in {} scope", UNIT, scope.name()),
        )
    })?;
    if !owned(ctx, &existing)? {
        return Err(Error::new(
            "error.config",
            format!(
                "refusing to control non-CqShell unit: {}",
                scope.path(ctx).display()
            ),
        ));
    }
    systemctl(ctx, scope, &[verb, UNIT])?;
    ctx.message(
        "notice.mcp_service",
        format!("{verb} completed ({}, {UNIT})", scope.name()),
    );
    Ok(())
}

pub fn remove(ctx: &Context, scope_raw: &str) -> Result<()> {
    let scope = Scope::parse(scope_raw)?;
    preflight(ctx, scope)?;
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;

    let path = scope.path(ctx);
    let Some(existing) = read_unit(ctx, scope)? else {
        ctx.message(
            "notice.mcp_service",
            format!("not installed ({}, {})", scope.name(), path.display()),
        );
        return Ok(());
    };
    if !owned(ctx, &existing)? {
        return Err(Error::new(
            "error.config",
            format!("refusing to remove non-CqShell unit: {}", path.display()),
        ));
    }
    let _ = systemctl(ctx, scope, &["disable", "--now", UNIT]);
    storage::remove_file(&path)?;
    let _ = systemctl(ctx, scope, &["daemon-reload"]);
    ctx.message(
        "notice.mcp_service",
        format!("removed ({}, {})", scope.name(), path.display()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_unit_contains_ownership_marker_umask_and_no_secret_args() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let exec = temp.path().join("cqs");
        fs::write(&exec, b"bin").unwrap();

        let unit = render_unit(&ctx, &exec, Scope::User).unwrap();
        assert!(owned(&ctx, unit.as_bytes()).unwrap());
        assert!(unit.contains("Type=simple"));
        assert!(unit.contains("UMask=0077"));
        assert!(unit.contains("mcp serve"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("cqs_mcp_"));

        let sys_unit = render_unit(&ctx, &exec, Scope::System).unwrap();
        assert!(sys_unit.contains("WantedBy=multi-user.target"));
    }
}
