//! `cqs batch` without an event receiver prints what it printed before `batch_report` and
//! `Context::step` existed (requirements 11 and 14.2).
//!
//! `tests/snapshots/cli/batch_old.<environment>.txt` was recorded with the binary built from
//! commit 9b0f222, the source before that change, using the invocation in [`transcript`]:
//! build that commit outside this checkout and run
//! `CQS_OLD_BINARY=<old cqs> cargo test --locked --test cli_batch -- --ignored`.
//! The test applies the two intended changes to that old transcript and requires the current
//! binary to print the result line by line:
//!
//! 1. `confirm.batch` lists every job as `n. name (k=v, …)` with all parameters, defaults
//!    included, and only the name for jobs without parameters (requirement 11.1).
//! 2. With `--yes`, batch items no longer log the generic `confirm.action` text
//!    (requirement 11.2); terminal.restore keeps its RESTORE confirmation (requirement 11.3).
//!
//! Progress and completion lines, operation output, error lines, the final error and the
//! exit status are unchanged.
//!
//! Every job stops before it changes anything: on Windows the platform check rejects each
//! operation; on Debian or Ubuntu as a regular user the root-only actions stop at the root
//! check and terminal.apply stops because `PATH` has no starship. The test refuses to run in
//! other environments, notably as root, where these actions would reach the live system.
use cqs::{
    actions::{self, Parameters},
    i18n::I18n,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// One `cqs batch` invocation.
struct Case {
    name: &'static str,
    yes: bool,
    /// Arguments after `batch`.
    args: &'static [&'static str],
}

const CASES: &[Case] = &[
    // Without --yes and without a terminal, the batch confirmation fails before any job.
    Case {
        name: "no_yes",
        yes: false,
        args: &["tcp.tune", "terminal.status"],
    },
    // tcp.tune fails and the batch continues; terminal.status succeeds on Linux;
    // terminal.apply asked for the generic confirmation before and fails, so the batch stops
    // before system.update.
    Case {
        name: "yes_mixed",
        yes: true,
        args: &[
            "tcp.tune",
            "terminal.status",
            "terminal.apply",
            "system.update",
            "--set",
            "tcp.tune.rtt=80",
        ],
    },
    // terminal.restore still asks for RESTORE inside a batch.
    Case {
        name: "yes_restore",
        yes: true,
        args: &["tcp.tune", "terminal.restore"],
    },
    // Only continuable failures: the batch ends with error.batch listing both IDs.
    Case {
        name: "yes_continue",
        yes: true,
        args: &[
            "bbr.enable",
            "tcp.tune",
            "--set",
            "bbr.enable.qdisc=fq_codel",
        ],
    },
];
const LOCALES: &[&str] = &["en-US", "zh-CN"];

fn run(binary: &Path, locale: &str, case: &Case) -> String {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        // An empty directory: terminal.apply cannot find starship.
        .env("PATH", &bin)
        .stdin(Stdio::null());
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        command.env("SYSTEMROOT", root);
    }
    command.args(["--lang", locale]);
    if case.yes {
        command.arg("--yes");
    }
    let output = command.arg("batch").args(case.args).output().unwrap();
    let normalize = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace("\r\n", "\n")
            // Paths under the temporary home differ between runs.
            .replace(&home.display().to_string(), "<HOME>")
    };
    format!(
        "=== {} {locale}\n$ cqs --lang {locale}{} batch {}\nexit: {}\n--- stdout\n{}--- stderr\n{}",
        case.name,
        if case.yes { " --yes" } else { "" },
        case.args.join(" "),
        output
            .status
            .code()
            .map_or_else(|| "signal".into(), |code| code.to_string()),
        normalize(&output.stdout),
        normalize(&output.stderr)
    )
}

fn transcript(binary: &Path) -> String {
    LOCALES
        .iter()
        .flat_map(|locale| CASES.iter().map(move |case| run(binary, locale, case)))
        .collect()
}

#[cfg(windows)]
fn environment() -> &'static str {
    "windows"
}
#[cfg(target_os = "linux")]
fn environment() -> &'static str {
    let os = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let id = os
        .lines()
        .find_map(|l| l.strip_prefix("ID="))
        .unwrap_or("")
        .trim_matches('"');
    let root = unsafe { libc::geteuid() } == 0;
    assert!(
        matches!(id, "debian" | "ubuntu") && !root,
        "`cqs batch` was recorded on Debian/Ubuntu as a regular user; here ID={id:?}, root={root}. \
         As root the batch would change this system, so run the test as a regular user."
    );
    "linux"
}
#[cfg(not(any(windows, target_os = "linux")))]
fn environment() -> &'static str {
    panic!(
        "no recording of `cqs batch` for {}; record one before relying on this test",
        std::env::consts::OS
    )
}

