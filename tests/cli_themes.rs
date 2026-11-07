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
    let mut definition = cqs::theme::builtin::nord();
    definition.id = "my-nord".into();
    definition.name = cqs::theme::ThemeName::Custom("My Nord".into());
    std::fs::write(
        directory.join("my-nord.toml"),
        cqs::theme::file::print(&definition),
    )
    .unwrap();
    std::fs::write(directory.join("bad.toml"), "invalid").unwrap();
    for (locale, label) in [("en-US", "Classic"), ("zh-CN", "经典")] {
        let output = command(temp.path())
            .args(["--lang", locale, "themes"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!("classic\t{label}")) && text.contains("my-nord\tMy Nord"));
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
        assert_eq!(rows[0]["id"], "aurora");
        assert_eq!(rows[1]["name"], label);
        assert_eq!(rows[8]["id"], "bad");
        assert_eq!(rows[8]["valid"], false);
        assert!(rows[8]["error"].as_str().unwrap().contains("bad.toml"));
        assert_eq!(rows[9]["valid"], true);
        assert_eq!(rows[0]["path"], serde_json::Value::Null);
    }
}

#[test]
fn theme_flag_is_ignored_outside_tui_and_json_rejection_mentions_themes() {
    let temp = tempfile::tempdir().unwrap();
    let output = command(temp.path())
        .args(["--theme", "missing", "languages"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = command(temp.path())
        .args(["--lang", "en-US", "--json", "run", "system.info"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("list/languages/themes")
    );
}
