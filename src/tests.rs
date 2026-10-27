use crate::{
    actions,
    config::Paths,
    error::Error,
    i18n::{self, Catalog, I18n},
    ops::{ai, docker, kernel, memory, network, ssh, system, terminal, tools},
    runtime::Context,
    storage::{self, Lock, Saved},
};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) fn context(root: &Path) -> Context {
    let paths = Paths {
        home: root.into(),
        state: root.join("state"),
        preferences: root.join("config.toml"),
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
    };
    Context::new(paths, I18n::load(Some("en-US"), None).unwrap(), true)
}

#[test]
fn kernel_install_variants_accept_parameters_and_reach_the_operation_lock() {
    for yes in [false, true] {
        for profile in ["standard", "max"] {
            for version in ["", "7.2.8", "7.3.0"] {
                let temp = tempfile::tempdir().unwrap();
                let mut ctx = context(temp.path());
                ctx.yes = yes;
                let parameters = actions::find("kernel.install")
                    .unwrap()
                    .parameters(&BTreeMap::from([
                        ("profile".into(), profile.into()),
                        ("version".into(), version.into()),
                    ]))
                    .unwrap();
                assert_eq!(parameters["profile"], profile);
                assert_eq!(parameters["version"], version);
                // Stop at the real lock boundary; never download or install a kernel in tests.
                let _lock = Lock::acquire(&ctx.paths.kernel_lock).unwrap();
                let error = kernel::install(&ctx, profile, version).unwrap_err();
                assert_eq!(error.key, "error.lock");
                assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
            }
        }
    }
}

#[test]
fn bbr_tcp_optimization_is_available_independently_of_kernel_installation() {
    for id in ["bbr.enable", "tcp.tune"] {
        actions::find(id)
            .unwrap()
            .parameters(&BTreeMap::new())
            .unwrap();
        assert!(actions::OPTIMIZATION.contains(&id));
    }
    assert!(!actions::OPTIMIZATION.contains(&"kernel.install"));
}

#[test]
fn terminal_escape_sequences_do_not_leak_into_task_logs() {
    let input = "Setting up openssh-server\n\x1b[?1049h\x1b[22;0;0t\x1b[1;24r\x1b[?25l\x1b[m\x1b[37m\x1b[40m\x1b[1;1H\x1b[97m\x1b[44m\x1b[K\n\x1b[K\n";
    assert_eq!(
        crate::runtime::sanitize(input),
        "Setting up openssh-server\n\n\n"
    );
    assert_eq!(
        crate::runtime::sanitize("\x1b]8;;https://example.org\x07链接\x1b]8;;\x07\t完成"),
        "链接\t完成"
    );
}

