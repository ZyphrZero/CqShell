#![cfg(target_os = "linux")]

use cqs::{
    config::Paths,
    i18n::I18n,
    ops::clash::controller,
    runtime::Context,
    storage::{self, Saved},
};
use std::{
    cell::Cell,
    fs,
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn context(root: &Path) -> Context {
    let paths = Paths {
        home: root.into(),
        state: root.join("state"),
        preferences: root.join("preferences.toml"),
        terminal: root.join("starship.toml"),
        bashrc: root.join(".bashrc"),
        zshrc: root.join(".zshrc"),
        bbr: root.join("bbr.conf"),
        tcp: root.join("tcp.conf"),
        kernel_lock: root.join("kernel.lock"),
        kernel_backup: root.join("kernel-backup"),
        boot: root.join("boot"),
        install: root.join("cqs"),
        ssh: root.join("sshd_config"),
        docker: root.join("daemon.json"),
        clash_home: root.join("clash"),
        clash_data: root.join("clash/data"),
        clash_resources: root.join("clash/resources"),
        clash_binary: root.join("clash/bin/mihomo"),
        clash_config: root.join("clash/data/config.yaml"),
        clash_mixin: root.join("clash/data/mixin.yaml"),
        clash_runtime: root.join("clash/data/runtime.yaml"),
        clash_profiles: root.join("clash/data/profiles.yaml"),
        clash_pid: root.join("clash/data/mihomo.pid"),
        clash_pid_start: root.join("clash/data/mihomo.start"),
        clash_log: root.join("clash/data/mihomo.log"),
        clash_lock: root.join("clash/data/clash.lock"),
    };
    Context::new(paths, I18n::load(Some("en-US"), None).unwrap(), true)
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "subprocess fixture invoked only by the recovery test"]
fn transaction_child() {
    let root = std::env::var_os("CQS_CRASH_FIXTURE_ROOT").expect("fixture root");
    let root = Path::new(&root);
    let ctx = context(root);
    let calls = Cell::new(0);
    storage::write_transaction(
        &[
            (
                ctx.paths.clash_mixin.clone(),
                Some(Saved::new(b"custom-marker: interrupted\n", 0o600)),
            ),
            (
                ctx.paths.clash_runtime.clone(),
                Some(Saved::new(b"mode: global\n", 0o600)),
            ),
        ],
        &ctx.paths.clash_data.join("pending.json"),
        || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                // The first file and before-image journal are fsynced by the
                // actual storage layer before this deterministic crash point.
                storage::atomic_write(&root.join("checkpoint"), b"ready", 0o600)?;
                let deadline = Instant::now() + Duration::from_secs(30);
                while Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                panic!("parent did not kill the fixture at its checkpoint");
            }
            Ok(())
        },
    )
    .unwrap();
    panic!("interrupted transaction unexpectedly completed");
}

#[test]
fn sigkill_between_file_commits_recovers_before_the_next_settings_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ctx = context(root);
    storage::private_dir(&ctx.paths.clash_data).unwrap();
    let original_mixin = b"custom-marker: original\nexternal-controller: 127.0.0.1:19090\n";
    let original_runtime = b"mode: direct\ncustom-marker: original-runtime\n";
    let original_base = b"mode: direct\nmixed-port: 17890\n";
    let original_profiles = b"use: saved\nprofiles: []\ncustom-metadata: preserve\n";
    for (path, contents) in [
        (&ctx.paths.clash_mixin, original_mixin.as_slice()),
        (&ctx.paths.clash_runtime, original_runtime.as_slice()),
        (&ctx.paths.clash_config, original_base.as_slice()),
        (&ctx.paths.clash_profiles, original_profiles.as_slice()),
    ] {
        storage::atomic_write(path, contents, 0o600).unwrap();
    }
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "transaction_child", "--nocapture"])
            .env("CQS_CRASH_FIXTURE_ROOT", root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("checkpoint").is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before its first durable file commit"
        );
        assert!(Instant::now() < deadline, "fixture checkpoint timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let pending = ctx.paths.clash_data.join("pending.json");
    assert!(pending.is_file());
    assert_eq!(
        fs::read(&ctx.paths.clash_mixin).unwrap(),
        b"custom-marker: interrupted\n"
    );
    assert_eq!(
        fs::read(&ctx.paths.clash_runtime).unwrap(),
        original_runtime
    );
    // Child::kill sends SIGKILL on Unix, so no destructor or rollback runs.
    child.0.kill().unwrap();
    let killed = child.0.wait().unwrap();
    assert_eq!(killed.signal(), Some(libc::SIGKILL));
    assert!(pending.is_file());

    controller::configure(&ctx, "127.0.0.1:19091").unwrap();

    let mixin: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
    let runtime: serde_yaml::Value =
        serde_yaml::from_slice(&fs::read(&ctx.paths.clash_runtime).unwrap()).unwrap();
    assert_eq!(mixin["custom-marker"], "original");
    assert_eq!(mixin["external-controller"], "127.0.0.1:19091");
    assert_eq!(runtime["custom-marker"], "original");
    assert_eq!(runtime["external-controller"], "127.0.0.1:19091");
    assert_eq!(runtime["mode"], "direct");
    assert_eq!(fs::read(&ctx.paths.clash_config).unwrap(), original_base);
    assert_eq!(
        fs::read(&ctx.paths.clash_profiles).unwrap(),
        original_profiles
    );
    assert!(!pending.exists());
    for path in [
        &ctx.paths.clash_mixin,
        &ctx.paths.clash_runtime,
        &ctx.paths.clash_config,
        &ctx.paths.clash_profiles,
    ] {
        assert_eq!(storage::mode(path).unwrap(), 0o600);
    }
}
