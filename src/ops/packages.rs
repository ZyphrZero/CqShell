use crate::{error::Result, runtime::Context};

// Captured package-manager commands cannot answer a second terminal's prompts.
// Preserve locally edited files; only report services that may need a restart.
const OPTIONS: &[&str] = &[
    "-o",
    "Dpkg::Use-Pty=0",
    "-o",
    "Dpkg::Progress-Fancy=0",
    "-o",
    "APT::Color=0",
    "-o",
    "Dpkg::Options::=--force-confdef",
    "-o",
    "Dpkg::Options::=--force-confold",
];

fn configured(ctx: &Context) -> Context {
    let mut ctx = ctx.clone();
    for (name, value) in [
        ("DEBIAN_FRONTEND", "noninteractive"),
        ("UCF_FORCE_CONFFOLD", "1"),
        ("UCF_FORCE_CONFFNEW", ""),
        ("NEEDRESTART_MODE", "l"),
        ("APT_LISTCHANGES_FRONTEND", "none"),
    ] {
        ctx.environment.insert(name.into(), value.into());
    }
    ctx
}

fn arguments<'a>(args: &[&'a str]) -> Vec<&'a str> {
    args.iter().chain(OPTIONS).copied().collect()
}

pub fn run(ctx: &Context, args: &[&str]) -> Result<()> {
    let result = configured(ctx).run("apt-get", &arguments(args));
    if result.is_err() && !matches!(args.first(), Some(&"update" | &"clean")) {
        ctx.message("notice.package_audit", "dpkg --audit");
    }
    result
}

pub fn output(ctx: &Context, args: &[&str]) -> Result<String> {
    configured(ctx).output("apt-get", &arguments(args))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    #[ignore = "real ucf requires root; writes only isolated temporary files"]
    fn real_ucf_preserves_edited_configuration_without_a_dialog() {
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "ucf silently switches to dry-run without root"
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let state = root.join("ucf-state");
        fs::create_dir(&state).unwrap();
        let debconf = root.join("debconf.conf");
        fs::write(
            &debconf,
            "Config: config\nTemplates: templates\n\nName: config\nDriver: File\nFilename: "
                .to_owned()
                + &root.join("config.dat").display().to_string()
                + "\n\nName: templates\nDriver: File\nFilename: "
                + &root.join("templates.dat").display().to_string()
                + "\n",
        )
        .unwrap();
        let source = root.join("packaged.conf");
        let installed = root.join("installed.conf");
        let mut ctx = crate::tests::context(root);
        ctx.interactive = false;
        ctx.environment
            .insert("DEBCONF_SYSTEMRC".into(), debconf.display().to_string());
        let ctx = configured(&ctx);
        let ucf = std::env::var("CQS_TEST_UCF").unwrap_or_else(|_| "/usr/bin/ucf".into());
        let args = [
            "--state-dir",
            crate::runtime::path(&state).unwrap(),
            crate::runtime::path(&source).unwrap(),
            crate::runtime::path(&installed).unwrap(),
        ];
        fs::write(&source, "package-v1\n").unwrap();
        ctx.run(&ucf, &args).unwrap();
        assert_eq!(fs::read_to_string(&installed).unwrap(), "package-v1\n");
        fs::write(&installed, "locally edited SSH configuration\n").unwrap();
        fs::write(&source, "package-v2\n").unwrap();
        ctx.run(&ucf, &args).unwrap();
        assert_eq!(
            fs::read_to_string(&installed).unwrap(),
            "locally edited SSH configuration\n"
        );
        assert!(state.join("hashfile").is_file());
    }
}
