//! Explicit TUN configuration for the directly managed native Linux process.
//! No implicit privilege escalation or host network changes are performed here.

use super::{State, build_runtime_config, edit_mixin, state};
use crate::{
    error::{Error, Result},
    runtime::Context,
    storage,
};
use serde_yaml::{Mapping, Value};
use std::{fs, path::Path};

#[derive(Debug, PartialEq, Eq)]
struct Settings {
    enabled: bool,
    device: String,
    stack: String,
    auto_route: bool,
    dns_hijack: bool,
}

fn key(name: &str) -> Value {
    Value::String(name.into())
}

fn settings(value: &Value) -> Result<Settings> {
    let root = value
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Clash configuration root must be a map"))?;
    let Some(tun) = root.get(key("tun")) else {
        return Ok(Settings {
            enabled: false,
            device: "cqs-tun".into(),
            stack: "mixed".into(),
            auto_route: true,
            dns_hijack: false,
        });
    };
    let tun = tun
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "tun must be a map"))?;
    let string = |name: &str, default: &str| -> Result<String> {
        match tun.get(key(name)) {
            Some(Value::String(value)) => Ok(value.clone()),
            None => Ok(default.into()),
            _ => Err(Error::new(
                "error.data",
                format!("tun.{name} must be a string"),
            )),
        }
    };
    let boolean = |name: &str, default: bool| -> Result<bool> {
        match tun.get(key(name)) {
            Some(Value::Bool(value)) => Ok(*value),
            None => Ok(default),
            _ => Err(Error::new(
                "error.data",
                format!("tun.{name} must be a boolean"),
            )),
        }
    };
    let dns_hijack = match tun.get(key("dns-hijack")) {
        None => false,
        Some(Value::Sequence(values)) if values.iter().all(Value::is_string) => !values.is_empty(),
        _ => {
            return Err(Error::new(
                "error.data",
                "tun.dns-hijack must be a string sequence",
            ));
        }
    };
    let settings = Settings {
        enabled: boolean("enable", false)?,
        device: string("device", "Meta")?,
        stack: string("stack", "mixed")?,
        auto_route: boolean("auto-route", true)?,
        dns_hijack,
    };
    validate_options(&settings.stack, &settings.device)?;
    Ok(settings)
}

