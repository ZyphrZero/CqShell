use std::{
    fs,
    process::{Command, Stdio},
};

fn cli(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cqs"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CQS_LANG")
        .env_remove("CQS_GH_PROXY")
        .env_remove("GH_PROXY")
        .stdin(Stdio::null());
    command
}
#[test]
fn metadata_commands_have_no_startup_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        &["--help"][..],
        &["--version"],
        &["--lang", "zh-CN", "languages"],
        &["list", "--json"],
        &["proxy-env", "off"],
        &["proxy-env", "off", "--shell", "fish"],
    ] {
        let output = cli(temp.path()).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn proxy_environment_command_is_shell_specific_and_has_no_filesystem_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(temp.path())
        .args(["proxy-env", "off", "--shell", "fish"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "set -e http_proxy\nset -e https_proxy\nset -e all_proxy\nset -e no_proxy\nset -e HTTP_PROXY\nset -e HTTPS_PROXY\nset -e ALL_PROXY\nset -e NO_PROXY\ntrue\n"
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[test]
fn noninteractive_start_and_unknown_actions_fail_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        &[][..],
        &["run", "unknown"],
        &["--wat"],
        &["run", "bbr.enable", "--set", "qdisc=invalid"],
        &["run", "test.ip", "--set", "ip_family=ipv4;id"],
        &["run", "test.ip", "--set", "full_ip=yes"],
        &["batch", "ssh.key"],
    ] {
        let output = cli(temp.path()).args(args).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
#[test]
fn common_tools_and_ai_have_individual_bilingual_entries() {
    for locale in ["zh-CN", "en-US"] {
        let temp = tempfile::tempdir().unwrap();
        let output = cli(temp.path())
            .args(["--lang", locale, "list", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let rows = rows.as_array().unwrap();
        for package in [
            "curl", "wget", "nano", "unzip", "tar", "tmux", "iftop", "btop", "gdu", "fzf",
        ] {
            let id = format!("tools.{package}");
            let row = rows.iter().find(|r| r["id"] == id).expect(&id);
            assert_eq!(row["group"], "tools");
            assert_eq!(row["parameters"], serde_json::json!([]));
            assert_eq!(row["root"], true);
            assert!(row["label"].as_str().unwrap().contains(package));
        }
        let all = rows.iter().find(|r| r["id"] == "tools.install").unwrap();
        assert_eq!(all["parameters"][0]["default"], "all");
        for id in ["ai.pi.install", "ai.claude.install", "ai.codex.install"] {
            let row = rows.iter().find(|r| r["id"] == id).expect(id);
            assert_eq!(row["group"], "ai");
            assert_eq!(row["root"], false);
            assert_eq!(row["interactive"], false);
            assert_eq!(row["mutates"], true);
        }
        let status = rows.iter().find(|r| r["id"] == "ai.status").unwrap();
        assert_eq!(status["mutates"], false);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn command_line_language_overrides_environment_and_toml_preference() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("preferences.toml");
    fs::write(&config, "language = 'zh-CN'\n").unwrap();
    let output = cli(temp.path())
        .args(["--config", config.to_str().unwrap(), "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("服务器工具箱"));
    let output = cli(temp.path())
        .env("CQS_LANG", "en-US")
        .args(["--config", config.to_str().unwrap(), "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("server toolbox"));
    let output = cli(temp.path())
        .env("CQS_LANG", "en-US")
        .args([
            "--config",
            config.to_str().unwrap(),
            "--lang",
            "zh-CN",
            "--help",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("服务器工具箱"));
}
#[test]
fn list_json_exposes_stable_ids_and_parameter_schema() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(temp.path())
        .args(["--lang", "zh-CN", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ssh = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "ssh.key")
        .unwrap();
    assert_eq!(ssh["parameters"][0]["name"], "public_key");
    assert_eq!(ssh["parameters"][0]["required"], true);
    let quality = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "test.ip")
        .unwrap();
    assert!(
        quality["label"]
            .as_str()
            .unwrap()
            .contains("xykt/IPQuality")
    );
    assert_eq!(quality["interactive"], true);
    assert_eq!(quality["parameters"][0]["name"], "ip_family");
    assert_eq!(quality["parameters"][0]["default"], "dual");
    assert_eq!(quality["parameters"][1]["name"], "full_ip");
    assert_eq!(quality["parameters"][1]["default"], "off");
    let kernel = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "kernel.install")
        .unwrap();
    assert_eq!(kernel["available"], true);
    assert!(kernel["unavailable_reason"].is_null());
    for id in ["kernel.list", "kernel.remove", "bbr.enable"] {
        let action = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap();
        assert_eq!(action["available"], true);
    }
}

#[test]
fn kernel_install_exposes_standard_max_and_optional_version_in_both_languages() {
    for locale in ["zh-CN", "en-US"] {
        let temp = tempfile::tempdir().unwrap();
        let output = cli(temp.path())
            .args(["--lang", locale, "--yes", "list", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let actions: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let kernel = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "kernel.install")
            .unwrap();
        assert_eq!(kernel["available"], true);
        assert!(kernel["unavailable_reason"].is_null());
        let fields = kernel["parameters"].as_array().unwrap();
        let profile = fields.iter().find(|f| f["name"] == "profile").unwrap();
        assert_eq!(profile["choices"], serde_json::json!(["standard", "max"]));
        let version = fields.iter().find(|f| f["name"] == "version").unwrap();
        assert_eq!(version["default"], "");
        assert_eq!(version["required"], false);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn tui_pty_start_language_switch_and_exit_restores_terminal() {
    tui_pty(None);
}

#[cfg(target_os = "linux")]
#[test]
fn tui_pty_cancels_running_task_from_logs_and_restores_terminal() {
    for exit_key in [&b"\x03"[..], &b"q"[..]] {
        tui_pty(Some(exit_key));
    }
}

#[cfg(target_os = "linux")]
fn tui_pty(exit_key: Option<&[u8]>) {
    use std::{
        fs::File,
        io::{Read, Write},
        os::fd::{AsRawFd, FromRawFd},
        os::unix::{fs::PermissionsExt, process::CommandExt},
        thread,
        time::{Duration, Instant},
    };
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 32,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    let mut master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut before = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), before.as_mut_ptr()) },
        0
    );
    let before = unsafe { before.assume_init() };
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    if exit_key.is_some() {
        let config = temp.path().join("config/cqs");
        fs::create_dir_all(&config).unwrap();
        // Target a stable category ID instead of depending on the default
        // category order, which may place a root-only action at this offset.
        fs::write(config.join("config.toml"), "category_order = ['ai']\n").unwrap();
    }
    fs::write(
        bin.join("curl"),
        r#"#!/usr/bin/python3
import os, signal, sys, time
from pathlib import Path
root = Path(os.environ['CQS_PTY_ROOT'])
root.joinpath('child.pid').write_text(str(os.getpid()))
def interrupted(*_):
    root.joinpath('child.cancelled').write_text('SIGINT')
    sys.exit(0)
signal.signal(signal.SIGINT, interrupted)
try:
    fd = os.open('/dev/tty', os.O_RDONLY | os.O_NONBLOCK)
except OSError:
    print('CQS_NO_CTTY', flush=True)
else:
    os.close(fd)
    print('CQS_HAS_CTTY', flush=True)
print('CQS_CANCEL_READY', flush=True)
time.sleep(8)
sys.exit(23)
"#,
    )
    .unwrap();
    fs::set_permissions(bin.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = cli(temp.path());
    command
        .env("TERM", "xterm-256color")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("CQS_STATE_DIR", temp.path().join("state"))
        .env("CQS_PTY_ROOT", temp.path())
        .args(["--lang", "en-US"])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap());
    // Model an actual SSH terminal: the TUI owns a controlling PTY.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    unsafe {
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let start = Instant::now();
    let mut switched = false;
    let mut quit = false;
    let mut started_task = false;
    let mut cursor_queries = 0;
    let ansi = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap();
    loop {
        match master.read(&mut buffer) {
            Ok(n) if n > 0 => bytes.extend_from_slice(&buffer[..n]),
            Ok(_) => (),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => (),
        }
        // A PTY transports bytes; emulate the terminal's standard cursor-position reply.
        let queries = bytes.windows(4).filter(|w| *w == b"\x1b[6n").count();
        while cursor_queries < queries {
            master.write_all(b"\x1b[1;1R").unwrap();
            cursor_queries += 1;
        }
        if !switched && bytes.windows(7).any(|w| w == b"CqShell") {
            master.write_all(b"l").unwrap();
            switched = true;
        }
        if switched
            && !quit
            && !started_task
            && ansi
                .replace_all(&String::from_utf8_lossy(&bytes), "")
                .contains("功能分类")
        {
            if exit_key.is_some() {
                // Jump from Overview to the first category (Agent tools via '1'),
                // run its first user action, and open the full logs before cancelling.
                master.write_all(b"1\r\x1bOQ").unwrap();
                started_task = true;
            } else {
                master.write_all(b"q").unwrap();
                quit = true;
            }
        }
        if started_task && !quit && bytes.windows(16).any(|part| part == b"CQS_CANCEL_READY") {
            master.write_all(exit_key.unwrap()).unwrap();
            quit = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{}", String::from_utf8_lossy(&bytes));
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("TUI timeout: {}", String::from_utf8_lossy(&bytes));
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(switched && quit);
    if exit_key.is_none() {
        let preferences = temp.path().join("config/cqs/config.toml");
        let saved = fs::read_to_string(preferences).expect("language preference was saved");
        assert!(saved.contains("language = \"zh-CN\""), "{saved}");
        let restarted = cli(temp.path()).arg("--help").output().unwrap();
        assert!(restarted.status.success());
        assert!(String::from_utf8_lossy(&restarted.stdout).contains("服务器工具箱"));
    }
    if exit_key.is_some() {
        let pid = fs::read_to_string(temp.path().join("child.pid")).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "Cancelled command was not reaped"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("child.cancelled")).unwrap(),
            "SIGINT"
        );
        assert!(
            bytes.windows(11).any(|part| part == b"CQS_NO_CTTY"),
            "Captured command inherited the controlling terminal"
        );
    }
    let mut after = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), after.as_mut_ptr()) },
        0
    );
    let after = unsafe { after.assume_init() };
    assert_eq!(
        after.c_lflag & (libc::ICANON | libc::ECHO),
        before.c_lflag & (libc::ICANON | libc::ECHO)
    );
}

#[test]
fn github_proxy_action_and_cli_overrides_validate_and_persist() {
    let temp = tempfile::tempdir().unwrap();
    let help = cli(temp.path()).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--gh-proxy"));

    // Invalid overrides fail with exit code 2 without creating any files.
    for args in [
        &["--gh-proxy", "http://gh-proxy.com/", "list"][..],
        &["--gh-proxy", "https://user@gh-proxy.com/", "list"],
        &["--gh-proxy", "https://gh-proxy.com/?k=v", "list"],
        &["--gh-proxy", "https:///gh-proxy.com/", "list"],
        &[
            "run",
            "github.proxy",
            "--yes",
            "--set",
            "gh_proxy_url=http://insecure.example/",
        ],
    ] {
        let out = cli(temp.path()).args(args).output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    // Invalid env vars are also rejected unless overridden by `--gh-proxy off`.
    let bad_env = cli(temp.path())
        .env("GH_PROXY", "http://bad.example/")
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(bad_env.status.code(), Some(2));
    let overridden = cli(temp.path())
        .env("GH_PROXY", "http://bad.example/")
        .args(["--gh-proxy", "off", "list"])
        .output()
        .unwrap();
    assert!(overridden.status.success());

    // Enabling `github.proxy` with defaults writes https://gh-proxy.com/ to config.toml.
    let enable = cli(temp.path())
        .args(["run", "github.proxy", "--yes"])
        .output()
        .unwrap();
    assert!(
        enable.status.success(),
        "{}",
        String::from_utf8_lossy(&enable.stderr)
    );
    let config_path = temp.path().join("config/cqs/config.toml");
    let saved = fs::read_to_string(&config_path).unwrap();
    assert!(saved.contains("github_proxy_enabled = true"), "{saved}");
    assert!(
        saved.contains("github_proxy = \"https://gh-proxy.com/\""),
        "{saved}"
    );

    // Disabling keeps the normalized custom URL for future re-enabling.
    let disable = cli(temp.path())
        .args([
            "run",
            "github.proxy",
            "--yes",
            "--set",
            "gh_proxy_state=off",
            "--set",
            "gh_proxy_url=https://mirror.example/gh",
        ])
        .output()
        .unwrap();
    assert!(
        disable.status.success(),
        "{}",
        String::from_utf8_lossy(&disable.stderr)
    );
    let saved = fs::read_to_string(&config_path).unwrap();
    assert!(saved.contains("github_proxy_enabled = false"), "{saved}");
    assert!(
        saved.contains("github_proxy = \"https://mirror.example/gh/\""),
        "{saved}"
    );
}

#[test]
fn terminal_preview_runs_read_only_across_single_and_gallery_modes() {
    let temp = tempfile::tempdir().unwrap();
    for (locale, expected_title, args) in [
        (
            "zh-CN",
            "终端提示符预览",
            &[
                "run",
                "terminal.preview",
                "--set",
                "style=box",
                "--set",
                "color=tokyo-night",
            ][..],
        ),
        (
            "en-US",
            "Terminal prompt style gallery",
            &[
                "run",
                "terminal.preview",
                "--set",
                "style=all",
                "--set",
                "color=catppuccin",
            ][..],
        ),
        (
            "en-US",
            "Terminal prompt palette gallery",
            &[
                "run",
                "terminal.preview",
                "--set",
                "style=ribbon",
                "--set",
                "color=all",
            ][..],
        ),
    ] {
        let out = cli(temp.path())
            .args(["--lang", locale])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains(expected_title), "{stdout}");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    let bad = cli(temp.path())
        .args(["run", "terminal.preview", "--set", "style=nonexistent"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn docker_subcommand_help_is_bilingual_and_requires_tty_without_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    for (locale, expected) in [("zh-CN", "Docker TUI"), ("en-US", "Docker TUI")] {
        let out = cli(temp.path())
            .args(["--lang", locale, "docker", "--help"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains(expected), "{stdout}");
        assert!(stdout.contains("--tab"), "{stdout}");
        assert!(stdout.contains("--embedded"), "{stdout}");
    }
    let no_tty = cli(temp.path())
        .args(["--lang", "en-US", "docker", "--tab", "compose"])
        .output()
        .unwrap();
    assert_eq!(no_tty.status.code(), Some(1));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn update_subcommand_help_aliases_and_argument_validation_have_no_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    for (locale, expected) in [
        ("zh-CN", "ZyphrZero/CqShell"),
        ("en-US", "ZyphrZero/CqShell"),
    ] {
        for sub in ["update", "self-update"] {
            let out = cli(temp.path())
                .args(["--lang", locale, sub, "--help"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert!(stdout.contains(expected), "{stdout}");
            assert!(stdout.contains("--check"), "{stdout}");
            assert!(stdout.contains("--version"), "{stdout}");
            assert!(stdout.contains("--force"), "{stdout}");
        }
    }

    // --json requires --check on update; --check conflicts with --force; invalid tags fail offline
    for args in [
        &["--json", "update"][..],
        &["update", "--check", "--force"],
        &["update", "--check", "--version", "not-a-semver"],
        &["run", "app.update", "--set", "version=v1.2.3/../bad"],
    ] {
        let out = cli(temp.path()).args(args).output().unwrap();
        assert!(!out.status.success(), "{args:?} should fail");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    // Both update actions appear in `cqs list --json` with bilingual metadata
    for locale in ["zh-CN", "en-US"] {
        let out = cli(temp.path())
            .args(["--lang", locale, "list", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let rows = rows.as_array().unwrap();
        let check = rows
            .iter()
            .find(|r| r["id"] == "app.check_update")
            .expect("app.check_update");
        assert_eq!(check["group"], "app");
        assert_eq!(check["root"], false);
        assert_eq!(check["mutates"], false);
        let update = rows
            .iter()
            .find(|r| r["id"] == "app.update")
            .expect("app.update");
        assert_eq!(update["group"], "app");
        assert_eq!(update["root"], false);
        assert_eq!(update["mutates"], true);
    }
}
