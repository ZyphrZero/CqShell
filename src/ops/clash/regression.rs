use super::*;

#[test]
fn proxy_environment_never_substitutes_another_protocol_port() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    for config in [
        "socks-port: 17890\n",
        "port: 17890\n",
        "mixed-port: 0\nsocks-port: 17890\n",
        "mixed-port: 0\nport: 17890\n",
    ] {
        fs::write(&ctx.paths.clash_config, config).unwrap();
        for shell in ["bash", "zsh", "fish"] {
            assert!(proxy_env_script(&proxy_config(&ctx).unwrap(), shell).is_err());
        }
    }
    for (config, http_port, socks_port) in [
        ("mixed-port: 17890\n", 17890, 17890),
        ("port: 17890\nsocks-port: 17891\n", 17890, 17891),
    ] {
        fs::write(&ctx.paths.clash_config, config).unwrap();
        for shell in ["bash", "zsh", "fish"] {
            let script = proxy_env_script(&proxy_config(&ctx).unwrap(), shell).unwrap();
            assert!(script.contains(&format!("http://127.0.0.1:{http_port}")));
            assert!(script.contains(&format!("socks5h://127.0.0.1:{socks_port}")));
        }
    }
}

#[test]
fn selected_subscription_cannot_fall_back_to_direct_when_base_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    fs::write(&ctx.paths.clash_profiles, "use: selected\nprofiles: []\n").unwrap();

    assert!(build_runtime_config(&ctx).is_err());
    assert!(!ctx.paths.clash_runtime.exists());
    assert!(!ctx.paths.clash_config.exists());
}

#[test]
fn fresh_and_legacy_empty_profiles_still_allow_explicit_direct_startup() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    for metadata in [
        None,
        Some("use:\nprofiles:\n"),
        Some("use: ''\nprofiles: []\n"),
    ] {
        if let Some(metadata) = metadata {
            fs::write(&ctx.paths.clash_profiles, metadata).unwrap();
        }
        let runtime = yaml(&build_runtime_config(&ctx).unwrap()).unwrap();
        assert_eq!(runtime["mode"], "direct");
    }
}

#[test]
fn final_runtime_enforces_controller_authentication_after_mixin_merge() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    fs::write(
        &ctx.paths.clash_config,
        "mode: direct\nexternal-controller: 0.0.0.0:19090\n",
    )
    .unwrap();
    assert!(build_runtime_config(&ctx).is_err());

    fs::write(
        &ctx.paths.clash_mixin,
        "external-controller: 127.0.0.1:19090\n",
    )
    .unwrap();
    assert!(build_runtime_config(&ctx).is_ok());
    fs::write(
        &ctx.paths.clash_mixin,
        "external-controller: '[::]:19090'\n",
    )
    .unwrap();
    assert!(build_runtime_config(&ctx).is_err());
    fs::write(
        &ctx.paths.clash_mixin,
        "external-controller: '[::]:19090'\nsecret: fixture-only-key\n",
    )
    .unwrap();
    assert!(build_runtime_config(&ctx).is_ok());
}

