//! Mihomo lifecycle and subscription primitives.
//!
//! The reference `clashctl` project is shell based and supports several init
//! systems. CqShell keeps the same data flow while using a single supervised
//! user process: validate YAML, publish a runtime config atomically, record a
//! PID with the expected command line, and expose read-only status queries.

use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock},
};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const STOP_WAIT: Duration = Duration::from_secs(5);
const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_BINARY: usize = 512 * 1024 * 1024;
const MAX_INSTALL_RECORD: u64 = 4096;

pub mod compat;
pub mod controller;
mod logs;
pub mod panel;
pub mod service;
pub mod tun;

#[cfg(test)]
mod regression;

pub use logs::logs;
use logs::open_private_log;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Running(u32),
    Stopped,
    Unknown,
}

impl State {
    fn label(self) -> String {
        match self {
            Self::NotInstalled => "not installed".into(),
            Self::Running(pid) => format!("running (PID {pid})"),
            Self::Stopped => "stopped".into(),
            Self::Unknown => "unknown".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct ProfilesFile {
    #[serde(rename = "use", default)]
    current: Option<String>,
    #[serde(default)]
    profiles: Vec<Profile>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Profile {
    name: String,
    url: String,
    path: String,
    #[serde(default)]
    updated: String,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
struct ProxyConfig {
    host: String,
    http_port: Option<u16>,
    socks_port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
}

pub fn status(ctx: &Context) -> Result<()> {
    let state = state(ctx)?;
    let detail = format!(
        "{}; HTTP {}; SOCKS5 {}",
        state.label(),
        proxy_address(ctx, false).unwrap_or_else(|| "not configured".into()),
        proxy_address(ctx, true).unwrap_or_else(|| "not configured".into()),
    );
    ctx.message("notice.clash_status", detail);
    let profiles = read_profiles(ctx)?;
    ctx.message(
        "notice.clash_subscriptions",
        profiles.current.as_deref().unwrap_or("none"),
    );
    if let State::Running(pid) = state
        && let Some(seconds) = process_uptime(pid)
    {
        ctx.message(
            "notice.clash_uptime",
            format!(
                "{}h {}m {}s",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            ),
        );
    }
    controller::status(ctx)?;
    panel::status(ctx)?;
    tun::status(ctx)?;
    Ok(())
}

fn process_uptime(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let ticks = process_start_time(pid)?.parse::<f64>().ok()?;
        let frequency = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if frequency <= 0 {
            return None;
        }
        let uptime = fs::read_to_string("/proc/uptime").ok()?;
        let elapsed =
            uptime.split_whitespace().next()?.parse::<f64>().ok()? - ticks / frequency as f64;
        (elapsed.is_finite() && elapsed >= 0.0).then_some(elapsed as u64)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

pub fn start(ctx: &Context) -> Result<()> {
    if service::start_managed(ctx)? {
        return Ok(());
    }
    start_inner(ctx, true)
}

fn start_inner(ctx: &Context, report: bool) -> Result<()> {
    ctx.check()?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    match state(ctx)? {
        State::Running(pid) => {
            if report {
                ctx.message(
                    "notice.clash_started",
                    format!("already running (PID {pid})"),
                );
            }
            return Ok(());
        }
        State::Unknown => return Err(unknown_process()),
        State::NotInstalled | State::Stopped => (),
    }
    ensure_layout(ctx)?;
    require_binary(ctx)?;
    let binary = path_string(&ctx.paths.clash_binary)?;
    let resources = path_string(&ctx.paths.clash_resources)?;
    let runtime_bytes = build_runtime_config(ctx)?;
    tun::validate_start(ctx, &runtime_bytes)?;
    let candidate = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    fs::write(candidate.path(), &runtime_bytes)?;
    validate_config(ctx, candidate.path(), true)?;
    validate_with_mihomo(ctx, candidate.path())?;
    storage::atomic_write(&ctx.paths.clash_runtime, &runtime_bytes, 0o600)?;
    let runtime = path_string(&ctx.paths.clash_runtime)?;
    let log = open_private_log(ctx)?;
    let stderr = log.try_clone()?;
    let mut command = Command::new(&binary);
    command
        .args(["-d", &resources, "-f", &runtime])
        .envs(&ctx.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The daemon must outlive its CLI's terminal and cancellation group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| Error::new("error.command", format!("{binary}: {e}")))?;
    let pid = child.id();
    let Some(start_time) = process_start_time(pid) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::new(
            "error.verify",
            "cannot read Mihomo process start time",
        ));
    };
    if let Err(error) =
        storage::atomic_write(&ctx.paths.clash_pid, pid.to_string().as_bytes(), 0o600).and_then(
            |()| storage::atomic_write(&ctx.paths.clash_pid_start, start_time.as_bytes(), 0o600),
        )
    {
        let _ = child.kill();
        let _ = child.wait();
        let _ = storage::remove_file(&ctx.paths.clash_pid);
        let _ = storage::remove_file(&ctx.paths.clash_pid_start);
        return Err(error);
    }
    thread::sleep(Duration::from_millis(100));
    if let Err(cancelled) = ctx.check() {
        let cleanup = (|| {
            signal(pid, false)?;
            let deadline = std::time::Instant::now() + STOP_WAIT;
            while std::time::Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    storage::remove_file(&ctx.paths.clash_pid)?;
                    storage::remove_file(&ctx.paths.clash_pid_start)?;
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(Error::new(
                "error.recovery",
                format!(
                    "start was cancelled but Mihomo did not stop after SIGTERM (PID {pid}); PID records retained"
                ),
            ))
        })();
        reap_child(ctx, child);
        cleanup?;
        return Err(cancelled);
    }
    if !pid_matches(ctx, pid) {
        let exited = child.try_wait();
        reap_child(ctx, child);
        if exited?.is_none() {
            return Err(unknown_process());
        }
        let _ = storage::remove_file(&ctx.paths.clash_pid);
        let _ = storage::remove_file(&ctx.paths.clash_pid_start);
        let log_tail = logs::startup_tail(ctx)?;
        return Err(Error::new(
            "error.command",
            format!("Mihomo exited during startup: {}", tail(&log_tail)),
        ));
    }
    if report {
        ctx.message("notice.clash_started", format!("PID {pid}"));
    }
    reap_child(ctx, child);
    Ok(())
}

fn reap_child(ctx: &Context, mut child: std::process::Child) {
    // A long-lived TUI remains the daemon's parent, including failed or
    // cancelled starts whose PID records must be retained for recovery.
    let logger = ctx.clone();
    thread::spawn(move || {
        if let Err(error) = child.wait() {
            logger.message("notice.clash_reap", error);
        }
    });
}

pub fn stop(ctx: &Context) -> Result<()> {
    if service::stop_managed(ctx)? {
        return Ok(());
    }
    ctx.check()?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    let pid = match state(ctx)? {
        State::Running(pid) => pid,
        State::Stopped | State::NotInstalled => {
            storage::remove_file(&ctx.paths.clash_pid)?;
            storage::remove_file(&ctx.paths.clash_pid_start)?;
            ctx.message("notice.clash_stopped", "already stopped");
            return Ok(());
        }
        State::Unknown => return Err(unknown_process()),
    };
    signal(pid, false)?;
    let deadline = std::time::Instant::now() + STOP_WAIT;
    while std::time::Instant::now() < deadline {
        match process_state(ctx, pid) {
            State::Stopped => {
                storage::remove_file(&ctx.paths.clash_pid)?;
                storage::remove_file(&ctx.paths.clash_pid_start)?;
                ctx.message("notice.clash_stopped", format!("PID {pid}"));
                return Ok(());
            }
            State::Running(_) | State::Unknown => (),
            State::NotInstalled => return Err(unknown_process()),
        }
        thread::sleep(Duration::from_millis(100));
    }
    if matches!(process_state(ctx, pid), State::Unknown) {
        return Err(unknown_process());
    }
    Err(Error::new(
        "error.command",
        format!("Mihomo did not stop after SIGTERM (PID {pid})"),
    ))
}

pub fn proxy_env(ctx: &Context, enabled: bool, shell: &str) -> Result<String> {
    if !matches!(shell, "bash" | "zsh" | "fish") {
        return Err(Error::new(
            "error.input",
            format!("unsupported shell: {shell}"),
        ));
    }
    if !enabled {
        return Ok(match shell {
            "bash" | "zsh" => "unset http_proxy https_proxy all_proxy no_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY\n".into(),
            "fish" => "set -e http_proxy\nset -e https_proxy\nset -e all_proxy\nset -e no_proxy\nset -e HTTP_PROXY\nset -e HTTPS_PROXY\nset -e ALL_PROXY\nset -e NO_PROXY\ntrue\n".into(),
            _ => unreachable!(),
        });
    }
    if !matches!(state(ctx)?, State::Running(_)) && !service::start_managed(ctx)? {
        start_inner(ctx, false)?;
    }
    let config = proxy_config(ctx)?;
    proxy_env_script(&config, shell)
}

fn proxy_env_script(config: &ProxyConfig, shell: &str) -> Result<String> {
    let http = config
        .http_port
        .ok_or_else(|| Error::new("error.data", "runtime config has no HTTP or mixed port"))?;
    let socks = config
        .socks_port
        .ok_or_else(|| Error::new("error.data", "runtime config has no SOCKS or mixed port"))?;
    let http = proxy_url("http", config, http)?;
    let socks = proxy_url("socks5h", config, socks)?;
    let assignments = [
        ("http_proxy", http.as_str()),
        ("https_proxy", http.as_str()),
        ("all_proxy", socks.as_str()),
        ("no_proxy", "localhost,127.0.0.1,::1"),
        ("HTTP_PROXY", http.as_str()),
        ("HTTPS_PROXY", http.as_str()),
        ("ALL_PROXY", socks.as_str()),
        ("NO_PROXY", "localhost,127.0.0.1,::1"),
    ];
    let mut script = String::new();
    for (key, value) in assignments {
        match shell {
            "bash" | "zsh" => {
                script.push_str(&format!("export {key}={}\n", shell_quote(shell, value)))
            }
            "fish" => script.push_str(&format!("set -gx {key} {}\n", shell_quote(shell, value))),
            _ => unreachable!(),
        }
    }
    Ok(script)
}

pub fn install(ctx: &Context, requested: &str) -> Result<()> {
    ctx.check()?;
    ctx.require(&["curl"])?;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64-v1",
        "aarch64" => "arm64",
        "arm" => "armv7",
        "x86" => "386",
        value => return Err(Error::new("error.platform", value)),
    };
    ensure_layout(ctx)?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    if matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo before replacing its executable",
        ));
    }
    let release = if requested.is_empty() {
        release_metadata(ctx, "latest")?
    } else {
        release_metadata(ctx, &validate_version(requested)?)?
    };
    let version = validate_version(release["tag_name"].as_str().unwrap_or(""))?;
    let name = format!("mihomo-linux-{arch}-{version}.gz");
    let url = format!("https://github.com/MetaCubeX/mihomo/releases/download/{version}/{name}");
    let release = release_metadata(ctx, &version)?;
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "assets"))?;
    let mut matched = assets.iter().filter(|asset| asset["name"] == name);
    let asset = matched
        .next()
        .ok_or_else(|| Error::new("error.release", &name))?;
    if matched.next().is_some() || asset["browser_download_url"] != url {
        return Err(Error::new("error.release", &name));
    }
    let digest = asset["digest"]
        .as_str()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::new("error.release", format!("missing SHA-256 for {name}")))?;
    let size = asset["size"]
        .as_u64()
        .filter(|size| (1..=MAX_ARCHIVE).contains(size))
        .ok_or_else(|| Error::new("error.release", "asset size"))?;
    if installed_release_matches(ctx, &version, &digest)? {
        ctx.message(
            "notice.clash_installed",
            format!("{version} ({arch}); already verified, download skipped"),
        );
        return Ok(());
    }

    let work = tempfile::tempdir_in(&ctx.paths.clash_data)?;
    let archive = work.path().join("mihomo.gz");
    download_release_asset(ctx, &url, &archive, size)?;
    let bytes = fs::read(&archive)?;
    if bytes.len() as u64 != size || sha256_hex(&bytes) != digest {
        return Err(Error::new("error.digest", name));
    }
    use flate2::read::GzDecoder;
    let decoder = GzDecoder::new(bytes.as_slice());
    let mut binary = Vec::new();
    decoder
        .take(MAX_BINARY as u64 + 1)
        .read_to_end(&mut binary)?;
    if binary.len() > MAX_BINARY || !valid_elf_arch(&binary) {
        return Err(Error::new("error.release", "Mihomo ELF architecture/size"));
    }
    let candidate = work.path().join("mihomo");
    storage::atomic_write(&candidate, &binary, 0o755)?;
    let candidate = path_string(&candidate)?;
    let banner = ctx.output(&candidate, &["-v"])?;
    if !version_banner_matches(&banner, &version) {
        return Err(Error::new(
            "error.verify",
            format!("unexpected Mihomo version: {banner}"),
        ));
    }
    storage::atomic_write(&ctx.paths.clash_binary, &binary, 0o755)?;
    let record = InstallRecord {
        version: version.clone(),
        asset_digest: digest,
        binary_digest: sha256_hex(&binary),
    };
    storage::atomic_write(
        &install_record_path(ctx),
        &serde_json::to_vec(&record)?,
        0o600,
    )?;
    ctx.message("notice.clash_installed", format!("{version} ({arch})"));
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRecord {
    version: String,
    asset_digest: String,
    binary_digest: String,
}

