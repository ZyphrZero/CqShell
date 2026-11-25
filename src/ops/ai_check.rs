//! ipforai.cc terminal checker integration. The upstream script retains ownership of its report.
use crate::{
    error::{Error, Result},
    runtime::{Context, path},
};
use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

pub const PROJECT: &str = "https://github.com/xjokerabab/ipforai";
pub const SCRIPT_URL: &str = "https://ipforai.cc/sh";
pub const REPORT_MODES: &[&str] = &["standard", "detail", "summary", "markdown"];

pub fn validate_target_ip(value: &str) -> Result<()> {
    let ip: IpAddr = value
        .parse()
        .map_err(|_| Error::new("error.input", "target_ip"))?;
    let valid = match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified(),
        IpAddr::V6(v6) => !value.contains('.') && !v6.is_loopback() && !v6.is_unspecified(),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new("error.input", "target_ip"))
    }
}

pub fn arguments(report_mode: &str, share_link: &str, target_ip: &str) -> Result<Vec<String>> {
    // CqShell confirms the external run beforehand; -y only skips the upstream script's
    // duplicate "Press Enter to continue" prompt (the script is read-only and never installs packages).
    let mut args = vec!["-y".into()];
    match report_mode {
        "standard" => (),
        "detail" => args.push("-d".into()),
        "summary" => args.push("-s".into()),
        "markdown" => args.push("-m".into()),
        _ => return Err(Error::new("error.input", "report_mode")),
    }
    match share_link {
        "off" => args.push("--no-share".into()),
        "on" => args.push("-r".into()),
        _ => return Err(Error::new("error.input", "share_link")),
    }
    if !target_ip.is_empty() {
        validate_target_ip(target_ip)?;
        args.push(target_ip.into());
    }
    Ok(args)
}

pub fn run(ctx: &Context, report_mode: &str, share_link: &str, target_ip: &str) -> Result<()> {
    let args = arguments(report_mode, share_link, target_ip)?;
    ctx.require(&["curl", "sh"])?;
    ctx.confirm(
        "confirm.ai_check",
        format!("{SCRIPT_URL}\n{PROJECT}"),
        "RUN",
    )?;
    let work = tempfile::tempdir()?;
    let script = resolve_script_path(&work.path().join("ipforai-check.sh"))?;
    ctx.download(SCRIPT_URL, &script, 300)?;
    verify_script(ctx, &script)?;
    let script_arg = path(&script)?;
    let mut argv = vec![script_arg];
    argv.extend(args.iter().map(String::as_str));
    ctx.passthrough("sh", &argv)
}

fn resolve_script_path(script: &Path) -> Result<PathBuf> {
    if script.is_absolute() {
        Ok(script.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(script))
    }
}

fn verify_script(ctx: &Context, script: &Path) -> Result<()> {
    if fs::metadata(script)?.len() == 0 {
        return Err(Error::new("error.data", SCRIPT_URL));
    }
    let resolved = resolve_script_path(script)?;
    ctx.run("sh", &["-n", path(&resolved)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_disables_online_share_and_skips_duplicate_tty_prompt() {
        assert_eq!(
            arguments("standard", "off", "").unwrap(),
            ["-y", "--no-share"]
        );
        for mode in REPORT_MODES {
            let args = arguments(mode, "off", "").unwrap();
            assert!(args.iter().any(|arg| arg == "--no-share"));
            assert!(!args.iter().any(|arg| arg == "-r" || arg == "-j"));
        }
    }

    #[test]
    fn report_modes_share_toggle_and_target_ip_are_validated() {
        assert_eq!(
            arguments("detail", "on", "8.8.8.8").unwrap(),
            ["-y", "-d", "-r", "8.8.8.8"]
        );
        assert_eq!(
            arguments("summary", "off", "2606:4700:4700::1111").unwrap(),
            ["-y", "-s", "--no-share", "2606:4700:4700::1111"]
        );
        assert_eq!(arguments("markdown", "on", "").unwrap(), ["-y", "-m", "-r"]);

        for bad_mode in ["json", "-j", "full", "detail;id", ""] {
            assert_eq!(
                arguments(bad_mode, "off", "").unwrap_err().key,
                "error.input"
            );
        }
        for bad_share in ["auto", "yes", "-r", ""] {
            assert_eq!(
                arguments("standard", bad_share, "").unwrap_err().key,
                "error.input"
            );
        }
        for bad_ip in [
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "::",
            "::ffff:8.8.8.8",
            "-j",
            "--help",
            "8.8.8.8;id",
            " 8.8.8.8",
            "ipforai.cc",
        ] {
            assert_eq!(
                arguments("standard", "off", bad_ip).unwrap_err().key,
                "error.input",
                "{bad_ip}"
            );
        }
    }

    #[test]
    fn action_schema_is_read_only_interactive_and_exposes_defaults() {
        let spec = crate::actions::find("test.ai").unwrap();
        assert_eq!(spec.group, "tests");
        assert!(spec.external);
        assert!(!spec.root);
        assert!(!spec.changes);
        let defaults = spec.parameters(&crate::actions::Parameters::new()).unwrap();
        assert_eq!(defaults["report_mode"], "standard");
        assert_eq!(defaults["share_link"], "off");
        assert_eq!(defaults["target_ip"], "");
    }

    #[cfg(unix)]
    #[test]
    fn sh_parser_rejects_empty_and_malformed_scripts_without_executing() {
        use crate::{config::Paths, i18n::I18n};
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("ipforai-check.sh");
        let marker = temp.path().join("executed");
        let ctx = Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some("en-US"), None).unwrap(),
            true,
        );

        fs::write(&script, b"").unwrap();
        assert_eq!(verify_script(&ctx, &script).unwrap_err().key, "error.data");

        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        verify_script(&ctx, &script).unwrap();
        assert!(!marker.exists());

        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\nif\n", marker.display()),
        )
        .unwrap();
        assert_eq!(
            verify_script(&ctx, &script).unwrap_err().key,
            "error.command"
        );
        assert!(!marker.exists());
    }
}