#[test]
fn selection_rejects_invalid_mixin_before_committing_any_file() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    let source = b"proxies:\n  - name: sample\n    type: direct\nrules: ['MATCH,DIRECT']\n";
    fs::write(ctx.paths.clash_data.join("profiles/sample.yaml"), source).unwrap();
    let metadata = b"use: previous\nprofiles:\n  - name: sample\n    url: https://example.invalid/sub\n    path: sample.yaml\n";
    fs::write(&ctx.paths.clash_profiles, metadata).unwrap();
    fs::write(&ctx.paths.clash_config, b"mode: direct\n").unwrap();
    fs::write(&ctx.paths.clash_runtime, b"old runtime").unwrap();
    fs::write(&ctx.paths.clash_mixin, b"rules: [invalid-operation-map]\n").unwrap();

    assert!(subscription_use(&ctx, "sample").is_err());
    assert_eq!(fs::read(&ctx.paths.clash_profiles).unwrap(), metadata);
    assert_eq!(
        fs::read(&ctx.paths.clash_config).unwrap(),
        b"mode: direct\n"
    );
    assert_eq!(fs::read(&ctx.paths.clash_runtime).unwrap(), b"old runtime");
    assert!(!ctx.paths.clash_data.join("pending.json").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn uncertain_live_process_identity_blocks_stop_start_and_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
    fs::copy(std::env::current_exe().unwrap(), &ctx.paths.clash_binary).unwrap();
    let pid = std::process::id().to_string();
    fs::write(&ctx.paths.clash_pid, &pid).unwrap();

    for recorded in [
        None,
        Some("0"),
        process_start_time(std::process::id()).as_deref(),
    ] {
        if let Some(recorded) = recorded {
            fs::write(&ctx.paths.clash_pid_start, recorded).unwrap();
        }
        assert_eq!(state(&ctx).unwrap(), State::Unknown);
        assert!(stop(&ctx).is_err());
        assert!(start_inner(&ctx, false).is_err());
        assert!(controller::configure(&ctx, "127.0.0.1:19090").is_err());
        assert_eq!(fs::read_to_string(&ctx.paths.clash_pid).unwrap(), pid);
        assert!(!ctx.paths.clash_runtime.exists());
    }

    fs::remove_file(&ctx.paths.clash_pid).unwrap();
    assert_eq!(state(&ctx).unwrap(), State::Unknown);
    assert!(stop(&ctx).is_err());
    assert!(start_inner(&ctx, false).is_err());
    assert!(ctx.paths.clash_pid_start.exists());
    fs::write(&ctx.paths.clash_pid, &pid).unwrap();
    fs::remove_file(&ctx.paths.clash_binary).unwrap();
    assert_eq!(state(&ctx).unwrap(), State::Unknown);
    assert!(stop(&ctx).is_err());
    assert_eq!(fs::read_to_string(&ctx.paths.clash_pid).unwrap(), pid);
}

#[cfg(target_os = "linux")]
#[test]
fn confirmed_exited_process_can_clear_stale_records() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
    fs::copy(std::env::current_exe().unwrap(), &ctx.paths.clash_binary).unwrap();
    let mut child = Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id();
    assert!(child.wait().unwrap().success());
    fs::write(&ctx.paths.clash_pid, pid.to_string()).unwrap();

    assert_eq!(state(&ctx).unwrap(), State::Stopped);
    stop(&ctx).unwrap();
    assert!(!ctx.paths.clash_pid.exists());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a verified Mihomo binary in CQS_CLASH_SMOKE_BINARY"]
fn real_mihomo_rejects_merged_subscription_before_commit() {
    let binary = std::env::var_os("CQS_CLASH_SMOKE_BINARY").expect("CQS_CLASH_SMOKE_BINARY");
    let temp = tempfile::tempdir().unwrap();
    let ctx = crate::tests::context(temp.path());
    ensure_layout(&ctx).unwrap();
    fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
    fs::copy(binary, &ctx.paths.clash_binary).unwrap();
    let source = b"mixed-port: 0\nproxies:\n  - name: candidate\n    type: direct\nrules: ['MATCH,DIRECT']\n";
    let profile = ctx.paths.clash_data.join("profiles/sample.yaml");
    fs::write(&profile, source).unwrap();
    validate_with_mihomo(&ctx, &profile).unwrap();
    let metadata = b"use: previous\nprofiles:\n  - name: sample\n    url: https://example.invalid/sub\n    path: sample.yaml\n";
    fs::write(&ctx.paths.clash_profiles, metadata).unwrap();
    fs::write(&ctx.paths.clash_config, b"mode: direct\n").unwrap();
    fs::write(&ctx.paths.clash_runtime, b"mode: direct\n").unwrap();
    fs::write(
        &ctx.paths.clash_mixin,
        b"rules:\n  prepend:\n    - MATCH,missing-policy\n",
    )
    .unwrap();

    assert_eq!(
        subscription_use(&ctx, "sample").unwrap_err().key,
        "error.verify"
    );
    assert_eq!(fs::read(&ctx.paths.clash_profiles).unwrap(), metadata);
    assert_eq!(
        fs::read(&ctx.paths.clash_config).unwrap(),
        b"mode: direct\n"
    );
    assert_eq!(
        fs::read(&ctx.paths.clash_runtime).unwrap(),
        b"mode: direct\n"
    );
    assert!(!ctx.paths.clash_data.join("pending.json").exists());
}