fn install_record_path(ctx: &Context) -> PathBuf {
    ctx.paths.clash_data.join("mihomo-install.json")
}

fn installed_release_matches(ctx: &Context, version: &str, asset_digest: &str) -> Result<bool> {
    let record_path = install_record_path(ctx);
    storage::check_path(&record_path, true)?;
    if !record_path.is_file() {
        return Ok(false);
    }
    if fs::metadata(&record_path)?.len() > MAX_INSTALL_RECORD {
        return Ok(false);
    }
    let record: InstallRecord = match serde_json::from_slice(&fs::read(&record_path)?) {
        Ok(record) => record,
        Err(_) => return Ok(false),
    };
    if record.version != version || record.asset_digest != asset_digest {
        return Ok(false);
    }
    storage::check_path(&ctx.paths.clash_binary, true)?;
    let binary = match fs::read(&ctx.paths.clash_binary) {
        Ok(binary) => binary,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if sha256_hex(&binary) != record.binary_digest {
        return Ok(false);
    }
    let binary_path = path_string(&ctx.paths.clash_binary)?;
    let banner = match ctx.output(&binary_path, &["-v"]) {
        Ok(banner) => banner,
        Err(error) if error.key == "error.cancelled" => return Err(error),
        Err(_) => return Ok(false),
    };
    Ok(version_banner_matches(&banner, version))
}

fn release_metadata(ctx: &Context, version: &str) -> Result<serde_json::Value> {
    let url = if version == "latest" {
        "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest".to_owned()
    } else {
        format!("https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/{version}")
    };
    let body = ctx.output(
        "curl",
        &[
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-fsSL",
            "--connect-timeout",
            "15",
            "--max-time",
            "30",
            "-H",
            "Accept: application/vnd.github+json",
            &url,
        ],
    )?;
    let release: serde_json::Value = serde_json::from_str(&body)?;
    if release["draft"] == true || release["prerelease"] == true {
        return Err(Error::new("error.release", "draft or prerelease"));
    }
    Ok(release)
}

fn validate_version(value: &str) -> Result<String> {
    let version = value.strip_prefix('v').unwrap_or(value);
    let components = version.split('.').collect::<Vec<_>>();
    if components.len() != 3
        || components
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(Error::new("error.input", "Mihomo version must be vX.Y.Z"));
    }
    Ok(format!("v{version}"))
}

