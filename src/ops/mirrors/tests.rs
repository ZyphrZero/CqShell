use super::*;
use crate::tests::context;

fn host() -> Host {
    Host::parse("ID=debian\nVERSION_CODENAME=trixie\n", "amd64").unwrap()
}

#[test]
fn release_architecture_and_security_routes_are_explicit() {
    for (distro, code, arch, branch, security) in [
        (
            "debian",
            "bookworm",
            "amd64",
            "debian",
            "security.debian.org",
        ),
        ("debian", "trixie", "arm64", "debian", "security.debian.org"),
        ("ubuntu", "noble", "amd64", "ubuntu", "security.ubuntu.com"),
        (
            "ubuntu",
            "resolute",
            "arm64",
            "ubuntu-ports",
            "ports.ubuntu.com",
        ),
    ] {
        let h = Host::parse(&format!("ID=\"{distro}\"\nVERSION_CODENAME={code}"), arch).unwrap();
        assert_eq!(h.branch(), branch);
        let args = arguments(&h, "tuna").unwrap();
        assert!(
            args.windows(2)
                .any(|a| a == ["--source-security", security])
        );
        assert!(
            args.windows(2)
                .any(|a| a == ["--upgrade-software", "false"])
        );
        assert!(args.windows(2).any(|a| a == ["--protocol", "https"]));
    }
    assert!(Host::parse("ID=debian\nVERSION_CODENAME=sid", "amd64").is_err());
    assert!(Host::parse("ID=ubuntu\nVERSION_CODENAME=noble", "riscv64").is_err());
    assert!(host().domain("tuna; id").is_err());
}

#[test]
fn speed_samples_reject_html_partial_errors_nan_and_ignored_ranges() {
    let mut data = vec![0; SAMPLE as usize];
    data[..6].copy_from_slice(b"\xfd7zXZ\0");
    let headers = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-1048575/9000000\r\n";
    assert_eq!(
        parse_measurement("206\t1048576\t0.5\t0.1", &data, headers).unwrap(),
        (2.0, 100.0)
    );
    for output in [
        "200\t1048576\t0.5\t0.1",
        "206\t512\t0.5\t0.1",
        "206\t1048576\tNaN\t0.1",
        "206\t1048576\t0\t0",
        "206\t1048576\t1\t2",
        "206\t1048576\t1\tinf",
    ] {
        assert!(parse_measurement(output, &data, headers).is_err());
    }
    assert!(parse_measurement("206\t1048576\t1\t0.1", &data[..100], headers).is_err());
    assert!(parse_measurement("206\t1048576\t1\t0.1", &data, "").is_err());
    data[..6].copy_from_slice(b"<html>");
    assert!(parse_measurement("206\t1048576\t1\t0.1", &data, headers).is_err());
}

#[test]
fn custom_mixed_sources_and_trust_overrides_stop_before_replacement() {
    let h = host();
    let source = "deb https://deb.debian.org/debian trixie main\n";
    check_source(&h, "sources.list", source).unwrap();
    check_source(&h, "sources.list", "deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian trixie main\n").unwrap();
    for extra in [
        "deb https://download.docker.com/linux/debian trixie stable",
        "deb [trusted=yes] https://deb.debian.org/debian trixie main",
        "deb https://deb.debian.org/debian stable main",
        "deb https://deb.debian.org/debian bookworm main",
    ] {
        assert!(check_source(&h, "sources.list", &format!("{source}{extra}\n")).is_err());
    }
    let deb822 = "Types: deb\nURIs: https://deb.debian.org/debian\nSuites: trixie trixie-updates\nComponents: main\nSigned-By: /usr/share/keyrings/debian-archive-keyring.gpg\n";
    check_source(&h, "debian.sources", deb822).unwrap();
    for extra in [
        "Trusted: yes\n",
        "URIs: https://other.invalid\n",
        "Architectures: i386\n",
        "Enabled: no\n",
    ] {
        assert!(check_source(&h, "debian.sources", &format!("{deb822}{extra}")).is_err());
    }
}

