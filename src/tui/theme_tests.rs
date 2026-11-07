use super::*;
use crate::config::Preferences;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn style_previews_save_cancel_and_restore_after_failed_saves() {
    for width in [80, 120] {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        Preferences {
            language: Some("zh-CN".into()),
            detect_background: Some(false),
            ..Preferences::default()
        }
        .write(&ctx.paths.preferences)
        .unwrap();
        let mut app = App::new(ctx);
        app.viewport = Rect::new(0, 0, width, 36);
        let preferences = Preferences::read(&app.ctx.paths.preferences, false).unwrap();
        app.configure_theme(
            Some("classic"),
            None,
            Background::Detected((11, 17, 25)),
            &preferences,
        );
        let previous = app.theme.active.clone();
        app.key(key(KeyCode::Char('t'))).unwrap();
        let geometry = view::StyleGeometry::new(app.viewport, &app);
        assert_eq!(
            geometry.panel,
            view::Regions::new(app.viewport, &app).details
        );
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(app.theme.active.id, "phosphor");
        app.key(key(KeyCode::Esc)).unwrap();
        assert_eq!(app.theme.active, previous);
        app.open_style();
        app.preview_style(4);
        assert_eq!(app.theme.active.id, "nord");
        let path = app.ctx.paths.preferences.clone();
        app.ctx.paths.preferences = temp.path().to_owned();
        app.key(key(KeyCode::Enter)).unwrap();
        assert_eq!(app.theme.active, previous);
        assert!(app.style_panel.is_some() && app.notice.is_some());
        app.key(key(KeyCode::Esc)).unwrap();
        app.ctx.paths.preferences = path;
        app.key(key(KeyCode::Enter)).unwrap();
        let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
        assert_eq!(saved.theme.as_deref(), Some("nord"));
        assert_eq!(saved.language.as_deref(), Some("zh-CN"));
        assert_eq!(saved.detect_background, Some(false));
        assert_eq!(app.theme.active.id, "nord");
        assert!(app.style_panel.is_none());
    }
}

#[test]
fn invalid_custom_themes_remain_visible_and_do_not_replace_the_preview() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(crate::tests::context(temp.path()));
    std::fs::create_dir_all(&app.ctx.paths.themes).unwrap();
    std::fs::write(app.ctx.paths.themes.join("bad.toml"), "name = 4").unwrap();
    app.configure_theme(
        Some("bad"),
        None,
        Background::Disabled,
        &Preferences::default(),
    );
    assert_eq!(app.theme.active.id, "aurora");
    assert!(app.theme.error.as_ref().unwrap().contains("bad.toml"));
    assert!(app.toast.as_ref().unwrap().text.contains("bad.toml"));
    app.open_style();
    let previous = app.theme.active.clone();
    app.key(key(KeyCode::End)).unwrap();
    assert_eq!(app.theme.active, previous);
    app.key(key(KeyCode::Enter)).unwrap();
    assert!(app.style_panel.is_some());
    assert!(!app.ctx.paths.preferences.exists());
    for code in [
        KeyCode::Char('r'),
        KeyCode::Char('l'),
        KeyCode::Char('/'),
        KeyCode::Char('3'),
        KeyCode::Tab,
        KeyCode::Char(' '),
    ] {
        assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
    }
    assert!(app.command_palette.is_none() && app.marked.is_empty());
    app.key(key(KeyCode::Char('?'))).unwrap();
    assert!(app.help);
    app.key(key(KeyCode::Esc)).unwrap();
    assert!(app.style_panel.is_some());
}

#[test]
fn theme_directory_errors_reach_the_startup_toast() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(crate::tests::context(temp.path()));
    std::fs::write(&app.ctx.paths.themes, "not a directory").unwrap();
    let customs = theme::file::load_dir(&app.ctx.paths.themes);
    let expected = customs[0].result.as_ref().unwrap_err().to_string();

    app.configure_theme(
        Some("custom"),
        None,
        Background::Disabled,
        &Preferences::default(),
    );
    assert_eq!(app.theme.active.id, "aurora");
    assert_eq!(app.theme.error.as_deref(), Some(expected.as_str()));
    assert!(app.toast.as_ref().unwrap().text.contains(&expected));
}

