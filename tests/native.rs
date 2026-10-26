#![cfg(target_os = "linux")]
use cqs::ops::{docker, ssh};
use std::{fs, process::Command};

#[test]
#[ignore = "requires native sshd; run explicitly with CQS_TEST_SSHD or an installed sshd"]
fn native_sshd_validates_generated_port_and_detects_listen_conflicts() {
    let binary = std::env::var("CQS_TEST_SSHD").unwrap_or_else(|_| "/usr/sbin/sshd".into());
    let temp = tempfile::tempdir().unwrap();
    let key = temp.path().join("host_key");
    assert!(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap()
            .success()
    );
    for listen in ["127.0.0.1", "127.0.0.1:22"] {
        let original = format!(
            "HostKey {}\nUsePAM no\nPort 22\nListenAddress {listen}\n",
            key.display()
        );
        let settings = [("Port", "2222".into())];
        let config = temp.path().join("sshd_config");
        fs::write(&config, ssh::candidate(&original, &settings)).unwrap();
        let output = Command::new(&binary)
            .args(["-T", "-f"])
            .arg(&config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            ssh::validate_effective(&String::from_utf8(output.stdout).unwrap(), &settings).is_ok(),
            listen == "127.0.0.1"
        );
    }
}
#[test]
#[ignore = "requires native dockerd --validate; run explicitly with CQS_TEST_DOCKERD"]
fn native_dockerd_validates_merged_ipv6_and_mirror_configuration() {
    let binary = std::env::var("CQS_TEST_DOCKERD").unwrap_or_else(|_| "dockerd".into());
    let temp = tempfile::tempdir().unwrap();
    let binary_path = std::path::Path::new(&binary);
    let parent = binary_path
        .parent()
        .unwrap_or(std::path::Path::new("/usr/bin"));
    let mut search = vec![
        parent.to_path_buf(),
        parent.parent().unwrap_or(parent).join("bin"),
    ];
    search.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let file = temp.path().join("daemon.json");
    let original = r#"{"data-root":"/srv/docker","log-driver":"journald"}"#;
    for change in [
        docker::Change::Ipv6(true),
        docker::Change::Ipv6(false),
        docker::Change::Mirror("https://mirror.example.com"),
    ] {
        fs::write(
            &file,
            docker::candidate(original, change, "fd42:1234::/64").unwrap(),
        )
        .unwrap();
        let output = Command::new(&binary)
            .env("PATH", std::env::join_paths(&search).unwrap())
            .args(["--validate", "--config-file"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn native_sysctl_parser_accepts_multivalue_configuration_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sysctl.conf");
    fs::write(
        &file,
        "net.ipv4.tcp_rmem = 4096 131072 1048576\nnet.core.default_qdisc = fq\n",
    )
    .unwrap();
    let binary = std::env::var("CQS_TEST_SYSCTL").unwrap_or_else(|_| "/usr/sbin/sysctl".into());
    let output = Command::new(binary)
        .args(["--dry-run", "-p"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("4096 131072 1048576"));
}