/// The jobs of a case with all their parameters, defaults included.
fn jobs(case: &Case) -> Vec<(actions::Spec, Parameters)> {
    let (ids, sets) = match case.args.iter().position(|a| *a == "--set") {
        Some(i) => case.args.split_at(i),
        None => (case.args, &[][..]),
    };
    ids.iter()
        .map(|id| {
            let spec = actions::find(id).unwrap();
            let mut parameters = spec.defaults();
            for set in sets.iter().filter(|s| **s != "--set") {
                let (key, value) = set.split_once('=').unwrap();
                if let Some(key) = key.strip_prefix(&format!("{id}.")) {
                    parameters.insert(key.into(), value.into());
                }
            }
            (spec, parameters)
        })
        .collect()
}

/// Replace the only occurrence of `from` in `text`.
fn replace_once(text: &mut String, from: &str, to: &str, what: &str) {
    assert_eq!(
        text.matches(from).count(),
        1,
        "{what} must occur once in the old transcript:\n{from}"
    );
    *text = text.replacen(from, to, 1);
}

/// The recorded transcript of the old binary in `environment`.
fn recording(environment: &str) -> (String, PathBuf) {
    let name = format!("batch_old.{environment}.txt");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/cli")
        .join(&name);
    (name, path)
}

/// Writes the recording from the old binary named by `CQS_OLD_BINARY`. Only run on request:
/// the current binary must never overwrite the file it is compared with.
#[test]
#[ignore = "records batch_old.<environment>.txt; needs CQS_OLD_BINARY"]
fn record_the_old_binary() {
    let binary = std::env::var_os("CQS_OLD_BINARY")
        .expect("set CQS_OLD_BINARY to cqs built from the source before batch_report");
    let (_, path) = recording(environment());
    fs::write(path, transcript(Path::new(&binary))).unwrap();
}

#[test]
fn batch_output_without_an_event_receiver_matches_the_old_binary() {
    let environment = environment();
    let (name, path) = recording(environment);
    // A checkout may convert the recorded file to CRLF; the program prints LF.
    let old = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
    let mut expected = old.clone();
    for locale in LOCALES {
        let i18n = I18n::load(Some(locale), None).unwrap();
        for case in CASES.iter().filter(|c| c.yes) {
            let jobs = jobs(case);
            // Change 1: the batch confirmation lists numbered jobs with their parameters.
            let names = jobs
                .iter()
                .map(|(spec, _)| i18n.text(&spec.label()).to_owned())
                .collect::<Vec<_>>();
            let numbered = jobs
                .iter()
                .zip(&names)
                .enumerate()
                .map(|(i, ((_, parameters), name))| {
                    if parameters.is_empty() {
                        format!("{}. {name}", i + 1)
                    } else {
                        let values = parameters
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>();
                        format!("{}. {name} ({})", i + 1, values.join(", "))
                    }
                })
                .collect::<Vec<_>>();
            replace_once(
                &mut expected,
                &i18n.format("confirm.batch", &[("detail", &names.join("\n"))]),
                &i18n.format("confirm.batch", &[("detail", &numbered.join("\n"))]),
                &format!("confirm.batch of {} {locale}", case.name),
            );
            // Change 2: the generic confirmation is logged only for terminal.restore. The
            // operations reach it only where the platform and root checks pass.
            for (spec, parameters) in &jobs {
                let generic = i18n.format(
                    "confirm.action",
                    &[(
                        "detail",
                        &format!(
                            "{}\n{}\n{}",
                            i18n.text(&spec.label()),
                            i18n.text(&spec.description()),
                            parameters
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ),
                    )],
                );
                let reached = environment == "linux"
                    && matches!(spec.id, "terminal.apply" | "terminal.restore");
                assert_eq!(
                    old.matches(&generic).count(),
                    usize::from(reached),
                    "generic confirmation of {} in {} {locale}",
                    spec.id,
                    case.name
                );
                if reached && spec.id != "terminal.restore" {
                    replace_once(
                        &mut expected,
                        &format!("{generic}\n"),
                        "",
                        &format!("generic confirmation of {}", spec.id),
                    );
                }
            }
        }
    }
    let actual = transcript(Path::new(env!("CARGO_BIN_EXE_cqs")));
    if let Some((line, (a, e))) = actual
        .split('\n')
        .zip(expected.split('\n'))
        .enumerate()
        .find(|(_, (a, e))| a != e)
    {
        panic!(
            "`cqs batch` differs from {name} at line {}:\n  actual:   {a}\n  expected: {e}",
            line + 1
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "`cqs batch` differs in length"
    );
}