fn valid_elf_arch(bytes: &[u8]) -> bool {
    if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" || bytes[5] != 1 {
        return false;
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    matches!(
        (std::env::consts::ARCH, machine),
        ("x86_64", 62) | ("aarch64", 183) | ("arm", 40) | ("x86", 3)
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn subscriptions(ctx: &Context) -> Result<()> {
    let file = read_profiles(ctx)?;
    if file.profiles.is_empty() {
        ctx.message("notice.clash_subscriptions", "none configured");
    } else {
        let current = file.current.as_deref().unwrap_or("none");
        let rows = file
            .profiles
            .iter()
            .map(|profile| {
                let marker = if Some(profile.name.as_str()) == file.current.as_deref() {
                    "*"
                } else {
                    " "
                };
                format!("{marker} {} ({})", profile.name, url_host(&profile.url))
            })
            .collect::<Vec<_>>()
            .join("\n");
        ctx.message(
            "notice.clash_subscriptions",
            format!("current={current}\n{rows}"),
        );
    }
    Ok(())
}

/// The host of a subscription URL; the full URL may carry an access token.
fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "invalid-url".into())
}

/// Saved subscriptions in their stored order, marking the selected one.
pub fn subscription_choices(ctx: &Context) -> Result<Vec<crate::ops::Choice>> {
    let file = read_profiles(ctx)?;
    Ok(file
        .profiles
        .into_iter()
        .map(|profile| crate::ops::Choice {
            current: file.current.as_deref() == Some(profile.name.as_str()),
            detail: url_host(&profile.url),
            value: profile.name,
            children: Vec::new(),
        })
        .collect())
}

pub fn subscription_add(ctx: &Context, name: &str, url: &str, use_after_add: bool) -> Result<()> {
    validate_profile_name(name)?;
    validate_subscription_url(url)?;
    ctx.require(&["curl"])?;
    ensure_layout(ctx)?;
    let profiles_dir = ctx.paths.clash_data.join("profiles");
    {
        let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
        recover_interrupted_files(ctx)?;
        validate_new_profile(&read_profiles(ctx)?, name, url)?;
    }
    let temporary = tempfile::NamedTempFile::new_in(&profiles_dir)?;
    download_subscription(ctx, url, temporary.path())?;
    validate_config(ctx, temporary.path(), false)?;
    validate_with_mihomo(ctx, temporary.path())?;
    let bytes = fs::read(temporary.path())?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let mut profiles = read_profiles(ctx)?;
    validate_new_profile(&profiles, name, url)?;
    if use_after_add {
        validate_subscription_runtime(ctx, &bytes)?;
    }
    let file_name = unused_profile_file_name(&profiles_dir, &profiles, name)?;
    profiles.profiles.push(Profile {
        name: name.into(),
        url: url.into(),
        path: file_name.clone(),
        updated: now_string(),
        extra: BTreeMap::new(),
    });
    if use_after_add {
        profiles.current = Some(name.into());
    }
    let metadata = profiles_bytes(&profiles)?;
    let mut updates = vec![
        (
            profiles_dir.join(file_name),
            Some(storage::Saved::new(&bytes, 0o600)),
        ),
        (
            ctx.paths.clash_profiles.clone(),
            Some(storage::Saved::new(&metadata, 0o600)),
        ),
    ];
    if use_after_add {
        updates.push((
            ctx.paths.clash_config.clone(),
            Some(storage::Saved::new(&bytes, 0o600)),
        ));
    }
    commit_files(ctx, &updates)?;
    ctx.message("notice.clash_subscription", format!("added {name}"));
    Ok(())
}

pub fn subscription_use(ctx: &Context, name: &str) -> Result<()> {
    validate_profile_name(name)?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let mut profiles = read_profiles(ctx)?;
    let bytes = activate_profile(ctx, &mut profiles, name)?;
    commit_files(
        ctx,
        &[
            (
                ctx.paths.clash_config.clone(),
                Some(storage::Saved::new(&bytes, 0o600)),
            ),
            (
                ctx.paths.clash_profiles.clone(),
                Some(storage::Saved::new(&profiles_bytes(&profiles)?, 0o600)),
            ),
        ],
    )?;
    ctx.message("notice.clash_subscription", format!("selected {name}"));
    Ok(())
}

pub fn subscription_update(ctx: &Context, name: &str) -> Result<()> {
    validate_profile_name(name)?;
    ctx.require(&["curl"])?;
    ensure_layout(ctx)?;
    let profile = {
        let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
        recover_interrupted_files(ctx)?;
        read_profiles(ctx)?
            .profiles
            .into_iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| Error::new("error.input", format!("unknown subscription: {name}")))?
    };
    let profiles_dir = ctx.paths.clash_data.join("profiles");
    let temporary = tempfile::NamedTempFile::new_in(&profiles_dir)?;
    download_subscription(ctx, &profile.url, temporary.path())?;
    validate_config(ctx, temporary.path(), false)?;
    validate_with_mihomo(ctx, temporary.path())?;
    let bytes = fs::read(temporary.path())?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let mut profiles = read_profiles(ctx)?;
    let index = profiles
        .profiles
        .iter()
        .position(|current| current.name == name)
        .ok_or_else(|| Error::new("error.input", format!("unknown subscription: {name}")))?;
    if profiles.profiles[index] != profile {
        return Err(Error::new(
            "error.busy",
            "subscription changed while its update was downloading; retry the update",
        ));
    }
    profiles.profiles[index].updated = now_string();
    let path = resolve_profile_path(ctx, &profile.path)?;
    let mut updates = vec![
        (path, Some(storage::Saved::new(&bytes, 0o600))),
        (
            ctx.paths.clash_profiles.clone(),
            Some(storage::Saved::new(&profiles_bytes(&profiles)?, 0o600)),
        ),
    ];
    if profiles.current.as_deref() == Some(name) {
        validate_subscription_runtime(ctx, &bytes)?;
        updates.push((
            ctx.paths.clash_config.clone(),
            Some(storage::Saved::new(&bytes, 0o600)),
        ));
    }
    commit_files(ctx, &updates)?;
    ctx.message("notice.clash_subscription", format!("updated {name}"));
    Ok(())
}

