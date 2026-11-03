use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn cli(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cqs"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("CQS_CLASH_HOME", home.join("clash"))
        .env_remove("CQS_LANG")
        .env_remove("CQS_CLASH_BINARY")
        .stdin(Stdio::null());
    command
}

#[test]
fn compatibility_help_and_shell_initialization_are_bilingual_and_read_only() {
    for locale in ["en-US", "zh-CN"] {
        let temp = tempfile::tempdir().unwrap();
        for args in [
            vec!["clashctl"],
            vec!["clashctl", "help"],
            vec!["clashctl", "--help"],
            vec!["clashctl", "--version"],
            vec!["clashctl", "-v"],
            vec!["clashctl", "sub", "add", "--help"],
            vec!["clashctl", "off", "--help"],
            vec!["clashctl", "shell-init", "--shell", "bash"],
            vec!["clashctl", "shell-init", "--shell", "zsh"],
            vec!["clashctl", "shell-init", "--shell", "fish"],
        ] {
            let output = cli(temp.path())
                .args(["--lang", locale])
                .args(&args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
            assert!(!String::from_utf8_lossy(&output.stdout).contains("__CQS_"));
        }
    }
}

#[test]
fn unsupported_commands_and_non_shell_proxy_switches_fail_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["clashctl", "on"],
        vec!["clashctl", "off"],
        vec!["clashctl", "off", "--stop"],
        vec!["clashctl", "update"],
        vec!["clashctl", "upgrade", "--alpha"],
        vec!["clashctl", "node", "delay", "-p", "node"],
        vec!["clashctl", "mixin", "-e"],
        vec!["clashctl", "secret", "test-value"],
        vec![
            "clashctl",
            "sub",
            "add",
            "--convert",
            "https://example.invalid/sub",
        ],
        vec!["clashctl", "sub", "update", "name", "--all"],
        vec!["clashctl", "sub", "use"],
    ] {
        let output = cli(temp.path()).args(&args).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        if args.contains(&"test-value") {
            assert!(!String::from_utf8_lossy(&output.stderr).contains("test-value"));
        }
    }
}

#[test]
fn original_command_names_and_flags_parse_to_the_rust_compatibility_dispatcher() {
    let i18n = cqs::i18n::I18n::load(Some("en-US"), None).unwrap();
    for args in [
        vec!["clashctl", "start"],
        vec![
            "clashctl",
            "sub",
            "add",
            "-n",
            "main",
            "--use",
            "https://example.invalid/sub",
        ],
        vec!["clashctl", "sub", "ls"],
        vec!["clashctl", "sub", "delete", "main"],
        vec!["clashctl", "node", "use", "group", "node"],
        vec!["clashctl", "node", "ls", "group"],
        vec!["clashctl", "node", "group", "node"],
        vec!["clashctl", "tun", "off"],
        vec!["clashctl", "mixin", "-c"],
        vec!["clashctl", "mixin", "-r"],
        vec!["clashctl", "log", "-n", "50", "-f"],
        vec!["clashctl", "upgrade", "-r", "-v"],
        vec!["clashon"],
        vec!["clashoff", "--stop"],
        vec!["clashsub", "list"],
    ] {
        let mut normalized = args.iter().map(|value| value.to_string()).collect();
        cqs::ops::clash::compat::normalize_arguments(&mut normalized);
        let matches = cqs::cli::command(&i18n)
            .try_get_matches_from(normalized)
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        assert_eq!(matches.subcommand_name(), Some("clashctl"));
    }
}

#[cfg(unix)]
#[test]
fn executable_alias_and_shell_functions_preserve_parent_shell_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let shell = cli(temp.path())
        .args(["clashctl", "shell-init", "--shell", "bash"])
        .output()
        .unwrap();
    assert!(shell.status.success());
    let init = temp.path().join("init.sh");
    fs::write(&init, shell.stdout).unwrap();
    let data = temp.path().join("clash/data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("mihomo.pid"), b"invalid pid").unwrap();
    assert!(
        Command::new("bash")
            .arg("-n")
            .arg(&init)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c", r#"
set -eu
source "$1"
export http_proxy=original HTTP_PROXY=original https_proxy=original HTTPS_PROXY=original
export all_proxy=original ALL_PROXY=original no_proxy=original NO_PROXY=original
clashoff --help >/dev/null
test "$http_proxy" = original
if clashon invalid >/dev/null 2>&1; then exit 21; fi
test "$http_proxy" = original
if clashon >/dev/null 2>&1; then exit 22; fi
test "$http_proxy" = original
clashoff
test -z "${http_proxy+x}${HTTP_PROXY+x}${https_proxy+x}${HTTPS_PROXY+x}${all_proxy+x}${ALL_PROXY+x}${no_proxy+x}${NO_PROXY+x}"
export http_proxy=original HTTPS_PROXY=original
if clashoff --stop >/dev/null 2>&1; then exit 23; fi
test -z "${http_proxy+x}${HTTPS_PROXY+x}"
clashhelp >/dev/null
clashversion >/dev/null
"#, "--"])
        .arg(&init)
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("CQS_CLASH_HOME", temp.path().join("clash"))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let alias = temp.path().join("clashctl");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_cqs"), &alias).unwrap();
    let output = Command::new(&alias)
        .arg("help")
        .env("HOME", temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("shell-init"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("clashctl clashctl"));
    let output = Command::new(alias)
        .arg("--help")
        .env("HOME", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Usage: clashctl [OPTIONS] [COMMAND]")
    );
}

#[cfg(unix)]
#[test]
fn shell_initializer_quotes_executable_paths_without_command_injection() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("cqs ' $literal");
    fs::copy(env!("CARGO_BIN_EXE_cqs"), &executable).unwrap();
    let output = Command::new(&executable)
        .args(["clashctl", "shell-init", "--shell", "bash"])
        .env("HOME", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let init = temp.path().join("quoted-init.sh");
    fs::write(&init, output.stdout).unwrap();
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c", "set -eu; source \"$1\"; export http_proxy=old; clashoff; test -z \"${http_proxy+x}\"; clashversion", "--"])
        .arg(init)
        .env("HOME", temp.path())
        .env("CQS_CLASH_HOME", temp.path().join("clash"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("clashctl compatibility"));
}
