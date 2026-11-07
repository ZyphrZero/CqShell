//! `cqs list` and `cqs list --json` must not change when actions gain attributes that are
//! internal to the TUI, such as `reversible` (requirement 12.7). The expected files were
//! recorded from the binary before `Spec::reversible` existed, with an empty home directory
//! and an explicit `--lang`, so neither the host locale nor saved preferences affect them.
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn list_output_is_unchanged_and_omits_reversible() {
    for locale in ["en-US", "zh-CN"] {
        for (extension, flags) in [("txt", &[][..]), ("json", &["--json"][..])] {
            let name = format!("list_{locale}.{extension}");
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/snapshots/cli")
                .join(&name);
            // A checkout may convert the recorded files to CRLF; the program prints LF.
            let expected = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            let temp = tempfile::tempdir().unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_cqs"))
                .env("HOME", temp.path())
                .env("USERPROFILE", temp.path())
                .env("XDG_CONFIG_HOME", temp.path().join("config"))
                .env("XDG_STATE_HOME", temp.path().join("state"))
                .env_remove("CQS_LANG")
                .stdin(Stdio::null())
                .args(["--lang", locale, "list"])
                .args(flags)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let actual = String::from_utf8(output.stdout).unwrap();
            if let Some((line, (a, e))) = actual
                .split('\n')
                .zip(expected.split('\n'))
                .enumerate()
                .find(|(_, (a, e))| a != e)
            {
                panic!(
                    "{name} differs at line {}:\n  actual:   {a}\n  expected: {e}",
                    line + 1
                );
            }
            assert_eq!(actual.len(), expected.len(), "{name} differs in length");
        }
    }
}