pub fn subscription_remove(ctx: &Context, name: &str) -> Result<()> {
    validate_profile_name(name)?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let mut profiles = read_profiles(ctx)?;
    let index = profiles
        .profiles
        .iter()
        .position(|profile| profile.name == name)
        .ok_or_else(|| Error::new("error.input", format!("unknown subscription: {name}")))?;
    if profiles.current.as_deref() == Some(name)
        && matches!(state(ctx)?, State::Running(_) | State::Unknown)
    {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo or select another subscription before removing the current subscription",
        ));
    }
    let removed = profiles.profiles.remove(index);
    let mut updates = vec![(resolve_profile_path(ctx, &removed.path)?, None)];
    if profiles.current.as_deref() == Some(name) {
        profiles.current = None;
        updates.push((ctx.paths.clash_config.clone(), None));
        updates.push((ctx.paths.clash_runtime.clone(), None));
    }
    updates.push((
        ctx.paths.clash_profiles.clone(),
        Some(storage::Saved::new(&profiles_bytes(&profiles)?, 0o600)),
    ));
    commit_files(ctx, &updates)?;
    ctx.message("notice.clash_subscription", format!("removed {name}"));
    Ok(())
}

pub(crate) fn state(ctx: &Context) -> Result<State> {
    let pid = match fs::read_to_string(&ctx.paths.clash_pid) {
        Ok(value) => value
            .trim()
            .parse::<u32>()
            .map_err(|_| Error::new("error.data", "invalid Mihomo PID file"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(&ctx.paths.clash_pid_start) {
                Ok(_) => return Ok(State::Unknown),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            return match fs::metadata(&ctx.paths.clash_binary) {
                Ok(metadata) if metadata.is_file() => Ok(State::Stopped),
                Ok(_) => Err(Error::new("error.data", "Mihomo binary path is not a file")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(State::NotInstalled)
                }
                Err(error) => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    };
    Ok(process_state(ctx, pid))
}

pub(crate) fn installed_version(ctx: &Context) -> Result<Option<String>> {
    storage::check_path(&ctx.paths.clash_binary, true)?;
    match fs::metadata(&ctx.paths.clash_binary) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    require_binary(ctx)?;
    let binary = path_string(&ctx.paths.clash_binary)?;
    let banner = ctx.output("timeout", &["--signal=KILL", "2s", &binary, "-v"])?;
    version_from_banner(&banner)
        .map(Some)
        .ok_or_else(|| Error::new("error.data", "unrecognized Mihomo version banner"))
}

fn unknown_process() -> Error {
    Error::new(
        "error.data",
        "Mihomo process identity cannot be verified; PID records retained for recovery",
    )
}

fn ensure_layout(ctx: &Context) -> Result<()> {
    storage::private_dir(&ctx.paths.clash_home)?;
    storage::private_dir(&ctx.paths.clash_data)?;
    storage::private_dir(&ctx.paths.clash_resources)?;
    storage::private_dir(&ctx.paths.clash_data.join("profiles"))
}

fn require_binary(ctx: &Context) -> Result<()> {
    storage::check_path(&ctx.paths.clash_binary, true)?;
    let metadata = fs::metadata(&ctx.paths.clash_binary).map_err(|_| {
        Error::new(
            "error.data",
            format!(
                "Mihomo binary is not installed: {}",
                ctx.paths.clash_binary.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(Error::new("error.data", "Mihomo binary path is not a file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::new("error.data", "Mihomo binary is not executable"));
        }
    }
    Ok(())
}

fn build_runtime_config(ctx: &Context) -> Result<Vec<u8>> {
    runtime_with_mixin(ctx, read_mixin(ctx)?)
}

fn read_mixin(ctx: &Context) -> Result<Value> {
    storage::check_path(&ctx.paths.clash_mixin, true)?;
    if ctx.paths.clash_mixin.is_file() {
        yaml(&fs::read(&ctx.paths.clash_mixin)?)
    } else {
        Ok(Value::Mapping(Default::default()))
    }
}

fn runtime_with_mixin(ctx: &Context, mixin: Value) -> Result<Vec<u8>> {
    storage::check_path(&ctx.paths.clash_config, true)?;
    let base = if ctx.paths.clash_config.is_file() {
        yaml(&fs::read(&ctx.paths.clash_config)?)?
    } else {
        if read_profiles(ctx)?
            .current
            .as_deref()
            .is_some_and(|name| !name.is_empty())
        {
            return Err(Error::new(
                "error.data",
                "selected subscription base configuration is missing; restore it or select a valid subscription",
            ));
        }
        let mut direct = serde_yaml::Mapping::new();
        direct.insert(Value::String("mode".into()), Value::String("direct".into()));
        direct.insert(
            Value::String("mixed-port".into()),
            Value::Number(7890.into()),
        );
        Value::Mapping(direct)
    };
    render_runtime(base, mixin)
}

fn render_runtime(base: Value, mixin: Value) -> Result<Vec<u8>> {
    let runtime = merge_config(base, mixin)?;
    controller::from_config(&runtime)?;
    let text = serde_yaml::to_string(&runtime).map_err(|e| Error::new("error.data", e))?;
    Ok(text.into_bytes())
}

/// Subscription panels choose the response format by User-Agent: without a Clash
/// identifier many return a Base64 share-link list instead of a configuration. This
/// identifies as the Mihomo (Clash Meta) client so the provider serves its native config.
const SUBSCRIPTION_AGENT: &str = "clash.meta";

/// Download a subscription and reject non-Clash formats with an actionable message
/// instead of a YAML type error. No format conversion is attempted.
fn download_subscription(ctx: &Context, url: &str, target: &Path) -> Result<()> {
    ctx.download_as(url, target, 30, Some(SUBSCRIPTION_AGENT))?;
    // A share-link list is valid YAML but parses as a plain string, not a map.
    if serde_yaml::from_slice::<Value>(&fs::read(target)?).is_ok_and(|value| !value.is_mapping()) {
        return Err(Error::new("error.subscription_format", ""));
    }
    Ok(())
}

fn validate_subscription_runtime(ctx: &Context, bytes: &[u8]) -> Result<()> {
    let runtime = render_runtime(yaml(bytes)?, read_mixin(ctx)?)?;
    let candidate = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    fs::write(candidate.path(), runtime)?;
    validate_config(ctx, candidate.path(), true)?;
    validate_with_mihomo(ctx, candidate.path())
}

/// Persist user overrides separately from subscription data. Validation occurs
/// before either file is published; the journal covers both files on a crash.
fn edit_mixin(
    ctx: &Context,
    edit: impl FnOnce(&mut serde_yaml::Mapping) -> Result<()>,
) -> Result<()> {
    ctx.check()?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    if matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo before changing persistent settings",
        ));
    }
    ensure_layout(ctx)?;
    storage::check_path(&ctx.paths.clash_mixin, true)?;
    let mut mixin = if ctx.paths.clash_mixin.is_file() {
        serde_yaml::from_slice::<Value>(&fs::read(&ctx.paths.clash_mixin)?)
            .map_err(|_| Error::new("error.data", "invalid Mixin YAML"))?
    } else {
        Value::Mapping(Default::default())
    };
    let map = mixin
        .as_mapping_mut()
        .ok_or_else(|| Error::new("error.data", "Mixin root must be a map"))?;
    edit(map)?;
    let runtime = runtime_with_mixin(ctx, mixin.clone())?;
    let candidate = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    fs::write(candidate.path(), &runtime)?;
    validate_config(ctx, candidate.path(), true)?;
    // Mihomo may echo YAML values in diagnostics, so avoid streaming secrets.
    if ctx.paths.clash_binary.is_file() {
        require_binary(ctx)?;
        let output = ctx.command(
            &path_string(&ctx.paths.clash_binary)?,
            &[
                "-d",
                &path_string(&ctx.paths.clash_resources)?,
                "-f",
                &path_string(candidate.path())?,
                "-t",
            ],
            false,
        )?;
        if !output.status.success() {
            return Err(Error::new(
                "error.verify",
                "Mihomo rejected settings; previous configuration retained",
            ));
        }
    }
    let bytes = serde_yaml::to_string(&mixin)?.into_bytes();
    commit_files(
        ctx,
        &[
            (
                ctx.paths.clash_mixin.clone(),
                Some(storage::Saved::new(&bytes, 0o600)),
            ),
            (
                ctx.paths.clash_runtime.clone(),
                Some(storage::Saved::new(&runtime, 0o600)),
            ),
        ],
    )
}

fn merge_config(mut base: Value, mixin: Value) -> Result<Value> {
    merge_map(&mut base, mixin)?;
    if !base.is_mapping() {
        return Err(Error::new(
            "error.data",
            "Clash configuration root must be a map",
        ));
    }
    Ok(base)
}

fn merge_map(base: &mut Value, mixin: Value) -> Result<()> {
    let Value::Mapping(base_map) = base else {
        return Err(Error::new("error.data", "Clash config root must be a map"));
    };
    let Value::Mapping(mixin_map) = mixin else {
        return Err(Error::new("error.data", "Clash mixin root must be a map"));
    };
    for (key, value) in mixin_map {
        if key.as_str() == Some("_custom") {
            continue;
        }
        let name = key.as_str().unwrap_or_default();
        if matches!(name, "rules" | "proxies" | "proxy-groups") {
            merge_special_sequence(base_map, &key, value)?;
        } else if let Some(existing) = base_map.get_mut(&key) {
            if existing.is_mapping() && value.is_mapping() {
                merge_map(existing, value)?;
            } else {
                *existing = value;
            }
        } else {
            base_map.insert(key, value);
        }
    }
    Ok(())
}

fn merge_special_sequence(map: &mut serde_yaml::Mapping, key: &Value, value: Value) -> Result<()> {
    let mixin = value
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Mixin list sections must be operation maps"))?;
    let base = map
        .entry(key.clone())
        .or_insert_with(|| Value::Sequence(Vec::new()));
    let original = base
        .as_sequence()
        .cloned()
        .ok_or_else(|| Error::new("error.data", "Clash config sequence has the wrong type"))?;
    let prepend = sequence(mixin, "prepend")?;
    let append = sequence(mixin, "append")?;
    let mut combined = prepend;
    if key.as_str() != Some("rules") {
        if let Some(overrides) = optional_sequence(mixin, "override")? {
            for item in original {
                let replacement = item.get("name").and_then(Value::as_str).and_then(|name| {
                    overrides.iter().find(|replacement| {
                        replacement.get("name").and_then(Value::as_str) == Some(name)
                    })
                });
                if let Some(replacement) = replacement {
                    combined.push(replacement.clone());
                } else {
                    combined.push(item);
                }
            }
        } else {
            combined.extend(original);
        }
    } else {
        if mixin.contains_key(Value::String("override".into())) {
            return Err(Error::new(
                "error.data",
                "rules mixin does not support override",
            ));
        }
        combined.extend(original);
    }
    combined.extend(append);
    if key.as_str() == Some("proxy-groups")
        && let Some(inject) = mixin
            .get(Value::String("inject".into()))
            .filter(|value| !value.is_null())
    {
        let injections = inject
            .as_mapping()
            .ok_or_else(|| Error::new("error.data", "proxy-groups.inject must be a map"))?;
        for group in &mut combined {
            let Some(name) = group.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(extra) = injections.get(Value::String(name.into())) else {
                continue;
            };
            let extra = extra.as_sequence().ok_or_else(|| {
                Error::new(
                    "error.data",
                    format!("proxy-groups.inject.{name} must be a list"),
                )
            })?;
            let Value::Mapping(group) = group else {
                return Err(Error::new("error.data", "proxy group must be a map"));
            };
            let proxies = group
                .entry(Value::String("proxies".into()))
                .or_insert_with(|| Value::Sequence(Vec::new()))
                .as_sequence_mut()
                .ok_or_else(|| Error::new("error.data", "proxy group proxies must be a list"))?;
            for value in extra {
                if !proxies.contains(value) {
                    proxies.push(value.clone());
                }
            }
        }
    }
    *base = Value::Sequence(combined);
    Ok(())
}

fn sequence(map: &serde_yaml::Mapping, key: &str) -> Result<Vec<Value>> {
    Ok(optional_sequence(map, key)?.unwrap_or_default())
}

fn optional_sequence(map: &serde_yaml::Mapping, key: &str) -> Result<Option<Vec<Value>>> {
    match map.get(Value::String(key.into())) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Sequence(values)) => Ok(Some(values.clone())),
        Some(_) => Err(Error::new(
            "error.data",
            format!("Mixin {key} must be a list"),
        )),
    }
}

fn validate_config(_ctx: &Context, path: &Path, allow_direct: bool) -> Result<()> {
    let value = yaml(&fs::read(path)?)?;
    let map = value
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Clash configuration root must be a map"))?;
    if map
        .get(Value::String("mode".into()))
        .and_then(Value::as_str)
        == Some("direct")
    {
        return if allow_direct {
            Ok(())
        } else {
            Err(Error::new(
                "error.data",
                "subscription config has no proxy sources",
            ))
        };
    }
    let has_nodes = map
        .get(Value::String("proxies".into()))
        .and_then(Value::as_sequence)
        .is_some_and(|items| !items.is_empty())
        || map
            .get(Value::String("proxy-providers".into()))
            .and_then(Value::as_mapping)
            .is_some_and(|items| !items.is_empty());
    if !has_nodes {
        return Err(Error::new(
            "error.data",
            "Clash config has no proxies or proxy-providers",
        ));
    }
    Ok(())
}

fn read_profiles(ctx: &Context) -> Result<ProfilesFile> {
    match fs::read(&ctx.paths.clash_profiles) {
        Ok(bytes) => serde_yaml::from_slice(&bytes)
            .map_err(|_| Error::new("error.data", "invalid subscription metadata YAML")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ProfilesFile::default()),
        Err(error) => Err(error.into()),
    }
}

fn profiles_bytes(profiles: &ProfilesFile) -> Result<Vec<u8>> {
    serde_yaml::to_string(profiles)
        .map(|value| value.into_bytes())
        .map_err(|e| Error::new("error.data", e))
}

fn activate_profile(ctx: &Context, profiles: &mut ProfilesFile, name: &str) -> Result<Vec<u8>> {
    let profile = profiles
        .profiles
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| Error::new("error.input", format!("unknown subscription: {name}")))?;
    let source = resolve_profile_path(ctx, &profile.path)?;
    let bytes = fs::read(&source)
        .map_err(|e| Error::new("error.data", format!("{}: {e}", source.display())))?;
    validate_config(ctx, &source, false)?;
    validate_with_mihomo(ctx, &source)?;
    validate_subscription_runtime(ctx, &bytes)?;
    profiles.current = Some(name.into());
    Ok(bytes)
}

fn validate_with_mihomo(ctx: &Context, path: &Path) -> Result<()> {
    if !ctx.paths.clash_binary.is_file() {
        return Ok(());
    }
    require_binary(ctx)?;
    let binary = path_string(&ctx.paths.clash_binary)?;
    let resources = path_string(&ctx.paths.clash_resources)?;
    let config = path_string(path)?;
    let output = ctx.command(&binary, &["-d", &resources, "-f", &config, "-t"], false)?;
    if !output.status.success() {
        return Err(Error::new(
            "error.verify",
            format!("Mihomo rejected configuration: {}", path.display()),
        ));
    }
    Ok(())
}

fn yaml(bytes: &[u8]) -> Result<Value> {
    serde_yaml::from_slice(bytes)
        .map_err(|_| Error::new("error.data", "invalid Mihomo configuration YAML"))
}

fn resolve_profile_path(ctx: &Context, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(Error::new("error.path", relative));
    }
    let path = ctx.paths.clash_data.join("profiles").join(path);
    storage::check_path(&path, false)?;
    Ok(path)
}

fn commit_files(ctx: &Context, updates: &[(PathBuf, Option<storage::Saved>)]) -> Result<()> {
    let pending = ctx.paths.clash_data.join("pending.json");
    storage::write_transaction(updates, &pending, || ctx.check())
}

fn recover_interrupted_files(ctx: &Context) -> Result<()> {
    let pending = ctx.paths.clash_data.join("pending.json");
    if !pending.exists() {
        return Ok(());
    }
    let bytes = fs::read(&pending)?;
    let before: BTreeMap<PathBuf, Option<storage::Saved>> = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new("error.pending", format!("{}: {e}", pending.display())))?;
    for path in before.keys() {
        let profile_dir = ctx.paths.clash_data.join("profiles");
        let profile_file = path.parent() == Some(profile_dir.as_path())
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|name| name.strip_suffix(".yaml"))
                .is_some_and(|stem| {
                    stem.len() == 24 && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
                });
        if path != &ctx.paths.clash_profiles
            && path != &ctx.paths.clash_mixin
            && path != &ctx.paths.clash_config
            && path != &ctx.paths.clash_runtime
            && !profile_file
        {
            return Err(Error::new("error.pending", path.display()));
        }
        storage::check_path(path, true)?;
    }
    for (path, saved) in before {
        storage::Saved::write(&path, &saved)?;
    }
    storage::remove_file(&pending)
}