fn validate_options(stack: &str, device: &str) -> Result<()> {
    if !matches!(stack, "system" | "gvisor" | "mixed") {
        return Err(Error::new(
            "error.input",
            "TUN stack must be system, gvisor or mixed",
        ));
    }
    // Linux IFNAMSIZ includes the trailing NUL. Reserve punctuation that has
    // special meaning in tools and forbid path components used by sysfs reads.
    if device.is_empty()
        || device.len() > 15
        || !device
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || !device.as_bytes()[0].is_ascii_alphabetic()
    {
        return Err(Error::new(
            "error.input",
            "TUN device must start with a letter and contain 1-15 ASCII letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn restricted_environment(kernel: &str, cgroup: &str, container_marker: bool) -> bool {
    let kernel = kernel.to_ascii_lowercase();
    container_marker
        || kernel.contains("microsoft")
        || kernel.contains("wsl")
        || [
            "docker",
            "containerd",
            "kubepods",
            "lxc",
            "podman",
            "libpod",
        ]
        .iter()
        .any(|name| cgroup.contains(name))
}

#[cfg(any(target_os = "linux", test))]
fn has_net_admin(status: &str) -> Result<bool> {
    let capabilities = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
        .ok_or_else(|| Error::new("error.platform", "cannot determine CAP_NET_ADMIN"))?;
    Ok(capabilities & (1 << 12) != 0)
}

fn require_native(ctx: &Context, device: &str) -> Result<()> {
    ctx.root()?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")?;
        let cgroup = fs::read_to_string("/proc/1/cgroup")?;
        let marker = [
            "/.dockerenv",
            "/run/.containerenv",
            "/run/systemd/container",
        ]
        .iter()
        .any(|path| Path::new(path).exists());
        if restricted_environment(&kernel, &cgroup, marker) {
            return Err(Error::new(
                "error.platform",
                "TUN requires native Linux; WSL and containers are not supported",
            ));
        }
        if !has_net_admin(&fs::read_to_string("/proc/self/status")?)? {
            return Err(Error::new(
                "error.root",
                "TUN requires root with CAP_NET_ADMIN",
            ));
        }
        let metadata = fs::metadata("/dev/net/tun")
            .map_err(|error| Error::new("error.platform", format!("/dev/net/tun: {error}")))?;
        if !metadata.file_type().is_char_device()
            || libc::major(metadata.rdev()) != 10
            || libc::minor(metadata.rdev()) != 200
        {
            return Err(Error::new(
                "error.platform",
                "/dev/net/tun is not the Linux TUN device",
            ));
        }
        match fs::symlink_metadata(Path::new("/sys/class/net").join(device)) {
            Ok(_) => {
                return Err(Error::new(
                    "error.busy",
                    format!("TUN device {device} already exists; choose an unused device name"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        Err(Error::new("error.platform", "TUN requires native Linux"))
    }
}

/// Called immediately before every process start, including manually edited
/// configurations, so configuration-time checks cannot be bypassed later.
pub(super) fn validate_start(ctx: &Context, runtime: &[u8]) -> Result<()> {
    let settings = settings(&serde_yaml::from_slice(runtime)?)?;
    if settings.enabled {
        require_native(ctx, &settings.device)?;
    }
    Ok(())
}

fn map_mut<'a>(map: &'a mut Mapping, name: &str) -> Result<&'a mut Mapping> {
    map.entry(key(name))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| Error::new("error.data", format!("{name} must be a map")))
}

pub fn configure(
    ctx: &Context,
    enabled: bool,
    stack: &str,
    device: &str,
    auto_route: bool,
    dns_hijack: bool,
) -> Result<()> {
    validate_options(stack, device)?;
    if enabled {
        require_native(ctx, device)?;
    }
    edit_mixin(ctx, |mixin| {
        let tun = map_mut(mixin, "tun")?;
        tun.insert(key("enable"), Value::Bool(enabled));
        if enabled {
            tun.insert(key("device"), key(device));
            tun.insert(key("stack"), key(stack));
            tun.insert(key("auto-route"), Value::Bool(auto_route));
            tun.insert(
                key("dns-hijack"),
                Value::Sequence(if dns_hijack {
                    vec![key("any:53")]
                } else {
                    vec![]
                }),
            );
            if dns_hijack {
                map_mut(mixin, "dns")?.insert(key("enable"), Value::Bool(true));
            }
        }
        Ok(())
    })?;
    ctx.message(
        "notice.clash_tun",
        if enabled {
            "enabled in configuration; start Mihomo as root to activate"
        } else {
            "disabled in configuration"
        },
    );
    Ok(())
}

pub(super) fn set_enabled(ctx: &Context, enabled: bool) -> Result<()> {
    edit_mixin(ctx, |mixin| {
        if enabled {
            let runtime = super::runtime_with_mixin(ctx, Value::Mapping(mixin.clone()))?;
            let settings = settings(&serde_yaml::from_slice(&runtime)?)?;
            require_native(ctx, &settings.device)?;
            map_mut(mixin, "tun")?.insert(key("device"), key(&settings.device));
        }
        map_mut(mixin, "tun")?.insert(key("enable"), Value::Bool(enabled));
        Ok(())
    })?;
    ctx.message(
        "notice.clash_tun",
        if enabled {
            "enabled in configuration; start Mihomo as root to activate"
        } else {
            "disabled in configuration"
        },
    );
    Ok(())
}

fn flags_up(flags: &str) -> Result<bool> {
    let flags = flags.trim();
    u32::from_str_radix(flags.strip_prefix("0x").unwrap_or(flags), 16)
        .map(|value| value & 1 != 0)
        .map_err(|_| Error::new("error.data", "invalid network interface flags"))
}

fn contains_device(fdinfo: &str, device: &str) -> bool {
    fdinfo.lines().any(|line| {
        line.strip_prefix("iff:")
            .is_some_and(|name| name.trim() == device)
    })
}

fn owns_device(pid: u32, device: &str) -> Result<bool> {
    let before = super::process_start_time(pid)
        .ok_or_else(|| Error::new("error.verify", "Mihomo process disappeared"))?;
    let mut readable = false;
    let mut owned = false;
    for entry in fs::read_dir(format!("/proc/{pid}/fdinfo"))? {
        let entry = entry?;
        match fs::read_to_string(entry.path()) {
            Ok(contents) => {
                readable = true;
                owned |= contains_device(&contents, device);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    if !readable || super::process_start_time(pid).as_deref() != Some(before.as_str()) {
        return Err(Error::new(
            "error.verify",
            "cannot verify TUN process ownership",
        ));
    }
    Ok(owned)
}

pub fn status(ctx: &Context) -> Result<()> {
    let process = state(ctx)?;
    let bytes = if matches!(process, State::Running(_)) {
        storage::check_path(&ctx.paths.clash_runtime, true)?;
        fs::read(&ctx.paths.clash_runtime)?
    } else {
        build_runtime_config(ctx)?
    };
    let settings = settings(&serde_yaml::from_slice(&bytes)?)?;
    let flags = Path::new("/sys/class/net")
        .join(&settings.device)
        .join("flags");
    let link = match fs::read_to_string(flags) {
        Ok(flags) => {
            if flags_up(&flags)? {
                "up"
            } else {
                "down"
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent",
        Err(error) => return Err(error.into()),
    };
    let ownership = match process {
        State::Running(pid) if link != "absent" => match owns_device(pid, &settings.device) {
            Ok(true) => "owned by managed Mihomo",
            Ok(false) => "not owned by managed Mihomo",
            Err(error) => {
                ctx.message(
                    "notice.clash_tun",
                    format!("ownership unavailable: {error}"),
                );
                "unverified"
            }
        },
        _ => "no managed process",
    };
    ctx.message("notice.clash_tun", format!("configured={}; device={}; stack={}; auto-route={}; dns-hijack={}; link={link}; {ownership}", settings.enabled, settings.device, settings.stack, settings.auto_route, settings.dns_hijack));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_and_stack_are_strict() {
        for device in ["cqs-tun", "Meta", "tun0", "a12345678901234"] {
            validate_options("mixed", device).unwrap();
        }
        for device in [
            "",
            "../tun",
            "tun/0",
            "tun.0",
            "0tun",
            "tun\n0",
            "测试",
            "a123456789012345",
        ] {
            assert!(validate_options("mixed", device).is_err(), "{device:?}");
        }
        assert!(validate_options("unknown", "Meta").is_err());
    }

    #[test]
    fn detects_wsl_containers_and_missing_capability() {
        assert!(!restricted_environment(
            "6.12.0-amd64",
            "0::/init.scope",
            false
        ));
        assert!(restricted_environment(
            "6.6.87.2-microsoft-standard-WSL2",
            "0::/",
            false
        ));
        assert!(restricted_environment("6.12", "0::/docker/abc", false));
        assert!(restricted_environment("6.12", "0::/", true));
        assert!(has_net_admin("CapEff:\t0000000000001000\n").unwrap());
        assert!(!has_net_admin("CapEff:\t0000000000000000\n").unwrap());
        assert!(has_net_admin("CapEff: not-hex\n").is_err());
        assert!(has_net_admin("").is_err());
    }

    #[test]
    fn settings_reject_mistyped_fields_and_use_defaults() {
        let configured = settings(&serde_yaml::from_str("tun:\n  enable: true\n  device: cqs-tun\n  stack: system\n  auto-route: false\n  dns-hijack: ['any:53']\n").unwrap()).unwrap();
        assert!(configured.enabled);
        assert!(configured.dns_hijack);
        assert!(!configured.auto_route);
        assert_eq!(configured.stack, "system");
        for yaml in [
            "tun: []",
            "tun: {enable: yes}",
            "tun: {device: 123}",
            "tun: {stack: invalid}",
            "tun: {dns-hijack: 'any:53'}",
            "tun: {dns-hijack: [1]}",
        ] {
            assert!(
                settings(&serde_yaml::from_str(yaml).unwrap()).is_err(),
                "{yaml}"
            );
        }
        assert!(
            !settings(&serde_yaml::from_str("mode: direct").unwrap())
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn interface_checks_require_exact_device_and_administrative_up() {
        assert!(contains_device("pos:\t0\niff:\tcqs-tun\n", "cqs-tun"));
        assert!(!contains_device("iff:\tcqs-tun-other\n", "cqs-tun"));
        assert!(!contains_device("other:\tcqs-tun\n", "cqs-tun"));
        assert!(flags_up("0x1003\n").unwrap());
        assert!(!flags_up("0x1002\n").unwrap());
        assert!(flags_up("garbage").is_err());
    }

    #[test]
    fn disabled_runtime_does_not_require_privileges_or_touch_network() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        validate_start(&ctx, b"mode: direct\ntun: {enable: false}\n").unwrap();
        assert!(!ctx.paths.clash_mixin.exists());
    }

    #[test]
    fn disabling_only_changes_enable_and_keeps_subscription_and_dns() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        let base = b"mode: direct\ntun: {enable: true, device: Meta}\n";
        fs::write(&ctx.paths.clash_config, base).unwrap();
        fs::write(&ctx.paths.clash_mixin, b"tun: {enable: true, device: previous, stack: gvisor}\ndns: {enable: true}\ncustom: keep\n").unwrap();

        configure(&ctx, false, "mixed", "cqs-tun", true, true).unwrap();

        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["tun"]["enable"], false);
        assert_eq!(mixin["tun"]["device"], "previous");
        assert_eq!(mixin["tun"]["stack"], "gvisor");
        assert_eq!(mixin["dns"]["enable"], true);
        assert_eq!(mixin["custom"], "keep");
        let runtime: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_runtime).unwrap()).unwrap();
        assert_eq!(runtime["tun"]["enable"], false);
        assert_eq!(fs::read(&ctx.paths.clash_config).unwrap(), base);
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn cancelled_disable_preserves_existing_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        let original = b"tun: {enable: true}\n";
        fs::write(&ctx.paths.clash_mixin, original).unwrap();
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(configure(&ctx, false, "mixed", "cqs-tun", true, true).is_err());
        assert_eq!(fs::read(&ctx.paths.clash_mixin).unwrap(), original);
        assert!(!ctx.paths.clash_runtime.exists());
    }
}
