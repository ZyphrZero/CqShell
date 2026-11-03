//! Optional systemd ownership of the existing PID-checked Mihomo lifecycle.
//!
//! A forking unit invokes CqShell rather than inventing a second config/PID
//! format. Direct and systemd starts therefore use the same operation lock.
use super::*;
use serde::{Deserialize, Serialize};

const UNIT: &str = "cqs-mihomo.service";
const MARKER: &str = "# CqShell managed Mihomo service v1\n";
const INVOKED: &str = "CQS_CLASH_SERVICE_INVOKED";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    User,
    System,
}

impl Scope {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            _ => Err(Error::new(
                "error.input",
                "service scope must be user or system",
            )),
        }
    }

    fn name(self) -> &'static str {
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

    fn path(self, ctx: &Context) -> PathBuf {
        match self {
            Self::User => ctx.paths.home.join(".config/systemd/user").join(UNIT),
            Self::System => Path::new("/etc/systemd/system").join(UNIT),
        }
    }
}

fn environment(ctx: &Context) -> [(&'static str, &Path); 15] {
    let p = &ctx.paths;
    [
        ("HOME", &p.home),
        ("CQS_STATE_DIR", &p.state),
        ("CQS_CLASH_HOME", &p.clash_home),
        ("CQS_CLASH_DATA_DIR", &p.clash_data),
        ("CQS_CLASH_RESOURCES_DIR", &p.clash_resources),
        ("CQS_CLASH_BINARY", &p.clash_binary),
        ("CQS_CLASH_CONFIG", &p.clash_config),
        ("CQS_CLASH_MIXIN", &p.clash_mixin),
        ("CQS_CLASH_RUNTIME", &p.clash_runtime),
        ("CQS_CLASH_PROFILES", &p.clash_profiles),
        ("CQS_CLASH_PID", &p.clash_pid),
        ("CQS_CLASH_PID_START", &p.clash_pid_start),
        ("CQS_CLASH_LOG", &p.clash_log),
        ("CQS_CLASH_LOCK", &p.clash_lock),
        ("CQS_TERMINAL_CONFIG", &p.terminal),
    ]
}

fn identity(ctx: &Context) -> Result<String> {
    let entries = environment(ctx);
    for (_, path) in entries {
        storage::check_path(path, false)?;
    }
    // Unrelated CqShell state/terminal preferences must not make a running
    // Mihomo instance lose its service ownership after an environment change.
    let instance = entries
        .into_iter()
        .filter(|(name, _)| *name == "HOME" || name.starts_with("CQS_CLASH_"))
        .collect::<Vec<_>>();
    Ok(sha256_hex(&serde_json::to_vec(&instance)?))
}

fn header(ctx: &Context) -> Result<String> {
    Ok(format!("{MARKER}# instance-sha256: {}\n", identity(ctx)?))
}

fn owned(ctx: &Context, bytes: &[u8]) -> Result<bool> {
    Ok(bytes.starts_with(header(ctx)?.as_bytes()))
}

fn read_unit(ctx: &Context, scope: Scope) -> Result<Option<Vec<u8>>> {
    let path = scope.path(ctx);
    match fs::symlink_metadata(&path) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    storage::check_path(&path, true)?;
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn quote(value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(Error::new("error.path", "invalid systemd argument"));
    }
    let value = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    // Environment assignments and the first ExecStart executable token both
    // preserve dollars literally. Escaping them as $$ changes the binary path.
    Ok(format!("\"{value}\""))
}

fn render(ctx: &Context, executable: &Path, scope: Scope) -> Result<String> {
    storage::check_path(executable, true)?;
    let executable = quote(&path_string(executable)?)?;
    let pid = path_string(&ctx.paths.clash_pid)?;
    // PIDFile is a path assignment, not an Exec command argument. Interior
    // spaces stay literal; surrounding whitespace/line continuations do not.
    if pid.trim() != pid || pid.ends_with('\\') {
        return Err(Error::new("error.path", "unsupported PIDFile suffix"));
    }
    let mut unit = header(ctx)?;
    unit.push_str("[Unit]\nDescription=CqShell managed Mihomo\nAfter=network.target\n\n[Service]\nType=forking\n");
    unit.push_str(&format!("PIDFile={}\n", pid.replace('%', "%%")));
    unit.push_str("GuessMainPID=no\nRestart=on-failure\nRestartSec=3s\nTimeoutStartSec=90s\nTimeoutStopSec=15s\nKillMode=control-group\nUMask=0077\n");
    unit.push_str(&format!("Environment=\"{INVOKED}=1\" \"CQS_LANG=en-US\"\n"));
    for (key, path) in environment(ctx) {
        unit.push_str(&format!(
            "Environment={}\n",
            quote(&format!("{key}={}", path_string(path)?))?
        ));
    }
    // A changed interactive preference/locale file cannot break boot or stop.
    // /dev/null is an explicitly selected, empty TOML preferences source.
    unit.push_str(&format!("ExecStart={executable} --yes --lang en-US --config /dev/null run clash.start\nExecStop={executable} --yes --lang en-US --config /dev/null run clash.stop\n\n[Install]\nWantedBy={}\n", match scope { Scope::User => "default.target", Scope::System => "multi-user.target" }));
    Ok(unit)
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
            "Mihomo service management requires native Linux with systemd; use direct start/stop in this environment",
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
    let mut arguments = vec![scope.flag()];
    arguments.extend_from_slice(args);
    ctx.output("systemctl", &arguments).map(|_| ())
}

fn active(ctx: &Context, scope: Scope) -> Result<bool> {
    let value = ctx.output(
        "systemctl",
        &[
            scope.flag(),
            "show",
            UNIT,
            "--property=ActiveState",
            "--value",
        ],
    )?;
    match value.as_str() {
        "active" | "reloading" => Ok(true),
        "inactive" | "failed" => Ok(false),
        "activating" | "deactivating" | "maintenance" | "refreshing" => Err(Error::new(
            "error.busy",
            format!("Mihomo systemd service is {value}; retry after it settles"),
        )),
        _ => Err(Error::new(
            "error.command",
            format!("unsupported systemd activity: {value}"),
        )),
    }
}

fn enabled(ctx: &Context, scope: Scope) -> Result<bool> {
    let out = ctx.command("systemctl", &[scope.flag(), "is-enabled", UNIT], false)?;
    let value = String::from_utf8_lossy(&out.stdout);
    match (out.status.code(), value.trim()) {
        (Some(0), "enabled") => Ok(true),
        (Some(1), "disabled" | "not-found") => Ok(false),
        (Some(1 | 4), "")
            if ctx.output(
                "systemctl",
                &[
                    scope.flag(),
                    "show",
                    UNIT,
                    "--property=LoadState",
                    "--value",
                ],
            )? == "not-found" =>
        {
            Ok(false)
        }
        (Some(4), "not-found") => Ok(false),
        _ => Err(Error::new(
            "error.command",
            format!(
                "unsupported systemd enable state: {} ({})",
                value.trim(),
                out.status
            ),
        )),
    }
}

fn check_fragment(ctx: &Context, scope: Scope, existing: bool) -> Result<()> {
    let out = ctx.output(
        "systemctl",
        &[
            scope.flag(),
            "show",
            UNIT,
            "--property=LoadState,FragmentPath,DropInPaths",
        ],
    )?;
    let properties = out
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect::<BTreeMap<_, _>>();
    if properties
        .get("DropInPaths")
        .is_some_and(|value| !value.is_empty())
    {
        return Err(Error::new(
            "error.busy",
            "remove systemd drop-in overrides before managing this unit",
        ));
    }
    let fragment = properties.get("FragmentPath").copied().unwrap_or("");
    if !fragment.is_empty() && fragment != path_string(&scope.path(ctx))? {
        return Err(Error::new(
            "error.busy",
            format!("{UNIT} is provided by another unit file: {fragment}"),
        ));
    }
    if existing && fragment.is_empty() {
        return Err(Error::new(
            "error.verify",
            "managed service is not loaded; run service install to reload it",
        ));
    }
    Ok(())
}

fn pending(ctx: &Context) -> PathBuf {
    ctx.paths.clash_data.join("service-pending.json")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Before {
    instance: String,
    scope: String,
    file: PathBuf,
    content: Option<storage::Saved>,
    enabled: bool,
    active: bool,
}

fn restore(ctx: &Context, before: &Before) -> Result<()> {
    let scope = Scope::parse(&before.scope)?;
    if before.instance != identity(ctx)? || before.file != scope.path(ctx) {
        return Err(Error::new(
            "error.pending",
            "service recovery record belongs to another instance",
        ));
    }
    if let Some(bytes) = read_unit(ctx, scope)?
        && !owned(ctx, &bytes)?
    {
        return Err(Error::new(
            "error.pending",
            "service file changed outside CqShell; recovery record preserved",
        ));
    }
    if let Some(saved) = &before.content
        && !owned(ctx, &saved.bytes()?)?
    {
        return Err(Error::new(
            "error.pending",
            "untrusted service before-image",
        ));
    }
    let recovery = ctx.recovery();
    // Stop while the currently installed ExecStop still exists, before removing
    // a newly installed unit or restoring its previous definition.
    if active(&recovery, scope)? {
        systemctl(&recovery, scope, &["stop", UNIT])?;
    }
    if read_unit(ctx, scope)?.is_some() {
        systemctl(&recovery, scope, &["disable", UNIT])?;
    }
    storage::Saved::write(&before.file, &before.content)?;
    systemctl(&recovery, scope, &["daemon-reload"])?;
    if before.enabled {
        systemctl(&recovery, scope, &["enable", UNIT])?;
    }
    if before.active {
        systemctl(&recovery, scope, &["start", UNIT])?;
    }
    if active(&recovery, scope)? != before.active {
        return Err(Error::new(
            "error.verify",
            "service recovery did not restore activity",
        ));
    }
    storage::remove_file(&pending(ctx))
}

fn recover(ctx: &Context) -> Result<()> {
    let path = pending(ctx);
    storage::check_path(&path, true)?;
    if !path.exists() {
        return Ok(());
    }
    if fs::metadata(&path)?.len() > 128 * 1024 {
        return Err(Error::new(
            "error.pending",
            "service recovery record is too large",
        ));
    }
    let before: Before = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|error| Error::new("error.pending", error))?;
    preflight(ctx, Scope::parse(&before.scope)?)?;
    restore(ctx, &before)
}

fn transaction(
    ctx: &Context,
    before: &Before,
    operation: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let journal = pending(ctx);
    storage::atomic_write(&journal, &serde_json::to_vec_pretty(before)?, 0o600)?;
    match operation().and_then(|()| ctx.check()) {
        Ok(()) => storage::remove_file(&journal),
        Err(error) => match restore(ctx, before) {
            Ok(()) => Err(error),
            Err(recovery) => Err(error.with_recovery(recovery, &journal)),
        },
    }
}

pub fn status(ctx: &Context, scope: &str) -> Result<()> {
    let scope = Scope::parse(scope)?;
    preflight(ctx, scope)?;
    if pending(ctx).exists() {
        return Err(Error::new("error.pending", pending(ctx).display()));
    }
    let Some(bytes) = read_unit(ctx, scope)? else {
        ctx.message(
            "notice.clash_service",
            format!("{}: not configured", scope.name()),
        );
        return Ok(());
    };
    if !owned(ctx, &bytes)? {
        return Err(Error::new(
            "error.busy",
            "service unit is not owned by this CqShell instance",
        ));
    }
    check_fragment(ctx, scope, true)?;
    ctx.message(
        "notice.clash_service",
        format!(
            "{}; active={}; enabled={}; {}",
            scope.name(),
            active(ctx, scope)?,
            enabled(ctx, scope)?,
            scope.path(ctx).display()
        ),
    );
    Ok(())
}

pub fn install(ctx: &Context, scope: &str) -> Result<()> {
    let scope = Scope::parse(scope)?;
    preflight(ctx, scope)?;
    ensure_layout(ctx)?;
    let _service_lock = Lock::acquire(&storage::suffix(&ctx.paths.clash_lock, ".service"))?;
    recover(ctx)?;
    let _operation_lock = Lock::acquire(&ctx.paths.clash_lock)?;
    if matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo before installing or replacing its service",
        ));
    }
    require_binary(ctx)?;
    let other = if scope == Scope::User {
        Scope::System
    } else {
        Scope::User
    };
    if let Some(bytes) = read_unit(ctx, other)?
        && owned(ctx, &bytes)?
    {
        return Err(Error::new(
            "error.busy",
            "remove the other service scope for this instance first",
        ));
    }
    let existing = read_unit(ctx, scope)?;
    if let Some(bytes) = &existing
        && !owned(ctx, bytes)?
    {
        return Err(Error::new(
            "error.busy",
            "refusing to overwrite an unowned service unit",
        ));
    }
    check_fragment(ctx, scope, false)?;
    if active(ctx, scope)? {
        return Err(Error::new(
            "error.busy",
            "stop the existing systemd unit before replacing it",
        ));
    }
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    let unit = render(ctx, &executable, scope)?;
    let before = Before {
        instance: identity(ctx)?,
        scope: scope.name().into(),
        file: scope.path(ctx),
        content: storage::Saved::capture(&scope.path(ctx))?,
        enabled: enabled(ctx, scope)?,
        active: false,
    };
    transaction(ctx, &before, || {
        storage::atomic_write(&before.file, unit.as_bytes(), 0o644)?;
        systemctl(ctx, scope, &["daemon-reload"])?;
        check_fragment(ctx, scope, true)?;
        systemctl(ctx, scope, &["enable", UNIT])?;
        if !enabled(ctx, scope)? {
            return Err(Error::new("error.verify", "service was not enabled"));
        }
        Ok(())
    })?;
    ctx.message(
        "notice.clash_service",
        format!(
            "{}: installed and enabled; start Mihomo to run it",
            scope.name()
        ),
    );
    Ok(())
}