fn process_start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    process_start_time_from_stat(&stat)
}

fn process_start_time_from_stat(stat: &str) -> Option<String> {
    let rest = stat.rsplit_once(") ")?.1;
    rest.split_whitespace().nth(19).map(str::to_owned)
}

fn validate_profile_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().any(char::is_control) || name.len() > 128 {
        return Err(Error::new("error.input", "invalid subscription name"));
    }
    Ok(())
}

fn validate_new_profile(profiles: &ProfilesFile, name: &str, url: &str) -> Result<()> {
    if profiles.profiles.iter().any(|profile| profile.name == name) {
        return Err(Error::new(
            "error.input",
            format!("duplicate subscription: {name}"),
        ));
    }
    if profiles.profiles.iter().any(|profile| profile.url == url) {
        return Err(Error::new("error.input", "subscription URL already exists"));
    }
    Ok(())
}

fn download_release_asset(ctx: &Context, url: &str, target: &Path, size: u64) -> Result<()> {
    let target = path_string(target)?;
    let max_size = size.to_string();
    ctx.run(
        "curl",
        &[
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-fSL",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--max-filesize",
            &max_size,
            "--output",
            &target,
            url,
        ],
    )?;
    let actual_size = fs::metadata(&target)?.len();
    if actual_size != size {
        return Err(Error::new(
            "error.release",
            format!("asset size: expected {size}, received {actual_size}"),
        ));
    }
    Ok(())
}