#[test]
fn real_files_rollback_on_error_and_cancellation_and_undo_checks_for_edits() {
    let temp = tempfile::tempdir().unwrap();
    let apt = temp.path().join("apt");
    fs::create_dir_all(apt.join("sources.list.d")).unwrap();
    let ctx = context(temp.path());
    let h = host();
    let target = apt.join("sources.list");
    let third_party = apt.join("sources.list.d/docker.list");
    fs::write(&target, b"# original\r\n").unwrap();
    fs::write(&third_party, b"keep me").unwrap();
    let before = capture(&apt, &h).unwrap();
    let error = transaction(&ctx, &apt, &h, before.clone(), || {
        fs::write(&target, "broken")?;
        fs::write(apt.join("sources.list.d/debian.sources"), "partial")?;
        Err(Error::new("error.command", "real failure"))
    })
    .unwrap_err();
    assert_eq!(error.key, "error.command");
    assert_eq!(capture(&apt, &h).unwrap(), before);
    assert!(!apt.join(STATE).join("pending.json").exists());
    transaction(&ctx, &apt, &h, before.clone(), || {
        fs::write(&target, "changed")?;
        Ok(())
    })
    .unwrap();
    let last = fs::read(apt.join(STATE).join("last.json")).unwrap();
    let current = capture(&apt, &h).unwrap();
    let err = transaction(&ctx, &apt, &h, current.clone(), || {
        fs::write(&target, "cancelled")?;
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.key, "error.cancelled");
    assert_eq!(capture(&apt, &h).unwrap(), current);
    assert_eq!(fs::read(apt.join(STATE).join("last.json")).unwrap(), last);
    fs::write(&target, "manual edit").unwrap();
    assert_eq!(
        restore_files(&apt, &h).unwrap_err().key,
        "error.mirror_changed"
    );
    fs::write(&target, "changed").unwrap();
    restore_files(&apt, &h).unwrap();
    assert_eq!(capture(&apt, &h).unwrap(), before);
    assert_eq!(fs::read(&third_party).unwrap(), b"keep me");
}

#[test]
fn durable_pending_recovery_is_validated_and_retryable() {
    let temp = tempfile::tempdir().unwrap();
    let apt = temp.path().join("apt");
    fs::create_dir_all(apt.join(STATE)).unwrap();
    let h = host();
    let before = capture(&apt, &h).unwrap();
    let mut record = Record {
        version: 1,
        distro: "debian".into(),
        before: before.clone(),
        after: None,
        previous: None,
    };
    let pending = apt.join(STATE).join("pending.json");
    record.before.insert("../../escape".into(), None);
    fs::write(&pending, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(restore_files(&apt, &h).unwrap_err().key, "error.snapshot");
    assert!(pending.exists());
    record.before = before.clone();
    fs::write(&pending, serde_json::to_vec(&record).unwrap()).unwrap();
    fs::write(apt.join("sources.list"), "interrupted").unwrap();
    restore_files(&apt, &h).unwrap();
    assert_eq!(capture(&apt, &h).unwrap(), before);
    assert!(!pending.exists());
}

#[test]
fn action_registry_validates_fixed_choices_and_allows_read_only_benchmark() {
    use crate::actions::{self, Parameters};
    let speed = actions::find("system.mirror.test").unwrap();
    assert!(!speed.root && !speed.changes && !speed.external);
    let apply = actions::find("system.mirror.apply").unwrap();
    assert!(apply.root && apply.changes && !apply.external);
    assert_eq!(apply.defaults()["mirror_site"], "auto");
    assert!(
        apply
            .parameters(&Parameters::from([(
                "mirror_site".into(),
                "https://evil.invalid".into()
            )]))
            .is_err()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rust_and_shell_can_undo_each_others_real_journals() {
    let temp = tempfile::tempdir().unwrap();
    let apt = temp.path().join("apt");
    fs::create_dir_all(apt.join("sources.list.d")).unwrap();
    let source = apt.join("sources.list");
    fs::write(&source, b"# original\r\n").unwrap();
    let h = host();
    let ctx = context(temp.path());
    let before = capture(&apt, &h).unwrap();
    transaction(&ctx, &apt, &h, before.clone(), || {
        fs::write(&source, "Rust change")?;
        Ok(())
    })
    .unwrap();
    let code = r#"
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from test_shell_mirrors import backend
m = backend()
apt = Path(sys.argv[2])
host = m.Host('ID=debian\nVERSION_CODENAME=trixie', 'amd64')
m.restore_files(apt, host)
assert (apt / 'sources.list').read_bytes() == b'# original\r\n'
m.transaction(apt, host, m.capture(apt, host), lambda: (apt / 'sources.list').write_text('Shell change'))
"#;
    ctx.run(
        "python3",
        &[
            "-c",
            code,
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests"),
            path(&apt).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"Shell change");
    restore_files(&apt, &h).unwrap();
    assert_eq!(capture(&apt, &h).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn failed_recovery_keeps_journal_and_never_follows_a_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let apt = temp.path().join("apt");
    fs::create_dir_all(&apt).unwrap();
    let source = apt.join("sources.list");
    let unrelated = temp.path().join("unrelated");
    fs::write(&source, "original").unwrap();
    fs::write(&unrelated, "keep me").unwrap();
    let h = host();
    let error = transaction(
        &context(temp.path()),
        &apt,
        &h,
        capture(&apt, &h).unwrap(),
        || {
            fs::remove_file(&source)?;
            std::os::unix::fs::symlink(&unrelated, &source)?;
            Err(Error::new("error.command", "operation failure"))
        },
    )
    .unwrap_err();
    assert_eq!(error.key, "error.recovery");
    assert_eq!(fs::read(&unrelated).unwrap(), b"keep me");
    assert!(apt.join(STATE).join("pending.json").exists());
    fs::remove_file(&source).unwrap();
    restore_files(&apt, &h).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"original");
}