pub fn remove(ctx: &Context, scope: &str) -> Result<()> {
    let scope = Scope::parse(scope)?;
    preflight(ctx, scope)?;
    ensure_layout(ctx)?;
    let _service_lock = Lock::acquire(&storage::suffix(&ctx.paths.clash_lock, ".service"))?;
    recover(ctx)?;
    let Some(bytes) = read_unit(ctx, scope)? else {
        return Err(Error::new("error.input", "service is not configured"));
    };
    if !owned(ctx, &bytes)? {
        return Err(Error::new(
            "error.busy",
            "refusing to remove an unowned service unit",
        ));
    }
    check_fragment(ctx, scope, true)?;
    let before = Before {
        instance: identity(ctx)?,
        scope: scope.name().into(),
        file: scope.path(ctx),
        content: storage::Saved::capture(&scope.path(ctx))?,
        enabled: enabled(ctx, scope)?,
        active: active(ctx, scope)?,
    };
    transaction(ctx, &before, || {
        systemctl(ctx, scope, &["disable", "--now", UNIT])?;
        if active(ctx, scope)? || matches!(state(ctx)?, State::Running(_) | State::Unknown) {
            return Err(Error::new(
                "error.verify",
                "Mihomo did not stop; service file retained",
            ));
        }
        let _operation_lock = Lock::acquire(&ctx.paths.clash_lock)?;
        storage::remove_file(&before.file)?;
        systemctl(ctx, scope, &["daemon-reload"])
    })?;
    ctx.message(
        "notice.clash_service",
        format!("{}: disabled and removed", scope.name()),
    );
    Ok(())
}