fn version_banner_matches(banner: &str, version: &str) -> bool {
    version_from_banner(banner).as_deref() == Some(version)
}

fn version_from_banner(banner: &str) -> Option<String> {
    if !(banner.starts_with("Mihomo") || banner.starts_with("Clash Meta")) {
        return None;
    }
    banner.split_whitespace().find_map(|token| {
        let token = token.strip_prefix('v')?;
        validate_version(token).ok()
    })
}

fn validate_subscription_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).map_err(|e| Error::new("error.url", e))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::new(
            "error.url",
            "subscription URL must be HTTPS without credentials",
        ));
    }
    Ok(())
}

fn profile_file_name(name: &str) -> String {
    format!("{}.yaml", &sha256_hex(name.as_bytes())[..24])
}

fn unused_profile_file_name(
    directory: &Path,
    profiles: &ProfilesFile,
    name: &str,
) -> Result<String> {
    let mut suffix = 0;
    loop {
        let file_name = if suffix == 0 {
            profile_file_name(name)
        } else {
            profile_file_name(&format!("{name}\0{suffix}"))
        };
        if !profiles
            .profiles
            .iter()
            .any(|profile| profile.path == file_name)
        {
            match fs::symlink_metadata(directory.join(&file_name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(file_name),
                Err(error) => return Err(error.into()),
                Ok(_) => (),
            }
        }
        suffix += 1;
    }
}

fn shell_quote(shell: &str, value: &str) -> String {
    match shell {
        "bash" | "zsh" => format!("'{}'", value.replace('\'', "'\\''")),
        "fish" => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => unreachable!(),
    }
}

fn proxy_url(scheme: &str, config: &ProxyConfig, port: u16) -> Result<String> {
    let mut url = url::Url::parse(&format!("{scheme}://localhost"))
        .map_err(|e| Error::new("error.url", e))?;
    url.set_host(Some(&config.host))
        .map_err(|_| Error::new("error.data", "invalid proxy bind address"))?;
    url.set_port(Some(port))
        .map_err(|_| Error::new("error.data", "invalid proxy port"))?;
    if let Some(user) = &config.username {
        url.set_username(user)
            .map_err(|_| Error::new("error.data", "invalid proxy username"))?;
    }
    if let Some(password) = &config.password {
        url.set_password(Some(password))
            .map_err(|_| Error::new("error.data", "invalid proxy password"))?;
    }
    Ok(url.to_string())
}

fn proxy_config(ctx: &Context) -> Result<ProxyConfig> {
    let path = if ctx.paths.clash_runtime.is_file() {
        &ctx.paths.clash_runtime
    } else {
        &ctx.paths.clash_config
    };
    let value = yaml(&fs::read(path)?)?;
    let map = value
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Clash config root must be a map"))?;
    let mixed = scalar_port(map.get(Value::String("mixed-port".into())))?;
    let http = scalar_port(map.get(Value::String("port".into())))?.or(mixed);
    let socks = scalar_port(map.get(Value::String("socks-port".into())))?.or(mixed);
    let allow_lan = map
        .get(Value::String("allow-lan".into()))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let bind = map
        .get(Value::String("bind-address".into()))
        .and_then(Value::as_str)
        .unwrap_or("*");
    let host = if allow_lan {
        local_host(bind)
    } else {
        "127.0.0.1".into()
    };
    let auth = map
        .get(Value::String("authentication".into()))
        .and_then(Value::as_sequence)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .and_then(|value| value.split_once(':'));
    Ok(ProxyConfig {
        host,
        http_port: http,
        socks_port: socks,
        username: auth.map(|(user, _)| user.into()),
        password: auth.map(|(_, password)| password.into()),
    })
}

fn proxy_address(ctx: &Context, socks: bool) -> Option<String> {
    let config = proxy_config(ctx).ok()?;
    let port = if socks {
        config.socks_port
    } else {
        config.http_port
    }?;
    let scheme = if socks { "socks5h" } else { "http" };
    Some(format!("{scheme}://{}:{}", format_host(&config.host), port))
}

fn scalar_port(value: Option<&Value>) -> Result<Option<u16>> {
    let Some(value) = value else { return Ok(None) };
    let port = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()));
    match port {
        None => Err(Error::new("error.data", "proxy port is not an integer")),
        Some(0) => Ok(None),
        Some(port @ 1..=65535) => Ok(Some(port as u16)),
        Some(_) => Err(Error::new("error.data", "proxy port is out of range")),
    }
}