#[test]
fn reopening_style_refreshes_the_definition_cache_and_preserves_invalid_entries() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(crate::tests::context(temp.path()));
    std::fs::create_dir_all(&app.ctx.paths.themes).unwrap();
    let path = app.ctx.paths.themes.join("custom.toml");
    let mut custom = builtin::nord();
    custom.id = "custom".into();
    custom.name = theme::ThemeName::Custom("Original".into());
    std::fs::write(&path, theme::file::print(&custom)).unwrap();
    app.configure_theme(
        Some("custom"),
        None,
        Background::Disabled,
        &Preferences::default(),
    );
    let previous = app.theme.active.clone();

    custom.name = theme::ThemeName::Custom("Updated".into());
    custom.patterns.gradient = !custom.patterns.gradient;
    std::fs::write(&path, theme::file::print(&custom)).unwrap();
    app.open_style();
    assert_eq!(
        app.theme
            .definitions
            .iter()
            .find(|definition| definition.id == "custom"),
        Some(&custom)
    );
    app.key(key(KeyCode::End)).unwrap();
    assert_eq!(
        app.theme.active,
        theme::resolve(&custom, &Background::Disabled)
    );
    app.key(key(KeyCode::Esc)).unwrap();
    assert_eq!(app.theme.active, previous);

    std::fs::write(&path, "name = 4").unwrap();
    app.open_style();
    assert!(
        app.theme
            .definitions
            .iter()
            .all(|definition| definition.id != "custom")
    );
    let panel = app.style_panel.as_ref().unwrap();
    assert!(
        panel
            .entries
            .iter()
            .any(|entry| entry.id == "custom" && entry.definition.is_err())
    );
    assert_eq!(
        panel.entries.iter().filter(|entry| entry.builtin).count(),
        builtin::all().len()
    );
    app.key(key(KeyCode::Esc)).unwrap();

    std::fs::remove_file(&path).unwrap();
    app.open_style();
    assert!(
        app.style_panel
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .all(|entry| entry.id != "custom")
    );
}

#[test]
fn style_mouse_uses_its_rows_and_ignores_the_background() {
    let mut app = snapshot::fixture_app("en-US");
    app.open_style();
    let area = Rect::new(0, 0, 120, 36);
    let geometry = view::StyleGeometry::new(area, &app);
    let point = (geometry.panel.y..geometry.panel.bottom())
        .flat_map(|row| {
            (geometry.panel.x..geometry.panel.right()).map(move |column| Position::new(column, row))
        })
        .find(|point| geometry.row_at(*point) == Some(3))
        .unwrap();
    let event = |kind, point: Position| MouseEvent {
        kind,
        column: point.x,
        row: point.y,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(event(MouseEventKind::Down(MouseButton::Left), point), area)
        .unwrap();
    assert_eq!(app.theme.active.id, "neon");
    app.mouse(event(MouseEventKind::ScrollDown, Position::new(0, 0)), area)
        .unwrap();
    app.mouse(
        event(MouseEventKind::Down(MouseButton::Left), Position::new(0, 0)),
        area,
    )
    .unwrap();
    assert_eq!(app.theme.active.id, "neon");
    app.mouse(event(MouseEventKind::ScrollDown, point), area)
        .unwrap();
    assert_eq!(app.theme.active.id, "nord");
}

#[test]
fn startup_reuses_preferences_but_theme_saves_read_the_latest_values() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(crate::tests::context(temp.path()));
    let preferences = Preferences {
        theme: Some("nord".into()),
        category_order: vec!["kernel".into()],
        detect_background: Some(false),
        ..Preferences::default()
    };
    std::fs::write(&app.ctx.paths.preferences, "invalid TOML").unwrap();
    app.configure_theme(None, None, Background::Disabled, &preferences);
    app.set_category_order(category_order_from_ids(&preferences.category_order).unwrap());
    assert_eq!(app.theme.active.id, "nord");
    assert_eq!(app.theme.saved_id.as_deref(), Some("nord"));
    assert_eq!(GROUPS[app.group], "kernel");

    app.open_style();
    app.key(key(KeyCode::Home)).unwrap();
    let latest = Preferences {
        language: Some("zh-CN".into()),
        category_order: vec!["network".into()],
        detect_background: Some(true),
        ..Preferences::default()
    };
    latest.write(&app.ctx.paths.preferences).unwrap();
    app.key(key(KeyCode::Enter)).unwrap();
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(
        saved,
        Preferences {
            theme: Some("aurora".into()),
            ..latest
        }
    );
}

#[test]
fn dialog_fade_uses_the_frame_clock_without_delaying_input() {
    assert_eq!(motion::dialog_opacity(Duration::ZERO), 0.0);
    assert_eq!(motion::dialog_opacity(Duration::from_millis(60)), 0.5);
    assert_eq!(motion::dialog_opacity(Duration::from_millis(120)), 1.0);
    let mut app = snapshot::fixture_app("en-US");
    app.preview_theme("neon", Background::Disabled).unwrap();
    app.help = true;
    let now = app.now;
    app.tick(now);
    assert!(app.dialog_fading());
    assert_eq!(app.frame_interval(), motion::ANIMATION_FRAME);
    app.key(key(KeyCode::Esc)).unwrap();
    assert!(!app.help);
    app.help = true;
    app.tick(now + motion::DIALOG_FADE);
    assert!(!app.dialog_fading());
    app.preview_theme("plain", Background::Disabled).unwrap();
    app.dialog_opened = Some(app.now);
    assert!(!app.dialog_fading());
}

#[test]
fn every_builtin_keeps_all_categories_at_minimum_size() {
    for definition in builtin::all() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = snapshot::fixture_app(locale);
            app.theme.active = theme::resolve(&definition, &Background::Detected((11, 17, 25)));
            let area = Rect::new(0, 0, 80, 24);
            let regions = view::Regions::new(area, &app);
            assert_eq!(regions.category_offset, 0);
            assert!(regions.category_rows.height >= 13, "{}", definition.id);
            snapshot::render(&app, 80, 24);
        }
    }
}