fn invoked(ctx: &Context) -> bool {
    ctx.environment.get(INVOKED).map(String::as_str) == Some("1")
        || std::env::var(INVOKED).as_deref() == Ok("1")
}

fn managed_scope(ctx: &Context) -> Result<Option<Scope>> {
    if invoked(ctx) {
        return Ok(None);
    }
    if pending(ctx).exists() {
        return Err(Error::new(
            "error.pending",
            format!(
                "{}; retry service install/remove to recover",
                pending(ctx).display()
            ),
        ));
    }
    let mut selected = None;
    for scope in [Scope::User, Scope::System] {
        if let Some(bytes) = read_unit(ctx, scope)?
            && owned(ctx, &bytes)?
            && selected.replace(scope).is_some()
        {
            return Err(Error::new(
                "error.busy",
                "both user and system services claim this instance",
            ));
        }
    }
    Ok(selected)
}

/// Returns false when the direct lifecycle should handle this instance.
pub(super) fn start_managed(ctx: &Context) -> Result<bool> {
    let Some(scope) = managed_scope(ctx)? else {
        return Ok(false);
    };
    preflight(ctx, scope)?;
    let _lock = Lock::acquire(&storage::suffix(&ctx.paths.clash_lock, ".service"))?;
    check_fragment(ctx, scope, true)?;
    if !active(ctx, scope)? && matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "a direct Mihomo process is running; stop it before starting the service",
        ));
    }
    systemctl(ctx, scope, &["start", UNIT])?;
    if !active(ctx, scope)? || !matches!(state(ctx)?, State::Running(_)) {
        return Err(Error::new("error.verify", "managed Mihomo did not start"));
    }
    Ok(true)
}