fn local_host(bind: &str) -> String {
    match bind {
        "" | "*" | "0.0.0.0" => "127.0.0.1".into(),
        "::" | "[::]" => "::1".into(),
        value if value.parse::<IpAddr>().is_ok() => value.trim_matches(['[', ']']).into(),
        value => value.into(),
    }
}

fn format_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.into()
    }
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("error.path", path.display()))
}

fn pid_matches(ctx: &Context, pid: u32) -> bool {
    matches!(process_state(ctx, pid), State::Running(_))
}

fn process_state(ctx: &Context, pid: u32) -> State {
    if pid <= 1 {
        return State::Unknown;
    }
    #[cfg(target_os = "linux")]
    {
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return State::Stopped,
            Err(_) => return State::Unknown,
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            return State::Unknown;
        };
        if matches!(fields.split_whitespace().next(), Some("Z" | "X")) {
            return State::Stopped;
        }
        let Some(process_start) = process_start_time_from_stat(&stat) else {
            return State::Unknown;
        };
        let recorded_start = fs::read_to_string(&ctx.paths.clash_pid_start).ok();
        if recorded_start.as_deref().map(str::trim) != Some(process_start.as_str()) {
            return State::Unknown;
        }
        let command_line = match fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(command_line) => command_line,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return State::Stopped,
            Err(_) => return State::Unknown,
        };
        let expected_binary = ctx.paths.clash_binary.to_string_lossy();
        let expected_runtime = ctx.paths.clash_runtime.to_string_lossy();
        let command_line = String::from_utf8_lossy(&command_line);
        let argv = command_line.split('\0').collect::<Vec<_>>();
        if argv.len() >= 5
            && argv[0] == expected_binary
            && argv[1] == "-d"
            && argv[2] == ctx.paths.clash_resources.to_string_lossy()
            && argv[3] == "-f"
            && argv[4] == expected_runtime
        {
            State::Running(pid)
        } else {
            State::Unknown
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ctx;
        State::Unknown
    }
}

fn signal(pid: u32, force: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(std::io::Error::last_os_error().into())
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, force);
        Err(Error::new(
            "error.platform",
            "process control requires Unix",
        ))
    }
}

fn tail(value: &str) -> String {
    value
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panels pick the format by User-Agent: a Clash identifier gets the native config,
    /// anything else gets a Base64 share-link list, as the provider in the bug report did.
    #[cfg(unix)]
    #[test]
    fn subscriptions_request_the_clash_format_and_reject_share_link_lists() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let curl = bin.join("curl");
        fs::write(
            &curl,
            r#"#!/bin/sh
out= agent=none
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) shift; out=$1 ;;
    --user-agent) shift; agent=$1 ;;
  esac
  shift
done
printf '%s\n' "$agent" >> "$CQS_TEST_AGENT_LOG"
if [ "$agent" = clash.meta ] && [ -z "$CQS_TEST_LINKS_ONLY" ]; then
  printf 'proxies:\n  - name: one\n    type: socks5\n    server: 127.0.0.1\n    port: 1\n' > "$out"
else
  printf 'dm1lc3M6Ly9leGFtcGxl\n' > "$out"
