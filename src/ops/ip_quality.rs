//! xykt/IPQuality integration. The upstream script retains ownership of its report format.
use crate::{
    error::{Error, Result},
    runtime::{Context, path, sha256},
};
use std::path::Path;

pub const PROJECT: &str = "https://github.com/xykt/IPQuality";
// Pin the entry script, not the live databases/reference files it queries.
// Update the revision and digest together after checking the upstream option contract.
pub const SCRIPT_URL: &str = "https://raw.githubusercontent.com/xykt/IPQuality/2384a67c756eb35231f5982b34731e522be3653e/ip.sh";
pub const SCRIPT_SHA256: &str = "b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf";

pub fn arguments(locale: &str, family: &str, full_ip: &str) -> Result<Vec<String>> {
    let mut args = vec![
        "-p".into(),
        "-l".into(),
        if locale == "zh-CN" {
            "cn".into()
        } else {
            "en".into()
        },
    ];
    match family {
        "dual" => (),
        "ipv4" => args.push("-4".into()),
        "ipv6" => args.push("-6".into()),
        _ => return Err(Error::new("error.input", "ip_family")),
    }
    match full_ip {
        "off" => (),
        "on" => args.push("-f".into()),
        _ => return Err(Error::new("error.input", "full_ip")),
    }
    // Do not forward CqShell --yes as upstream -y, or bypass dependency checks with -n.
    Ok(args)
}

pub fn run(ctx: &Context, family: &str, full_ip: &str) -> Result<()> {
    let args = arguments(ctx.i18n.locale(), family, full_ip)?;
    ctx.require(&["curl", "bash"])?;
    ctx.confirm("confirm.ip_quality", PROJECT, "RUN")?;
    let work = tempfile::tempdir()?;
    let script = work.path().join("ipquality.sh");
    ctx.download(SCRIPT_URL, &script, 300)?;
    verify_script(ctx, &script, SCRIPT_SHA256)?;
    let mut argv = vec!["--", path(&script)?];
    argv.extend(args.iter().map(String::as_str));
    ctx.passthrough("bash", &argv)
}

fn verify_script(ctx: &Context, script: &Path, expected: &str) -> Result<()> {
    if sha256(script)? != expected {
        return Err(Error::new("error.digest", "xykt/IPQuality/ip.sh"));
    }
    ctx.run("bash", &["-n", "--", path(script)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_mode_is_private_dual_stack_and_follows_ui_language() {
        assert_eq!(
            arguments("zh-CN", "dual", "off").unwrap(),
            ["-p", "-l", "cn"]
        );
        assert_eq!(
            arguments("en-US", "dual", "off").unwrap(),
            ["-p", "-l", "en"]
        );
        for language in ["zh-CN", "en-US"] {
            for family in ["dual", "ipv4", "ipv6"] {
                let args = arguments(language, family, "off").unwrap();
                for forbidden in ["-y", "-n", "-f", "-M", "-x"] {
                    assert!(!args.iter().any(|arg| arg == forbidden));
                }
            }
        }
    }
    #[test]
    fn address_family_and_full_ip_are_explicit_and_validated() {
        assert_eq!(
            arguments("en-US", "ipv4", "on").unwrap(),
            ["-p", "-l", "en", "-4", "-f"]
        );
        assert_eq!(
            arguments("zh-CN", "ipv6", "off").unwrap(),
            ["-p", "-l", "cn", "-6"]
        );
        for invalid in ["both", "ipv4; id", "-x", ""] {
            assert!(arguments("zh-CN", invalid, "off").is_err());
        }
        assert!(arguments("zh-CN", "dual", "yes").is_err());
    }
    #[test]
    fn action_schema_selects_this_provider_and_retains_terminal_handoff() {
        let spec = crate::actions::find("test.ip").unwrap();
        assert!(spec.external);
        assert!(!spec.root);
        let defaults = spec.parameters(&crate::actions::Parameters::new()).unwrap();
        assert_eq!(defaults["ip_family"], "dual");
        assert_eq!(defaults["full_ip"], "off");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn real_digest_and_bash_parser_reject_bad_downloads_without_execution() {
        use crate::{config::Paths, i18n::I18n};
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("script.sh");
        let marker = temp.path().join("executed");
        let ctx = Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some("en-US"), None).unwrap(),
            true,
        );
        std::fs::write(&script, format!("touch '{}'\n", marker.display())).unwrap();
        assert_eq!(
            verify_script(&ctx, &script, SCRIPT_SHA256).unwrap_err().key,
            "error.digest"
        );
        verify_script(&ctx, &script, &sha256(&script).unwrap()).unwrap();
        assert!(!marker.exists());
        std::fs::write(&script, format!("touch '{}'\nif\n", marker.display())).unwrap();
        assert_eq!(
            verify_script(&ctx, &script, &sha256(&script).unwrap())
                .unwrap_err()
                .key,
            "error.command"
        );
        assert!(!marker.exists());
    }
}