pub(super) fn stop_managed(ctx: &Context) -> Result<bool> {
    let Some(scope) = managed_scope(ctx)? else {
        return Ok(false);
    };
    preflight(ctx, scope)?;
    let _lock = Lock::acquire(&storage::suffix(&ctx.paths.clash_lock, ".service"))?;
    check_fragment(ctx, scope, true)?;
    systemctl(ctx, scope, &["stop", UNIT])?;
    if active(ctx, scope)? || matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new("error.verify", "managed Mihomo did not stop"));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires systemd-analyze; verifies temporary units without contacting a manager"]
    fn generated_units_pass_the_real_systemd_parser() {
        let temp = tempfile::Builder::new()
            .prefix("cqs unit $value %i ")
            .tempdir()
            .unwrap();
        let ctx = crate::tests::context(temp.path());
        let executable = temp.path().join("cqs $value %n");
        fs::copy("/usr/bin/true", &executable).unwrap();
        for scope in [Scope::User, Scope::System] {
            let unit = render(&ctx, &executable, scope).unwrap();
            let path = temp.path().join(UNIT);
            fs::write(&path, unit).unwrap();
            let result = Command::new("systemd-analyze")
                .args(["verify", "--user", "--man=no"])
                .env("XDG_RUNTIME_DIR", temp.path())
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "systemd-analyze rejected {} unit: {}{}",
                scope.name(),
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn systemd_quote_preserves_whitespace_and_neutralizes_expansion() {
        assert_eq!(
            quote("/opt/my cqs/$value/%i/\"bin\"").unwrap(),
            "\"/opt/my cqs/$value/%%i/\\\"bin\\\"\""
        );
        assert_eq!(
            quote("HOME=/opt/$user/%h").unwrap(),
            "\"HOME=/opt/$user/%%h\""
        );
        assert!(quote("/a\nExecStart=/bin/false").is_err());
        assert!(quote("").is_err());
    }

    #[test]
    fn generated_unit_pins_all_paths_and_has_no_shell_or_secret() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let executable = temp.path().join("my cqs$tool%name");
        fs::write(&executable, b"test executable").unwrap();
        let unit = render(&ctx, &executable, Scope::User).unwrap();
        assert!(unit.starts_with(MARKER));
        assert!(unit.contains("Type=forking\n"));
        assert!(unit.contains("PIDFile="));
        assert!(unit.contains("CQS_CLASH_SERVICE_INVOKED=1"));
        assert!(
            unit.contains(
                "my cqs$tool%%name\" --yes --lang en-US --config /dev/null run clash.start"
            )
        );
        assert!(unit.contains("run clash.stop"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("/bin/sh"));
        assert!(!unit.contains("secret"));
        for (key, _) in environment(&ctx) {
            assert!(unit.contains(&format!("Environment=\"{key}=")));
        }
        assert!(
            render(&ctx, &executable, Scope::System)
                .unwrap()
                .contains("WantedBy=multi-user.target")
        );
    }

    #[test]
    fn owned_units_are_bound_to_the_exact_instance() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = crate::tests::context(first.path());
        let b = crate::tests::context(second.path());
        let bytes = header(&a).unwrap();
        assert!(owned(&a, bytes.as_bytes()).unwrap());
        assert!(!owned(&b, bytes.as_bytes()).unwrap());
        assert!(!owned(&a, b"[Service]\nExecStart=/bin/false").unwrap());
        assert!(Scope::parse("system\n").is_err());
        assert!(Scope::parse("user").is_ok());
    }

    #[test]
    fn invalid_pending_before_image_is_rejected_before_any_command() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let before = Before {
            instance: "wrong-instance".into(),
            scope: "user".into(),
            file: Scope::User.path(&ctx),
            content: None,
            enabled: false,
            active: false,
        };
        assert_eq!(restore(&ctx, &before).unwrap_err().key, "error.pending");
        let before = Before {
            instance: identity(&ctx).unwrap(),
            file: temp.path().join("foreign.service"),
            ..before
        };
        assert_eq!(restore(&ctx, &before).unwrap_err().key, "error.pending");
    }

    #[test]
    fn unit_child_bypasses_manager_but_external_calls_do_not_ignore_pending() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        fs::write(pending(&ctx), b"broken").unwrap();
        assert_eq!(managed_scope(&ctx).unwrap_err().key, "error.pending");
        ctx.environment.insert(INVOKED.into(), "1".into());
        assert_eq!(managed_scope(&ctx).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn absent_unit_under_linked_config_does_not_block_direct_lifecycle() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let target = temp.path().join("dotfiles");
        fs::create_dir_all(target.join("systemd/user")).unwrap();
        symlink(&target, ctx.paths.home.join(".config")).unwrap();
        assert!(read_unit(&ctx, Scope::User).unwrap().is_none());
        fs::write(
            target.join("systemd/user").join(UNIT),
            header(&ctx).unwrap(),
        )
        .unwrap();
        assert_eq!(read_unit(&ctx, Scope::User).unwrap_err().key, "error.path");
    }

    #[cfg(unix)]
    #[test]
    fn unit_probe_rejects_dangling_links_and_invalid_parent_paths() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let unit = Scope::User.path(&ctx);
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        symlink(temp.path().join("absent-unit"), &unit).unwrap();
        assert_eq!(read_unit(&ctx, Scope::User).unwrap_err().key, "error.path");
        let invalid = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(invalid.path());
        fs::write(ctx.paths.home.join(".config"), b"not a directory").unwrap();
        assert!(read_unit(&ctx, Scope::User).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn missing_unit_enable_state_is_verified_without_hiding_manager_errors() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        let bin = temp.path().join("fixture-bin");
        fs::create_dir_all(&bin).unwrap();
        let command = bin.join("systemctl");
        fs::write(
            &command,
            r#"#!/bin/sh
case "$*" in
  '--user is-enabled cqs-mihomo.service')
    printf '%s' "$CQS_TEST_ENABLED"
    printf '%s' "$CQS_TEST_STDERR" >&2
    exit "$CQS_TEST_ENABLED_STATUS";;
  '--user show cqs-mihomo.service --property=LoadState --value')
    printf queried > "$CQS_TEST_QUERY"
    printf '%s' "$CQS_TEST_LOAD_STATE"
    exit "$CQS_TEST_LOAD_STATUS";;
  *) exit 64;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
        ctx.environment
            .insert("PATH".into(), path_string(&bin).unwrap());
        let cases = [
            (0, "enabled", "", 0, Some(true)),
            (1, "disabled", "", 0, Some(false)),
            (1, "not-found", "", 0, Some(false)),
            (4, "not-found", "", 0, Some(false)),
            (1, "", "not-found", 0, Some(false)),
            (4, "", "not-found", 0, Some(false)),
            (1, "", "loaded", 0, None),
            (1, "", "not-found", 1, None),
            (1, "masked", "", 0, None),
            (0, "static", "", 0, None),
        ];
        for (index, (status, value, load, load_status, expected)) in cases.into_iter().enumerate() {
            let query = temp.path().join(format!("query-{index}"));
            for (key, value) in [
                ("CQS_TEST_ENABLED", value.to_owned()),
                ("CQS_TEST_ENABLED_STATUS", status.to_string()),
                ("CQS_TEST_STDERR", "No such file or directory".to_owned()),
                ("CQS_TEST_LOAD_STATE", load.to_owned()),
                ("CQS_TEST_LOAD_STATUS", load_status.to_string()),
                ("CQS_TEST_QUERY", path_string(&query).unwrap()),
            ] {
                ctx.environment.insert(key.into(), value);
            }
            let actual = enabled(&ctx, Scope::User);
            if let Some(expected) = expected {
                assert_eq!(actual.unwrap(), expected, "case {index}");
            } else {
                assert!(actual.is_err(), "case {index}");
            }
            assert_eq!(query.exists(), value.is_empty(), "case {index}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_service_changes_restore_files_and_keep_failed_recovery_material() {
        use std::os::unix::fs::PermissionsExt;
        for fail_recovery in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut ctx = crate::tests::context(temp.path());
            let bin = temp.path().join("fixture-bin");
            fs::create_dir_all(&bin).unwrap();
            let command = bin.join("systemctl");
            // This fixture models only the boundary calls used by rollback;
            // unknown commands fail rather than masquerading as a manager.
            fs::write(&command, b"#!/bin/sh\ncase \"$*\" in\n'--user show cqs-mihomo.service --property=ActiveState --value') printf inactive;;\n'--user disable cqs-mihomo.service') test \"${CQS_SERVICE_FIXTURE_FAIL-}\" != 1 || exit 42; printf disabled > \"$CQS_SERVICE_FIXTURE_STATE\";;\n'--user daemon-reload') test -f \"$CQS_SERVICE_FIXTURE_STATE\" || exit 43;;\n*) exit 64;;\nesac\n").unwrap();
            fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
            ctx.environment
                .insert("PATH".into(), path_string(&bin).unwrap());
            ctx.environment.insert(
                "CQS_SERVICE_FIXTURE_STATE".into(),
                path_string(&temp.path().join("manager-state")).unwrap(),
            );
            if fail_recovery {
                ctx.environment
                    .insert("CQS_SERVICE_FIXTURE_FAIL".into(), "1".into());
            }
            let unit = Scope::User.path(&ctx);
            let old = format!(
                "{}[Service]\n# previous definition\n",
                header(&ctx).unwrap()
            );
            let changed = format!("{}[Service]\n# changed definition\n", header(&ctx).unwrap());
            storage::atomic_write(&unit, old.as_bytes(), 0o644).unwrap();
            let before = Before {
                instance: identity(&ctx).unwrap(),
                scope: "user".into(),
                file: unit.clone(),
                content: storage::Saved::capture(&unit).unwrap(),
                enabled: false,
                active: false,
            };
            let error = transaction(&ctx, &before, || {
                assert!(pending(&ctx).is_file());
                storage::atomic_write(&unit, changed.as_bytes(), 0o644)?;
                Err(Error::new("error.command", "injected operation failure"))
            })
            .unwrap_err();
            if fail_recovery {
                assert_eq!(error.key, "error.recovery");
                assert!(pending(&ctx).is_file());
                assert_eq!(fs::read(&unit).unwrap(), changed.as_bytes());
                ctx.environment.remove("CQS_SERVICE_FIXTURE_FAIL");
                restore(&ctx, &before).unwrap();
            } else {
                assert_eq!(error.key, "error.command");
            }
            assert_eq!(fs::read(&unit).unwrap(), old.as_bytes());
            assert!(!pending(&ctx).exists());
        }
    }
}