fi
"#,
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        ctx.environment
            .insert("PATH".into(), format!("{}:{path}", bin.display()));
        let log = temp.path().join("agents.log");
        ctx.environment
            .insert("CQS_TEST_AGENT_LOG".into(), log.display().to_string());
        let target = temp.path().join("subscription.yaml");
        let url = "https://panel.example/api/v1/client/subscribe?token=test";

        download_subscription(&ctx, url, &target).unwrap();
        validate_config(&ctx, &target, false).unwrap();
        ctx.download(url, &target, 5).unwrap();
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "clash.meta\nnone\n",
            "only subscription downloads change the User-Agent"
        );

        ctx.environment
            .insert("CQS_TEST_LINKS_ONLY".into(), "1".into());
        let error = download_subscription(&ctx, url, &target).unwrap_err();
        assert_eq!(error.key, "error.subscription_format");
    }

    #[test]
    fn merge_mixin_lists_support_prepend_append_and_named_override() {
        let base: Value =
            serde_yaml::from_str("rules: [A]\nproxies:\n  - name: one\n    type: direct\n")
                .unwrap();
        let mixin: Value = serde_yaml::from_str(
            "rules:\n  prepend: [P]\n  append: [Q]\nproxies:\n  override:\n    - name: one\n      type: socks5\n  append:\n    - name: two\n      type: direct\n",
        )
        .unwrap();
        let result = merge_config(base, mixin).unwrap();
        let map = result.as_mapping().unwrap();
        assert_eq!(
            map[Value::String("rules".into())]
                .as_sequence()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            map[Value::String("proxies".into())].as_sequence().unwrap()[0]["type"],
            "socks5"
        );
        assert_eq!(
            map[Value::String("proxies".into())].as_sequence().unwrap()[1]["name"],
            "two"
        );
    }

    #[test]
    fn mixin_override_only_replaces_matching_named_items() {
        let base: Value =
            serde_yaml::from_str("proxies:\n  - name: existing\n    type: direct\n").unwrap();
        let mixin: Value = serde_yaml::from_str(
            "proxies:\n  override:\n    - name: missing\n      type: socks5\n",
        )
        .unwrap();

        let result = merge_config(base, mixin).unwrap();
        let proxies = result["proxies"].as_sequence().unwrap();
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0]["name"], "existing");
    }

    #[test]
    fn mixin_special_sections_must_use_operation_maps() {
        let base: Value = serde_yaml::from_str("rules: [MATCH,DIRECT]\n").unwrap();
        let mixin: Value = serde_yaml::from_str("rules: [DOMAIN,example.com,DIRECT]\n").unwrap();

        assert!(merge_config(base, mixin).is_err());
    }

    #[test]
    fn release_banner_requires_an_exact_version_token() {
        assert!(version_banner_matches(
            "Mihomo v1.2.3 linux amd64",
            "v1.2.3"
        ));
        assert!(!version_banner_matches(
            "Mihomo v1.2.30 linux amd64",
            "v1.2.3"
        ));
        assert!(!version_banner_matches("other Mihomo v1.2.3", "v1.2.3"));
        assert_eq!(
            version_from_banner("Mihomo v1.2.3 linux amd64").as_deref(),
            Some("v1.2.3")
        );
        assert_eq!(version_from_banner("Mihomo development build"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn release_asset_download_limit_applies_to_chunked_responses() {
        use std::{
            io::{BufRead, BufReader},
            process::{Command, Stdio},
        };

        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        let cert = temp.path().join("cert.pem");
        let key = temp.path().join("key.pem");
        let generated = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=DNS:localhost",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .output()
            .unwrap();
        assert!(generated.status.success());

        let fixture = r#"
import http.server, ssl, sys
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def do_GET(self):
        self.send_response(200)
        self.send_header('Transfer-Encoding', 'chunked')
        self.end_headers()
        try:
            for _ in range(8):
                self.wfile.write(b'40\r\n' + b'x' * 64 + b'\r\n')
                self.wfile.flush()
            self.wfile.write(b'0\r\n\r\n')
        except (BrokenPipeError, ConnectionResetError):
            pass
    def log_message(self, *args): pass
server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(sys.argv[1], sys.argv[2])
server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_port, flush=True)
server.serve_forever()
"#;
        let mut server = Command::new("python3")
            .args(["-u", "-c", fixture])
            .arg(&cert)
            .arg(&key)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        struct Server(std::process::Child);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut port = String::new();
        BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let _server = Server(server);
        ctx.environment
            .insert("CURL_CA_BUNDLE".into(), cert.display().to_string());
        ctx.environment
            .insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
        let target = temp.path().join("mihomo.gz");
        let url = format!("https://localhost:{}/asset", port.trim());

        assert!(download_release_asset(&ctx, &url, &target, 16).is_err());
        assert!(fs::metadata(target).map_or(true, |metadata| metadata.len() <= 16));

        let valid_target = temp.path().join("valid.gz");
        download_release_asset(&ctx, &url, &valid_target, 512).unwrap();
        assert_eq!(fs::metadata(valid_target).unwrap().len(), 512);
    }

    #[test]
    fn deleting_a_stopped_current_subscription_clears_its_configs() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let name = "selected";
        let path = profile_file_name(name);
        let profile_path = ctx.paths.clash_data.join("profiles").join(&path);
        fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
        fs::write(&profile_path, b"mode: rule\nproxies: []\n").unwrap();
        let profiles = ProfilesFile {
            current: Some(name.into()),
            profiles: vec![Profile {
                name: name.into(),
                url: "https://example.invalid/sub".into(),
                path,
                updated: String::new(),
                extra: BTreeMap::new(),
            }],
            extra: BTreeMap::new(),
        };
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        fs::write(
            &ctx.paths.clash_profiles,
            profiles_bytes(&profiles).unwrap(),
        )
        .unwrap();
        fs::write(&ctx.paths.clash_config, b"old config").unwrap();
        fs::write(&ctx.paths.clash_runtime, b"old runtime").unwrap();

        subscription_remove(&ctx, name).unwrap();

        let saved = read_profiles(&ctx).unwrap();
        assert_eq!(saved.current, None);
        assert!(saved.profiles.is_empty());
        assert!(!profile_path.exists());
        assert!(!ctx.paths.clash_config.exists());
        assert!(!ctx.paths.clash_runtime.exists());
    }

    #[test]
    fn proxy_group_inject_adds_each_member_once() {
        let base: Value =
            serde_yaml::from_str("proxy-groups:\n  - name: Default\n    proxies: [one]\n").unwrap();
        let mixin: Value =
            serde_yaml::from_str("proxy-groups:\n  inject:\n    Default: [one, two]\n").unwrap();
        let result = merge_config(base, mixin).unwrap();
        let proxies = result["proxy-groups"][0]["proxies"].as_sequence().unwrap();
        assert_eq!(
            proxies,
            &[Value::String("one".into()), Value::String("two".into())]
        );
    }

    #[test]
    fn shell_proxy_values_are_quoted_for_the_selected_shell() {
        assert_eq!(shell_quote("bash", "a'b"), "'a'\\''b'");
        assert_eq!(shell_quote("fish", "a'b\\c"), "'a\\'b\\\\c'");
    }

    #[test]
    fn interrupted_subscription_transaction_restores_previous_file() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let target = ctx.paths.clash_data.join("profiles").join("profile.yaml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"old").unwrap();
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            commit_files(
                &ctx,
                &[(target.clone(), Some(storage::Saved::new(b"new", 0o600)),)]
            )
            .is_err()
        );
        assert_eq!(fs::read(target).unwrap(), b"old");
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn process_start_time_parser_handles_parentheses_in_comm_name() {
        let fields = (1..=19)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let stat = format!("42 (mihomo ) worker) S {fields}");
        assert_eq!(process_start_time_from_stat(&stat), Some("19".into()));
        assert_eq!(process_start_time_from_stat("not proc stat"), None);
    }

    #[test]
    fn profile_metadata_round_trips_unknown_fields() {
        let value: ProfilesFile = serde_yaml::from_str(
            "use: main\ncustom: keep\nprofiles:\n  - name: main\n    url: https://example.invalid/sub\n    path: 0123456789abcdef01234567.yaml\n    provider: test\n",
        )
        .unwrap();
        let serialized = String::from_utf8(profiles_bytes(&value).unwrap()).unwrap();
        assert!(serialized.contains("custom: keep"));
        assert!(serialized.contains("provider: test"));
    }

    #[test]
    fn profile_validation_rejects_credentials_and_non_https_urls() {
        assert!(validate_subscription_url("http://example.invalid/sub").is_err());
        assert!(validate_subscription_url("https://user:pass@example.invalid/sub").is_err());
        assert!(validate_subscription_url("https://example.invalid/sub").is_ok());
    }
}