#[test]
fn ai_cancelled_installation_stops_before_writing() {
    for tool in [ai::Tool::Pi, ai::Tool::Claude, ai::Tool::Codex] {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ctx.cancel.store(true, Ordering::SeqCst);
        assert_eq!(ai::install(&ctx, tool).unwrap_err().key, "error.cancelled");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn individual_package_entries_keep_the_fixed_package_and_shared_lock() {
    for package in tools::PACKAGES {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let spec = actions::find(&format!("tools.{package}")).unwrap();
        assert_eq!(spec.kind, actions::Kind::ToolInstall(package));
        assert!(
            spec.parameters(&BTreeMap::from([("package".into(), "all".into())]))
                .is_err()
        );
        let _lock = Lock::acquire(&ctx.paths.kernel_lock).unwrap();
        assert_eq!(
            tools::packages(&ctx, package).unwrap_err().key,
            "error.lock"
        );
    }
}

#[test]
fn catalogs_cover_all_action_and_field_labels() {
    for locale in ["zh-CN", "en-US"] {
        let i = I18n::load(Some(locale), None).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for spec in actions::specs() {
            assert!(ids.insert(spec.id));
            assert!(i.has(&spec.label()), "{}", spec.label());
            assert!(i.has(&spec.description()));
            assert!(i.has(&format!("group.{}", spec.group)));
            for field in spec.fields {
                assert!(i.has(&field.label()));
            }
        }
    }
}
#[test]
fn every_literal_translation_key_exists() {
    let i = I18n::load(Some("en-US"), None).unwrap();
    let pattern =
        regex::Regex::new(r#""((?:error|notice|confirm|ui|cli|info|status|dash)\.[a-z_]+)""#)
            .unwrap();
    for source in [
        include_str!("actions.rs"),
        include_str!("cli.rs"),
        include_str!("config.rs"),
        include_str!("runtime.rs"),
        include_str!("storage.rs"),
        include_str!("tui.rs"),
        include_str!("tui/view.rs"),
        include_str!("dashboard.rs"),
        include_str!("ops/ai.rs"),
        include_str!("ops/docker.rs"),
        include_str!("ops/ip_quality.rs"),
        include_str!("ops/kernel.rs"),
        include_str!("ops/memory.rs"),
        include_str!("ops/network.rs"),
        include_str!("ops/packages.rs"),
        include_str!("ops/service.rs"),
        include_str!("ops/ssh.rs"),
        include_str!("ops/system.rs"),
        include_str!("ops/terminal.rs"),
        include_str!("ops/tools.rs"),
    ] {
        for capture in pattern.captures_iter(source) {
            assert!(i.has(&capture[1]), "{}", &capture[1]);
        }
    }
}
#[test]
fn language_missing_keys_and_bad_placeholders_are_rejected() {
    let reference: Catalog = toml::from_str(include_str!("../locales/en-US.toml")).unwrap();
    let mut candidate = reference.clone();
    candidate.messages.remove("ui.ready");
    assert!(i18n::validate(&reference, &candidate).is_err());
    let mut candidate = reference.clone();
    candidate
        .messages
        .insert("notice.saved".into(), "saved {path}".into());
    assert!(i18n::validate(&reference, &candidate).is_err());
    let mut candidate = reference.clone();
    candidate.messages.insert("ui.ready".into(), "".into());
    assert!(i18n::validate(&reference, &candidate).is_err());
    assert!(i18n::placeholders("{{bad}").is_err());
    assert!(i18n::placeholders("bad}").is_err());
}
#[test]
fn translated_values_are_not_reinterpreted_as_templates() {
    let i = I18n::load(Some("en-US"), None).unwrap();
    assert_eq!(
        i.format("notice.saved", &[("detail", "{detail} {$x}")]),
        "Configuration saved: {detail} {$x}"
    );
}
#[test]
fn external_catalogs_are_strict_and_explicit_locales_do_not_fall_back() {
    let temp = tempfile::tempdir().unwrap();
    let text = include_str!("../locales/en-US.toml")
        .replace("locale = \"en-US\"", "locale = \"fr-FR\"")
        .replace("name = \"English\"", "name = \"Français\"");
    fs::write(temp.path().join("fr-FR.toml"), text).unwrap();
    assert_eq!(
        I18n::load(Some("fr-FR"), Some(temp.path()))
            .unwrap()
            .locale(),
        "fr-FR"
    );
    assert!(I18n::load(Some("missing"), Some(temp.path())).is_err());
    fs::write(temp.path().join("fr-FR.toml"), "not = valid translation").unwrap();
    assert!(I18n::load(Some("en-US"), Some(temp.path())).is_err());
    assert_eq!(i18n::normalize("zh_CN.UTF-8"), "zh-CN");
}
#[test]
fn action_parameters_reject_unknown_duplicates_and_empty_required() {
    assert!(
        actions::find("ssh.key")
            .unwrap()
            .parameters(&BTreeMap::new())
            .is_err()
    );
    assert!(
        actions::find("bbr.enable")
            .unwrap()
            .parameters(&BTreeMap::from([("qdisc".into(), "fq; id".into())]))
            .is_err()
    );
    assert!(
        actions::find("bbr.enable")
            .unwrap()
            .parameters(&BTreeMap::from([("bogus".into(), "x".into())]))
            .is_err()
    );
    assert!(crate::cli::parse_parameters(["x=1", "x=2"].into_iter().map(str::to_owned)).is_err());
    assert!(actions::find("; id").is_err());
}
#[test]
fn cli_run_accepts_one_id_and_batch_accepts_many() {
    let i = I18n::load(Some("en-US"), None).unwrap();
    crate::cli::command(&i).debug_assert();
    assert!(
        crate::cli::command(&i)
            .try_get_matches_from(["cqs", "run", "bbr.status"])
            .is_ok()
    );
    assert!(
        crate::cli::command(&i)
            .try_get_matches_from(["cqs", "run", "bbr.status", "kernel.list"])
            .is_err()
    );
    assert!(
        crate::cli::command(&i)
            .try_get_matches_from(["cqs", "batch", "bbr.status", "kernel.list"])
            .is_ok()
    );
}

#[test]
fn batch_parameter_preflight_rejects_invalid_ranges_before_execution() {
    for (id, key, value) in [
        ("ssh.port", "port", "65536"),
        ("swap.create", "size_mb", "0"),
        ("kernel.install", "version", "latest"),
        ("kernel.remove", "release", "6.1.0"),
        ("tcp.tune", "rtt", "0"),
        ("system.timezone", "timezone", "../x"),
        ("ssh.key", "public_key", "relative.pub"),
    ] {
        assert!(
            actions::find(id)
                .unwrap()
                .parameters(&BTreeMap::from([(key.into(), value.into())]))
                .is_err(),
            "{id}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path());
    let jobs = vec![
        (actions::find("app.install").unwrap(), BTreeMap::new()),
        (
            actions::find("ssh.port").unwrap(),
            BTreeMap::from([("port".into(), "0".into())]),
        ),
    ];
    assert_eq!(actions::batch(&ctx, &jobs).unwrap_err().key, "error.input");
    assert!(!ctx.paths.install.exists());
    let error = docker::candidate(
        "{}",
        docker::Change::Mirror("https://user:private-value@example.com"),
        "",
    )
    .unwrap_err();
    assert!(!error.to_string().contains("private-value"));
}
#[test]
fn tcp_buffer_matches_original_contract() {
    for (bw, rtt, mem, mib) in [
        ("100", "10", 1048576, 1),
        ("1000", "80", 1048576, 20),
        ("1000", "200", 1048576, 32),
        ("1000", "80", 262144, 8),
        ("100000", "10000", 8388608, 64),
    ] {
        assert_eq!(memory::tcp_buffer(bw, rtt, mem).unwrap(), mib * 1048576);
    }
    for bw in [
        "0", "-1", "NaN", "1e3", "100001", "1.0001", ".5", "1.", "1;id",
    ] {
        assert!(memory::tcp_buffer(bw, "50", 1048576).is_err(), "{bw}");
    }
    assert!(memory::tcp_buffer("1", "0", 1048576).is_err());
    assert!(memory::tcp_buffer("1", "1", 65535).is_err());
}

fn proc_tree(
    root: &Path,
    kind: &str,
    membership: &str,
    mount_root: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let proc = root.join("proc");
    let mount = root.join("cgroup space");
    fs::create_dir_all(proc.join("self")).unwrap();
    fs::create_dir_all(&mount).unwrap();
    fs::write(proc.join("meminfo"), "MemTotal: 8388608 kB\n").unwrap();
    fs::write(
        proc.join("self/cgroup"),
        if kind == "cgroup2" {
            format!("0::{membership}\n")
        } else {
            format!("5:memory:{membership}\n")
        },
    )
    .unwrap();
    let escaped = mount.display().to_string().replace(' ', "\\040");
    fs::write(
        proc.join("self/mountinfo"),
        format!(
            "29 23 0:26 {mount_root} {escaped} rw - {kind} cgroup {}\n",
            if kind == "cgroup2" { "rw" } else { "rw,memory" }
        ),
    )
    .unwrap();
    (proc, mount)
}
#[test]
fn cgroup_v2_ancestor_caps_unlimited_leaf() {
    let temp = tempfile::tempdir().unwrap();
    let (proc, mount) = proc_tree(temp.path(), "cgroup2", "/tenant/app", "/");
    fs::create_dir_all(mount.join("tenant/app")).unwrap();
    fs::write(mount.join("memory.max"), "max").unwrap();
    fs::write(mount.join("tenant/memory.max"), "268435456").unwrap();
    fs::write(mount.join("tenant/app/memory.max"), "max").unwrap();
    assert_eq!(memory::effective_kb(&proc).unwrap(), 262144);
}
#[test]
fn cgroup_v1_respects_mount_root_and_large_limit() {
    let temp = tempfile::tempdir().unwrap();
    let (proc, mount) = proc_tree(temp.path(), "cgroup", "/tenant/app", "/tenant");
    fs::create_dir(mount.join("app")).unwrap();
    fs::write(mount.join("memory.limit_in_bytes"), "134217728").unwrap();
    fs::write(
        mount.join("app/memory.limit_in_bytes"),
        "9223372036854771712",
    )
    .unwrap();
    assert_eq!(memory::effective_kb(&proc).unwrap(), 131072);
}
#[test]
fn cgroup_unknown_paths_and_bad_limits_fail() {
    for group in ["/missing", "/../outside"] {
        let temp = tempfile::tempdir().unwrap();
        let (proc, _) = proc_tree(temp.path(), "cgroup2", group, "/");
        assert!(memory::effective_kb(&proc).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let (proc, mount) = proc_tree(temp.path(), "cgroup2", "/", "/");
    fs::write(mount.join("memory.max"), "broken").unwrap();
    assert!(memory::effective_kb(&proc).is_err());
}
#[test]
fn sysctl_parser_preserves_multivalue_data_and_rejects_duplicates() {
    assert_eq!(
        network::parse("net.ipv4.tcp_rmem = 4096 131072 1048576").unwrap()["net.ipv4.tcp_rmem"],
        "4096 131072 1048576"
    );
    for input in [
        "",
        "vm.swappiness = 1",
        "net.core.default_qdisc = fq\nnet.core.default_qdisc = cake",
        "net.core.default_qdisc =",
    ] {
        assert!(network::parse(input).is_err());
    }
}
#[test]
fn docker_merges_fields_and_removes_only_ipv6_keys() {
    let original = r#"{"data-root":"/srv/docker","log-driver":"journald","registry-mirrors":["https://old.example"],"fixed-cidr-v6":"fd42::/64"}"#;
    let on = docker::candidate(original, docker::Change::Ipv6(true), "fd11::/64").unwrap();
    let value: serde_json::Value = serde_json::from_str(&on).unwrap();
    assert_eq!(value["data-root"], "/srv/docker");
    assert_eq!(value["fixed-cidr-v6"], "fd42::/64");
    let off = docker::candidate(&on, docker::Change::Ipv6(false), "").unwrap();
    let value: serde_json::Value = serde_json::from_str(&off).unwrap();
    assert_eq!(value["ipv6"], false);
    assert!(value.get("fixed-cidr-v6").is_none());
    assert_eq!(value["log-driver"], "journald");
    let mirror =
        docker::candidate(original, docker::Change::Mirror("https://new.example"), "").unwrap();
    let value: serde_json::Value = serde_json::from_str(&mirror).unwrap();
    assert_eq!(value["data-root"], "/srv/docker");
    assert_eq!(value["registry-mirrors"][0], "https://new.example");
    assert!(docker::candidate("[]", docker::Change::Ipv6(true), "fd11::/64").is_err());
    assert!(
        docker::candidate(
            "{}",
            docker::Change::Mirror("https://user:password@example.com"),
            ""
        )
        .is_err()
    );
}
#[test]
fn ssh_preserves_matches_and_rejects_effective_conflicts() {
    let original = "Include /etc/ssh/sshd_config.d/*.conf\nPort 22\nMatch User backup\n PasswordAuthentication yes\n";
    let settings = [("Port", "2222".into())];
    let desired = ssh::candidate(original, &settings);
    assert!(desired.starts_with("Port 2222\n"));
    assert!(desired.contains("Include /etc/ssh"));
    assert!(desired.ends_with("Match User backup\n PasswordAuthentication yes\n"));
    assert!(!desired.contains("Port 22\n"));
    assert!(ssh::validate_effective("port 2222\nlistenaddress 0.0.0.0:2222\n", &settings).is_ok());
    assert!(ssh::validate_effective("port 2222\nport 22\n", &settings).is_err());
    assert!(ssh::validate_effective("port 2222\nlistenaddress [::]:22\n", &settings).is_err());
}
fn release(arch: &str, version: &str, max: bool) -> serde_json::Value {
    let prefix = if arch == "amd64" { "x86_64" } else { "arm64" };
    let suffix = if max { "-max" } else { "" };
    let tag = format!("{prefix}-{version}{suffix}");
    let kernel = format!("{version}-joeyblog-bbrv3{suffix}");
    let assets = ["image", "headers"].map(|kind| {
        let name = format!("linux-{kind}-{kernel}_1_{arch}.deb");
        serde_json::json!({"name":name,"browser_download_url":format!("https://github.com/byJoey/Actions-bbr-v3/releases/download/{tag}/{name}"),"digest":format!("sha256:{}", "a".repeat(64))})
    });
    serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":assets})
}
#[test]
fn kernel_release_selection_is_numeric_and_architecture_specific() {
    let data = serde_json::json!([
        release("amd64", "6.9.0", false),
        release("amd64", "6.10.0", false),
        release("amd64", "6.11.0", true),
        release("arm64", "6.12.0", false)
    ]);
    assert_eq!(
        kernel::manifest(&data.to_string(), "amd64", "standard", None)
            .unwrap()
            .kernel,
        "6.10.0-joeyblog-bbrv3"
    );
    assert_eq!(
        kernel::manifest(&data.to_string(), "amd64", "max", None)
            .unwrap()
            .kernel,
        "6.11.0-joeyblog-bbrv3-max"
    );
    assert!(
        kernel::manifest(&data.to_string(), "amd64", "standard", Some("x86_64-6.1.0")).is_err()
    );
}
#[test]
fn kernel_release_rejects_untrusted_urls_digest_duplicates_and_prereleases() {
    for variation in 0..4 {
        let mut value = release("amd64", "6.10.0", false);
        match variation {
            0 => value["assets"][0]["digest"] = "missing".into(),
            1 => {
                value["assets"][0]["browser_download_url"] =
                    "https://other.example/kernel.deb".into()
            }
            2 => {
                let a = value["assets"][0].clone();
                value["assets"].as_array_mut().unwrap().push(a);
            }
            _ => value["prerelease"] = true.into(),
        }
        assert!(kernel::manifest(&value.to_string(), "amd64", "standard", None).is_err());
    }
}
#[test]
fn kernel_distribution_and_removal_plan_guards() {
    assert!(kernel::supported_system("debian", "12"));
    assert!(kernel::supported_system("ubuntu", "24.04"));
    assert!(!kernel::supported_system("ubuntu", "24.03"));
    assert!(!kernel::supported_system("debian", "bookworm"));
    assert!(
        kernel::validate_removal_plan(
            "Remv linux-image-6.1.0-joeyblog-bbrv3 [1]",
            "6.1.0-joeyblog-bbrv3"
        )
        .is_ok()
    );
    assert!(
        kernel::validate_removal_plan("Remv linux-image-amd64 [1]", "6.1.0-joeyblog-bbrv3")
            .is_err()
    );
}

#[test]
fn kernel_boot_action_accepts_generic_releases_and_rejects_grub_syntax() {
    let spec = actions::find("kernel.boot").unwrap();
    assert_eq!(spec.kind, actions::Kind::KernelBoot);
    assert!(
        spec.parameters(&BTreeMap::from([(
            "release".into(),
            "6.12.86+deb13-amd64".into()
        )]))
        .is_ok()
    );
    for release in ["../vmlinuz", "6.12>recovery", "6.12 86"] {
        assert!(
            spec.parameters(&BTreeMap::from([("release".into(), release.into())]))
                .is_err()
        );
    }
}

#[test]
fn grub_boot_entry_resolves_unique_normal_nested_and_flat_entries() {
    let release = "6.12.86+deb13-amd64";
    let id = format!("gnulinux-{release}-advanced-384b8a87-248e-434b-8932-add577388de1");
    let submenu = "gnulinux-advanced-384b8a87-248e-434b-8932-add577388de1";
    let target_title = format!("Debian, with Linux {release}");
    let config = format!(
        "submenu 'Advanced options' $menuentry_id_option '{submenu}' {{\n  menuentry '{target_title}' $menuentry_id_option '{id}' {{\n  }}\n  menuentry 'Debian, with Linux {release} (recovery mode)' $menuentry_id_option '{id}-recovery' {{\n  }}\n}}\n"
    );
    assert_eq!(
        kernel::grub_boot_entry(&config, release).unwrap(),
        format!("Advanced options>{target_title}")
    );
    let flat_title = format!("Linux, with Linux {release}");
    let flat = format!("menuentry '{flat_title}' $menuentry_id_option '{id}' {{\n}}\n");
    assert_eq!(kernel::grub_boot_entry(&flat, release).unwrap(), flat_title);
    assert!(kernel::grub_boot_entry(&config, "../etc").is_err());
    assert!(kernel::grub_boot_entry("", release).is_err());
}

#[test]
fn terminal_rc_preserves_bytes_and_requires_balanced_markers() {
    let original = b"# user\r\necho \xff\n";
    let new = terminal::edit_rc(original, "bash", true, "/home/user/a'b.toml").unwrap();
    assert!(new.starts_with(original));
    assert!(String::from_utf8_lossy(&new).contains("'\"'\"'"));
    assert_eq!(
        terminal::edit_rc(&new, "bash", true, "/home/user/a'b.toml").unwrap(),
        new
    );
    assert_eq!(
        terminal::edit_rc(&new, "bash", false, "/unused").unwrap(),
        original
    );
    assert!(terminal::edit_rc(b"# >>> cqs terminal >>>\n", "bash", false, "/unused").is_err());
}
#[test]
fn all_prompt_variants_are_valid_toml() {
    for style in ["full", "compact"] {
        for color in ["cyan", "green", "magenta"] {
            for time in ["on", "off"] {
                for git in ["on", "off"] {
                    let settings = terminal::Options {
                        shell: "both".into(),
                        style: style.into(),
                        color: color.into(),
                        time: time.into(),
                        git: git.into(),
                    };
                    let value: toml::Value =
                        toml::from_str(&terminal::render(&settings).unwrap()).unwrap();
                    assert_eq!(
                        value["git_branch"]["disabled"].as_bool().unwrap(),
                        git == "off"
                    );
                }
            }
        }
    }
}
#[test]
fn file_transaction_rolls_back_partial_writes_and_retains_no_false_success() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let pending = temp.path().join("pending.json");
    fs::write(&a, b"original").unwrap();
    let count = AtomicUsize::new(0);
    let result = storage::write_transaction(
        &[
            (a.clone(), Some(Saved::new(b"changed", 0o600))),
            (b.clone(), Some(Saved::new(b"new", 0o600))),
        ],
        &pending,
        || {
            if count.fetch_add(1, Ordering::SeqCst) == 1 {
                Err(Error::new("error.cancelled", "injected interruption"))
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(a).unwrap(), b"original");
    assert!(!b.exists());
    assert!(!pending.exists());
}
#[test]
fn pending_journal_blocks_further_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("a");
    let pending = temp.path().join("pending.json");
    fs::write(&pending, "incomplete").unwrap();
    assert!(
        storage::write_transaction(
            &[(target.clone(), Some(Saved::new(b"new", 0o600)))],
            &pending,
            || Ok(())
        )
        .is_err()
    );
    assert!(!target.exists());
    assert_eq!(fs::read_to_string(pending).unwrap(), "incomplete");
}
#[test]
fn locks_are_exclusive_and_reusable_without_removing_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lock");
    let lock = Lock::acquire(&path).unwrap();
    assert!(Lock::acquire(&path).is_err());
    drop(lock);
    assert!(Lock::acquire(&path).is_ok());
    assert!(path.exists());
}
#[cfg(unix)]
#[test]
fn symlinks_in_targets_or_parents_are_rejected() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    fs::write(&original, b"original").unwrap();
    let link = temp.path().join("link");
    symlink(&original, &link).unwrap();
    assert!(storage::atomic_write(&link, b"changed", 0o600).is_err());
    assert_eq!(fs::read(original).unwrap(), b"original");
    let directory = temp.path().join("dir");
    symlink(temp.path(), &directory).unwrap();
    assert!(storage::atomic_write(&directory.join("new"), b"data", 0o600).is_err());
}
#[test]
fn swap_editing_only_removes_the_exact_target() {
    assert_eq!(
        system::fstab_without_swap(
            "# /swapfile comment\n/swapfile2 none swap defaults 0 0\n/swapfile none swap defaults 0 0\n"
        ),
        "# /swapfile comment\n/swapfile2 none swap defaults 0 0\n"
    );
    assert_eq!(system::swap_size(512), 1024);
    assert_eq!(system::swap_size(4096), 4096);
    assert_eq!(system::swap_size(8192), 8192);
    assert_eq!(system::swap_size(131072), 16384);
}
#[test]
fn ui_renders_both_languages_and_small_terminals() {
    use ratatui::{Terminal, backend::TestBackend};
    for locale in ["en-US", "zh-CN"] {
        for (width, height) in [(120, 40), (80, 24), (40, 10)] {
            let temp = tempfile::tempdir().unwrap();
            let mut ctx = context(temp.path());
            ctx.i18n.select(locale).unwrap();
            let app = crate::tui::App::new(ctx);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            // TestBackend keeps a blank continuation cell after each double-width glyph.
            let compact = text.replace(' ', "");
            assert!(
                compact.contains(if width < 60 {
                    if locale == "zh-CN" {
                        "终端过小"
                    } else {
                        "Terminaltoosmall"
                    }
                } else {
                    "CqShell"
                }),
                "{locale} {width}x{height}: {text:?}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn lock_owner_release_does_not_leave_forked_child_holding_it() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("lock");
        let lock = Lock::acquire(&file).unwrap();
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // Only async-signal-safe syscalls are permitted in the forked test child.
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0u8;
                libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe {
            libc::close(pipe[0]);
        }
        drop(lock);
        let acquired = Lock::acquire(&file);
        unsafe {
            let byte = 1u8;
            libc::write(pipe[1], (&byte as *const u8).cast(), 1);
            libc::close(pipe[1]);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        assert!(acquired.is_ok(), "{:#?}", acquired.err());
    }
    fn executable(path: &Path, source: &str) {
        fs::write(path, source).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn boundary(root: &Path) -> Context {
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        // Explicit process-boundary doubles exercise real filesystem commits and errors.
        // They do not establish that a real kernel or daemon accepts the configuration.
        executable(
            &bin.join("sysctl"),
            r#"#!/bin/bash
set -eu
root="$CQS_TEST_ROOT"
printf '%s\n' "$*" >> "$root/calls"
case "$1" in
-n) cat "$root/${2//./_}" ;;
-p)
 while read -r key separator value; do
   printf '%s\n' "$value" > "$root/${key//./_}"
   if [[ -e "$root/fail_apply" ]]; then exit 23; fi
 done < "$2" ;;
-w)
 if [[ -e "$root/fail_restore" ]]; then exit 24; fi
 key="${2%%=*}"; printf '%s\n' "${2#*=}" > "$root/${key//./_}" ;;
*) exit 99 ;;
esac
"#,
        );
        executable(
            &bin.join("systemctl"),
            r#"#!/bin/bash
set -eu
root="$CQS_TEST_ROOT"
printf '%s\n' "$*" >> "$root/services"
case "$1" in
is-active) if [[ -e "$root/inactive" ]]; then exit 3; else exit 0; fi ;;
stop) touch "$root/inactive" ;;
restart)
 if [[ -e "$root/fail_always" ]]; then exit 21; fi
 if [[ -e "$root/fail_once" ]]; then rm "$root/fail_once"; exit 22; fi
 rm -f "$root/inactive" ;;
*) exit 99 ;;
esac
"#,
        );
        let mut ctx = context(root);
        ctx.environment
            .insert("PATH".into(), format!("{}:/usr/bin:/bin", bin.display()));
        ctx.environment
            .insert("CQS_TEST_ROOT".into(), root.display().to_string());
        ctx
    }
    fn network_values(root: &Path) {
        fs::write(root.join("net_core_default_qdisc"), "fq_codel\n").unwrap();
        fs::write(root.join("net_ipv4_tcp_congestion_control"), "cubic\n").unwrap();
    }

    #[test]
    fn package_updates_disable_interactive_frontends_and_dpkg_pty() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = boundary(temp.path());
        ctx.environment
            .insert("DEBIAN_FRONTEND".into(), "dialog".into());
        ctx.environment
            .insert("UCF_FORCE_CONFFNEW".into(), "1".into());
        executable(
            &temp.path().join("bin/apt-get"),
            r#"#!/usr/bin/python3
import json, os, sys
with open(os.path.join(os.environ['CQS_TEST_ROOT'], 'package-policy'), 'a') as output:
    output.write(json.dumps({'args': sys.argv[1:], 'env': {key: os.environ.get(key) for key in ['DEBIAN_FRONTEND', 'UCF_FORCE_CONFFOLD', 'UCF_FORCE_CONFFNEW', 'NEEDRESTART_MODE', 'APT_LISTCHANGES_FRONTEND']}}) + '\n')
"#,
        );
        system::update(&ctx).unwrap();
        let calls = fs::read_to_string(temp.path().join("package-policy")).unwrap();
        let calls: Vec<serde_json::Value> = calls
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["args"][0], "update");
        assert_eq!(calls[1]["args"][0], "upgrade");
        for call in calls {
            assert_eq!(call["env"]["DEBIAN_FRONTEND"], "noninteractive");
            assert_eq!(call["env"]["UCF_FORCE_CONFFOLD"], "1");
            assert!(
                call["env"]["UCF_FORCE_CONFFNEW"]
                    .as_str()
                    .is_none_or(str::is_empty)
            );
            assert_eq!(call["env"]["NEEDRESTART_MODE"], "l");
            assert_eq!(call["env"]["APT_LISTCHANGES_FRONTEND"], "none");
            for option in [
                "Dpkg::Use-Pty=0",
                "Dpkg::Options::=--force-confdef",
                "Dpkg::Options::=--force-confold",
            ] {
                assert!(
                    call["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| value == option),
                    "missing {option}"
                );
            }
        }
        assert_eq!(ctx.environment["DEBIAN_FRONTEND"], "dialog");
    }

    #[test]
    fn common_packages_use_the_expected_apt_arguments_and_propagate_failure() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        executable(
            &temp.path().join("bin/apt-get"),
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CQS_TEST_ROOT/packages"
test ! -e "$CQS_TEST_ROOT/fail_packages"
"#,
        );
        for package in tools::PACKAGES {
            tools::packages(&ctx, package).unwrap();
        }
        let calls = fs::read_to_string(temp.path().join("packages")).unwrap();
        assert_eq!(calls.lines().count(), tools::PACKAGES.len());
        for (call, package) in calls.lines().zip(tools::PACKAGES) {
            let (packages, options) = call.split_once(" -o ").unwrap();
            assert_eq!(packages, format!("install -y {package}"));
            assert!(options.contains("Dpkg::Use-Pty=0"));
        }
        fs::write(temp.path().join("fail_packages"), "").unwrap();
        assert_eq!(
            tools::packages(&ctx, "curl").unwrap_err().key,
            "error.command"
        );
    }

    fn desired() -> network::Parameters {
        network::parse("net.core.default_qdisc=fq\nnet.ipv4.tcp_congestion_control=bbr\n").unwrap()
    }
    #[test]
    fn enable_bbr_applies_the_selected_queue_and_persists_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        fs::write(
            temp.path()
                .join("net_ipv4_tcp_available_congestion_control"),
            "reno cubic bbr\n",
        )
        .unwrap();
        for qdisc in ["fq", "fq_codel", "fq_pie", "cake"] {
            network::enable_bbr(&ctx, qdisc).unwrap();
            assert_eq!(
                fs::read_to_string(temp.path().join("net_ipv4_tcp_congestion_control"))
                    .unwrap()
                    .trim(),
                "bbr"
            );
            assert_eq!(
                fs::read_to_string(temp.path().join("net_core_default_qdisc"))
                    .unwrap()
                    .trim(),
                qdisc
            );
            let config = network::parse(&fs::read_to_string(&ctx.paths.bbr).unwrap()).unwrap();
            assert_eq!(config["net.ipv4.tcp_congestion_control"], "bbr");
            assert_eq!(config["net.core.default_qdisc"], qdisc);
        }
        network::restore(&ctx, &ctx.paths.bbr).unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("net_ipv4_tcp_congestion_control"))
                .unwrap()
                .trim(),
            "cubic"
        );
    }
    #[test]
    fn network_roundtrip_keeps_first_baseline_and_original_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        fs::write(&ctx.paths.bbr, "# original\n").unwrap();
        network::apply(&ctx, &ctx.paths.bbr, &desired()).unwrap();
        let mut other = desired();
        other.insert("net.core.default_qdisc".into(), "cake".into());
        network::apply(&ctx, &ctx.paths.bbr, &other).unwrap();
        network::restore(&ctx, &ctx.paths.bbr).unwrap();
        assert_eq!(fs::read_to_string(&ctx.paths.bbr).unwrap(), "# original\n");
        assert_eq!(
            fs::read_to_string(temp.path().join("net_core_default_qdisc")).unwrap(),
            "fq_codel\n"
        );
        assert!(!storage::suffix(&ctx.paths.bbr, ".cqs-backup").exists());
    }
    #[test]
    fn network_partial_failure_restores_runtime_without_persisting() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        fs::write(temp.path().join("fail_apply"), "").unwrap();
        assert!(network::apply(&ctx, &ctx.paths.bbr, &desired()).is_err());
        assert!(!ctx.paths.bbr.exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("net_core_default_qdisc")).unwrap(),
            "fq_codel\n"
        );
        assert!(!storage::suffix(&ctx.paths.bbr, ".cqs-pending.json").exists());
    }
    #[test]
    fn network_failed_recovery_retains_durable_material() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        fs::write(temp.path().join("fail_apply"), "").unwrap();
        fs::write(temp.path().join("fail_restore"), "").unwrap();
        let error = network::apply(&ctx, &ctx.paths.bbr, &desired()).unwrap_err();
        assert_eq!(error.key, "error.recovery");
        assert!(storage::suffix(&ctx.paths.bbr, ".cqs-pending.json").exists());
        assert!(storage::suffix(&ctx.paths.bbr, ".cqs-backup").exists());
        assert_eq!(
            network::apply(&ctx, &ctx.paths.bbr, &desired())
                .unwrap_err()
                .key,
            "error.pending"
        );
    }
    #[test]
    fn network_rejects_lock_contention_before_runtime_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        let _lock = Lock::acquire(&storage::suffix(&ctx.paths.bbr, ".cqs-lock")).unwrap();
        assert_eq!(
            network::apply(&ctx, &ctx.paths.bbr, &desired())
                .unwrap_err()
                .key,
            "error.lock"
        );
        assert!(!temp.path().join("calls").exists());
    }
    #[test]
    fn service_restart_failure_restores_bytes_and_prior_inactive_state() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        fs::write(&ctx.paths.docker, "{ \"debug\": true }\n").unwrap();
        fs::write(temp.path().join("inactive"), "").unwrap();
        fs::write(temp.path().join("fail_once"), "").unwrap();
        let result = crate::ops::service::update(
            &ctx,
            &ctx.paths.docker,
            b"{}",
            "docker",
            false,
            |_| Ok(()),
            || ctx.run("systemctl", &["restart", "docker"]),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&ctx.paths.docker).unwrap(),
            "{ \"debug\": true }\n"
        );
        assert!(temp.path().join("inactive").exists());
        assert!(!storage::suffix(&ctx.paths.docker, ".cqs-pending.json").exists());
    }
    #[test]
    fn service_validation_failure_leaves_target_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        fs::write(&ctx.paths.docker, "original").unwrap();
        let result = crate::ops::service::update(
            &ctx,
            &ctx.paths.docker,
            b"candidate",
            "docker",
            true,
            |_| Err(Error::new("error.verify", "injected validator failure")),
            || panic!("must not activate"),
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&ctx.paths.docker).unwrap(), "original");
        assert!(!temp.path().join("services").exists());
    }
    #[test]
    fn service_failed_rollback_keeps_pending_record() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        fs::write(&ctx.paths.docker, "original").unwrap();
        fs::write(temp.path().join("fail_always"), "").unwrap();
        let error = crate::ops::service::update(
            &ctx,
            &ctx.paths.docker,
            b"candidate",
            "docker",
            true,
            |_| Ok(()),
            || ctx.run("systemctl", &["restart", "docker"]),
        )
        .unwrap_err();
        assert_eq!(error.key, "error.recovery");
        assert!(storage::suffix(&ctx.paths.docker, ".cqs-pending.json").exists());
    }
    #[test]
    fn runtime_preserves_nonzero_and_captures_both_streams() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let out = ctx
            .command(
                "bash",
                &["-c", "printf 'out'; printf 'err' >&2; exit 23"],
                false,
            )
            .unwrap();
        assert_eq!(out.status.code(), Some(23));
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert!(ctx.run("bash", &["-c", "exit 23"]).is_err());
    }
    #[test]
    fn cancellation_reaps_child_and_recovery_ignores_cancel_flag() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let flag = ctx.cancel.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let result = ctx.run("sleep", &["10"]);
        cancel.join().unwrap();
        assert_eq!(result.unwrap_err().key, "error.cancelled");
        ctx.recovery().run("true", &[]).unwrap();
    }
    #[test]
    fn legacy_terminal_json_baseline_restores_without_python() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let state = ctx.paths.state.join("terminal");
        fs::create_dir_all(&state).unwrap();
        fs::write(&ctx.paths.bashrc, "modified").unwrap();
        fs::write(&ctx.paths.terminal, "modified").unwrap();
        let baseline = serde_json::json!({"paths":{"bash":ctx.paths.bashrc,"zsh":ctx.paths.zshrc,"config":ctx.paths.terminal},"files":{"bash":{"data":"IyBvcmlnaW5hbAo=","mode":384},"zsh":null,"config":null}});
        fs::write(state.join("original.json"), baseline.to_string()).unwrap();
        fs::write(state.join("options.json"), "broken").unwrap();
        terminal::restore(&ctx).unwrap();
        assert_eq!(fs::read(&ctx.paths.bashrc).unwrap(), b"# original\n");
        assert!(!ctx.paths.terminal.exists());
        assert!(!state.join("original.json").exists());
    }

    #[test]
    fn terminal_apply_switch_and_restore_keep_first_bytes_and_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        for tool in ["starship", "zsh"] {
            executable(&temp.path().join("bin").join(tool), "#!/bin/sh\nexit 99\n");
        }
        fs::write(&ctx.paths.bashrc, b"# original\r\n").unwrap();
        fs::set_permissions(&ctx.paths.bashrc, fs::Permissions::from_mode(0o640)).unwrap();
        terminal::apply(&ctx, &terminal::Options::default()).unwrap();
        let first = fs::read(ctx.paths.state.join("terminal/original.json")).unwrap();
        let settings = terminal::Options {
            shell: "zsh".into(),
            ..Default::default()
        };
        terminal::apply(&ctx, &settings).unwrap();
        assert_eq!(
            fs::read(ctx.paths.state.join("terminal/original.json")).unwrap(),
            first
        );
        assert_eq!(fs::read(&ctx.paths.bashrc).unwrap(), b"# original\r\n");
        assert!(
            fs::read_to_string(&ctx.paths.zshrc)
                .unwrap()
                .contains("starship init zsh")
        );
        terminal::restore(&ctx).unwrap();
        assert!(!ctx.paths.zshrc.exists());
        assert!(!ctx.paths.terminal.exists());
        assert_eq!(storage::mode(&ctx.paths.bashrc).unwrap(), 0o640);
    }
    #[test]
    fn terminal_generated_bash_parses_and_cannot_execute_path_contents() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let marker = temp.path().join("NOT_EXECUTED");
        let config = format!(
            "{}/a'$(touch {})\".toml",
            temp.path().display(),
            marker.display()
        );
        let output = terminal::edit_rc(b"# user\n", "bash", true, &config).unwrap();
        let rc = temp.path().join("test.bashrc");
        fs::write(&rc, output).unwrap();
        ctx.run("bash", &["-n", rc.to_str().unwrap()]).unwrap();
        assert!(!marker.exists());
    }
    #[test]
    fn terminal_rejects_changed_paths_shared_lock_and_incomplete_journal() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = boundary(temp.path());
        executable(&temp.path().join("bin/starship"), "#!/bin/sh\nexit 99\n");
        terminal::apply(&ctx, &terminal::Options::default()).unwrap();
        let previous = ctx.paths.terminal.clone();
        ctx.paths.terminal = temp.path().join("other.toml");
        assert_eq!(terminal::restore(&ctx).unwrap_err().key, "error.snapshot");
        ctx.paths.terminal = ctx.paths.state.join("terminal/config.lock");
        assert_eq!(
            terminal::apply(&ctx, &terminal::Options::default())
                .unwrap_err()
                .key,
            "error.path"
        );
        ctx.paths.terminal = previous;
        let lock = Lock::acquire(&ctx.paths.state.join("terminal/config.lock")).unwrap();
        assert_eq!(terminal::restore(&ctx).unwrap_err().key, "error.lock");
        drop(lock);
        fs::write(ctx.paths.state.join("terminal/pending.json"), "pending").unwrap();
        assert_eq!(terminal::restore(&ctx).unwrap_err().key, "error.pending");
    }
    #[test]
    fn network_cannot_overwrite_baseline_with_different_parameter_set() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        network::apply(&ctx, &ctx.paths.bbr, &desired()).unwrap();
        let original = fs::read(&ctx.paths.bbr).unwrap();
        let changed = network::parse("net.core.default_qdisc=cake").unwrap();
        assert_eq!(
            network::apply(&ctx, &ctx.paths.bbr, &changed)
                .unwrap_err()
                .key,
            "error.snapshot"
        );
        assert_eq!(fs::read(&ctx.paths.bbr).unwrap(), original);
    }
    #[test]
    fn incomplete_network_baseline_cannot_be_used_for_restore() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = boundary(temp.path());
        network_values(temp.path());
        let backup = storage::suffix(&ctx.paths.bbr, ".cqs-backup");
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("runtime.conf"), "net.core.default_qdisc=fq").unwrap();
        assert_eq!(
            network::restore(&ctx, &ctx.paths.bbr).unwrap_err().key,
            "error.snapshot"
        );
        assert!(!temp.path().join("calls").exists());
    }
    #[test]
    fn native_deb_metadata_and_bbr_configuration_are_verified_without_installing() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let selected = kernel::manifest(
            &release("amd64", "6.10.0", false).to_string(),
            "amd64",
            "standard",
            None,
        )
        .unwrap();
        for kind in ["image", "headers"] {
            let source = temp.path().join(format!("source-{kind}"));
            fs::create_dir_all(source.join("DEBIAN")).unwrap();
            fs::write(source.join("DEBIAN/control"), format!("Package: linux-{kind}-{}\nVersion: 1\nArchitecture: amd64\nMaintainer: CqShell tests <tests@example.invalid>\nDescription: fixture only\n", selected.kernel)).unwrap();
            if kind == "image" {
                fs::create_dir(source.join("boot")).unwrap();
                fs::write(source.join(format!("boot/config-{}", selected.kernel)), "CONFIG_TCP_CONG_BBR=y\nCONFIG_NET_SCH_FQ=y\nCONFIG_HYPERV=y\nCONFIG_HYPERV_STORAGE=y\nCONFIG_HYPERV_NET=y\nCONFIG_PCI_HYPERV=y\nCONFIG_BLK_DEV_SD=y\n").unwrap();
            }
            ctx.run(
                "dpkg-deb",
                &[
                    "--build",
                    source.to_str().unwrap(),
                    temp.path().join(format!("{kind}.deb")).to_str().unwrap(),
                ],
            )
            .unwrap();
        }
        kernel::validate_packages(&ctx, temp.path(), &selected, "amd64").unwrap();
        assert!(kernel::validate_packages(&ctx, temp.path(), &selected, "arm64").is_err());
        fs::write(temp.path().join("image.deb"), "corrupt").unwrap();
        assert!(kernel::validate_packages(&ctx, temp.path(), &selected, "amd64").is_err());
    }

    #[test]
    fn kernel_reboot_requires_a_fresh_confirmation_even_with_yes() {
        use crate::runtime::Event;
        use std::{sync::mpsc, time::Duration};

        for yes in [false, true] {
            for accept in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let mut ctx = boundary(temp.path());
                ctx.yes = yes;
                ctx.interactive = false;
                // Stop at the reboot process boundary; never reboot the test host.
                executable(
                    &temp.path().join("bin/reboot"),
                    "#!/bin/sh\ntouch \"$CQS_TEST_ROOT/reboot-requested\"\nexit 73\n",
                );
                let (tx, rx) = mpsc::channel();
                ctx.events = Some(tx);
                let worker = std::thread::spawn(move || {
                    kernel::reboot_after_install(
                        &ctx,
                        "7.3.0-joeyblog-bbrv3",
                        "6.12.86+deb13-amd64",
                    )
                });
                let mut prompted = false;
                let mut logs = Vec::new();
                while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
                    match event {
                        Event::Confirm { text, token, reply } => {
                            assert!(!prompted);
                            assert!(text.contains("7.3.0-joeyblog-bbrv3"));
                            assert!(text.contains("SSH"));
                            assert_eq!(token, "YES");
                            prompted = true;
                            reply.send(accept).unwrap();
                        }
                        Event::Log(text) => logs.push(text),
                        Event::Finished(_) => panic!("unexpected completion event"),
                    }
                }
                let result = worker.join().unwrap();
                assert!(prompted, "--yes must not authorize a reboot");
                assert!(logs.iter().any(|line| line.contains("uname -r")));
                assert!(logs.iter().any(|line| line.contains("6.12.86+deb13-amd64")));
                assert_eq!(temp.path().join("reboot-requested").exists(), accept);
                if accept {
                    let error = result.unwrap_err();
                    assert_eq!(error.key, "error.command");
                    assert!(error.detail.contains("reboot"));
                    assert!(error.detail.contains("73"));
                } else {
                    result.unwrap();
                    assert!(logs.iter().any(|line| line.contains("sudo reboot")));
                }
            }
        }
    }

    #[test]
    fn kernel_reboot_is_deferred_without_a_terminal() {
        for yes in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut ctx = boundary(temp.path());
            ctx.yes = yes;
            ctx.interactive = false;
            executable(
                &temp.path().join("bin/reboot"),
                "#!/bin/sh\ntouch \"$CQS_TEST_ROOT/reboot-requested\"\nexit 73\n",
            );
            kernel::reboot_after_install(&ctx, "7.3.0-joeyblog-bbrv3", "6.12.86+deb13-amd64")
                .unwrap();
            assert!(!temp.path().join("reboot-requested").exists());
        }
    }

    #[test]
    fn kernel_install_failure_restores_grub_but_reports_partial_package_state() {
        for package_failure in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let ctx = boundary(temp.path());
            fs::create_dir_all(ctx.paths.boot.join("grub")).unwrap();
            fs::write(
                ctx.paths.boot.join("grub/grub.cfg"),
                "old boot configuration",
            )
            .unwrap();
            let apt = format!(
                "#!/bin/bash\nprintf 'changed by package hook' > \"$CQS_TEST_ROOT/boot/grub/grub.cfg\"\nexit {}\n",
                if package_failure { 27 } else { 0 }
            );
            executable(&temp.path().join("bin/apt-get"), &apt);
            executable(
                &temp.path().join("bin/dpkg-query"),
                "#!/bin/sh\nprintf 'install ok installed'\n",
            );
            let selected = kernel::manifest(
                &release("amd64", "6.10.0", false).to_string(),
                "amd64",
                "standard",
                None,
            )
            .unwrap();
            let _lock = Lock::acquire(&ctx.paths.kernel_lock).unwrap();
            let error = kernel::install_verified(
                &ctx,
                temp.path(),
                &selected,
                "CONFIG_TCP_CONG_BBR=y\n",
                "6.1.0-debian",
            )
            .unwrap_err();
            assert_eq!(error.key, "error.kernel_partial");
            assert_eq!(
                fs::read_to_string(ctx.paths.boot.join("grub/grub.cfg")).unwrap(),
                "old boot configuration"
            );
            assert_eq!(fs::read_dir(&ctx.paths.kernel_backup).unwrap().count(), 1);
            assert!(Lock::acquire(&ctx.paths.kernel_lock).is_err());
        }
    }

    #[test]
    fn cpu_usage_counts_idle_and_iowait_without_double_counting_guests() {
        let value = system::cpu_usage(
            "cpu 10 0 10 60 20 0 0 0 3 0",
            "cpu 20 0 20 120 40 0 0 0 9 0",
        )
        .unwrap();
        assert!((value - 20.0).abs() < 0.001);
        assert!(system::cpu_usage("cpu 1 2 3 4", "cpu 1 2 3 4").is_err());
    }
    #[test]
    fn curl_real_tls_rejects_downgrade_truncation_and_invalid_script() {
        use std::{
            io::{BufRead, BufReader},
            process::{Command, Stdio},
        };
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = context(temp.path());
        let cert = temp.path().join("cert.pem");
        let key = temp.path().join("key.pem");
        let out = Command::new("openssl")
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
        assert!(out.status.success());
        // Python is only a local TLS test fixture, never a runtime dependency.
        let fixture = r#"
import http.server, ssl, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/redirect':
            self.send_response(302)
            self.send_header('Location', 'http://127.0.0.1:1/unsafe')
            self.end_headers()
            return
        data = b'exit 7\n' if self.path != '/syntax' else b'if\n'
        self.send_response(200)
        self.send_header('Content-Length', str(len(data) + (100 if self.path == '/truncated' else 0)))
        self.end_headers()
        self.wfile.write(data)
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
            .stderr(Stdio::inherit())
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
        ctx.environment
            .insert("no_proxy".into(), "localhost,127.0.0.1".into());
        let base = format!("https://localhost:{}", port.trim());
        let target = temp.path().join("download.sh");
        ctx.download(&format!("{base}/complete"), &target, 5)
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"exit 7\n");
        assert_eq!(
            ctx.command("bash", &[target.to_str().unwrap()], false)
                .unwrap()
                .status
                .code(),
            Some(7)
        );
        for endpoint in ["redirect", "truncated"] {
            assert!(
                ctx.download(&format!("{base}/{endpoint}"), &target, 5)
                    .is_err()
            );
        }
        assert!(ctx.download("http://localhost/unsafe", &target, 5).is_err());
        assert!(crate::ops::tools::remote(&ctx, &format!("{base}/syntax")).is_err());
    }
}
