use std::process::Command;

fn command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cqs"));
    command
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env_remove("CQS_CONFIG")
        .env_remove("CQS_LOCALE_DIR");
    command
}

#[test]
fn themes_list_valid_invalid_and_localized_entries_without_failing() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("config/cqs/themes");
    std::fs::create_dir_all(&directory).unwrap();
    let mut definition = cqs::theme::builtin::obsidian();
    definition.id = "my-slate".into();
    definition.name = cqs::theme::ThemeName::Custom("My Slate".into());
    std::fs::write(
        directory.join("my-slate.toml"),
        cqs::theme::file::print(&definition),
    )
    .unwrap();
    std::fs::write(directory.join("bad.toml"), "invalid").unwrap();
    for (locale, label) in [("en-US", "Obsidian"), ("zh-CN", "曜石")] {
        let output = command(temp.path())
            .args(["--lang", locale, "themes"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(&format!("obsidian\t{label}")) && text.contains("my-slate\tMy Slate"),
            "{text}"
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("bad.toml")
        );
        let output = command(temp.path())
            .args(["--lang", locale, "themes", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0]["id"], "auto");
        assert_eq!(rows[0]["default_style"], "rounded");
        assert_eq!(rows[1]["id"], "obsidian");
        assert_eq!(rows[1]["default_style"], "bento");
        assert_eq!(rows[1]["name"], label);
        assert_eq!(rows[8]["id"], "bad");
        assert_eq!(rows[8]["valid"], false);
        assert!(rows[8]["error"].as_str().unwrap().contains("bad.toml"));
        assert_eq!(rows[9]["valid"], true);
        assert_eq!(rows[9]["default_style"], "bento");
        assert_eq!(rows[0]["path"], serde_json::Value::Null);
    }
}

#[test]
fn styles_subcommand_lists_auto_and_all_eight_builtin_styles() {
    let temp = tempfile::tempdir().unwrap();
    for (locale, auto_label, hud_label) in [
        ("en-US", "Follow palette", "Cyber HUD"),
        ("zh-CN", "跟随配色", "精密角框"),
    ] {
        let output = command(temp.path())
            .args(["--lang", locale, "styles"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains(&format!("auto\t{auto_label}"))
                && text.contains(&format!("hud\t{hud_label}")),
            "{text}"
        );

        let output = command(temp.path())
            .args(["--lang", locale, "styles", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[0]["id"], "auto");
        assert_eq!(rows[0]["name"], auto_label);
        assert!(
            rows.iter()
                .any(|r| r["id"] == "hud" && r["name"] == hud_label)
        );
    }
}

#[test]
fn theme_flag_is_ignored_outside_tui_and_json_rejection_mentions_themes() {
    let temp = tempfile::tempdir().unwrap();
    let output = command(temp.path())
        .args(["--theme", "missing", "--style", "hud", "languages"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = command(temp.path())
        .args(["--lang", "en-US", "--json", "proxy-env", "off"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("list/languages/themes/styles")
    );
}
