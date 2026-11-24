//! Comprehensive state machine, interaction, mouse hit-testing, theme persistence, and
//! multi-size render tests for the redesigned TUI.

use super::{
    layout::{self, Breakpoint, LayoutRegions},
    snapshot::fixture_snapshot,
    state::{
        App, Confirmation, Effect, GROUPS, NavTarget, OPTIMIZATION_GROUP, category_order_from_ids,
        default_category_order,
    },
};
use crate::{
    config::Preferences,
    i18n::I18n,
    theme::{Background, builtin},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{sync::mpsc, time::Instant};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn click(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn test_app(locale: &str) -> (tempfile::TempDir, App) {
    let temp = tempfile::tempdir().unwrap();
    let mut ctx = crate::tests::context(temp.path());
    ctx.i18n = I18n::load(Some(locale), None).unwrap();
    let mut app = App::new(ctx);
    let now = Instant::now();
    app.root = true;
    app.platform_error = None;
    app.snapshot = fixture_snapshot(now);
    app.tick(now);
    (temp, app)
}

#[test]
fn all_themes_and_breakpoints_render_without_panicking_in_both_locales() {
    let sizes = [
        (30, 10),
        (40, 16),
        (48, 20),
        (59, 24),
        (80, 18),
        (80, 24),
        (100, 30),
        (120, 36),
        (160, 44),
    ];
    for locale in ["zh-CN", "en-US"] {
        let (_temp, mut app) = test_app(locale);
        for def in builtin::all() {
            app.preview_theme(&def.id, Background::Detected((0x0d, 0x11, 0x17)))
                .unwrap();
            for transparent in [false, true] {
                if app.theme.transparent_bg != transparent {
                    app.toggle_transparent_bg(false);
                }
                for nav in [
                    NavTarget::Overview,
                    NavTarget::Optimization,
                    NavTarget::Group(0),
                ] {
                    app.goto_nav(nav);
                    for (w, h) in sizes {
                        app.viewport = Rect::new(0, 0, w, h);
                        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                        term.draw(|f| app.draw(f)).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn navigation_shortcuts_sidebar_toggle_and_category_reorder_work() {
    let (temp, mut app) = test_app("en-US");
    assert_eq!(app.nav, NavTarget::Overview);

    // Jump to Optimization via O
    app.handle_key(key(KeyCode::Char('o'))).unwrap();
    assert_eq!(app.nav, NavTarget::Optimization);

    // Jump to first numbered category via '1'
    app.handle_key(key(KeyCode::Char('1'))).unwrap();
    assert_eq!(app.nav, NavTarget::Group(app.category_order[0]));

    // Jump back to Overview via H
    app.handle_key(key(KeyCode::Char('h'))).unwrap();
    assert_eq!(app.nav, NavTarget::Overview);

    // Overview quick cards move in 2x2 grid
    app.handle_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.quick_cursor, 1);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.quick_cursor, 3);

    // Sidebar collapse on Wide persists preference
    app.viewport = Rect::new(0, 0, 120, 36);
    assert!(!app.sidebar_collapsed);
    app.handle_key(key(KeyCode::Char('['))).unwrap();
    assert!(app.sidebar_collapsed);
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.sidebar_collapsed, Some(true));

    // Sidebar toggle on Compact opens overlay drawer instead
    app.viewport = Rect::new(0, 0, 80, 24);
    assert_eq!(Breakpoint::of(80), Breakpoint::Compact);
    app.handle_key(key(KeyCode::Char('['))).unwrap();
    assert!(app.sidebar_overlay);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.sidebar_overlay);

    // Category reorder with Alt+Down persists stable IDs
    app.handle_key(key(KeyCode::Char('1'))).unwrap();
    let first = app.category_order[0];
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.category_order[1], first);
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.category_order[1], GROUPS[first]);
    drop(temp);
}

#[test]
fn language_switch_persists_without_clobbering_other_preferences() {
    let (_temp, mut app) = test_app("en-US");
    Preferences {
        language: Some("en-US".into()),
        theme: Some("tokyo-night".into()),
        transparent_bg: Some(true),
        ..Default::default()
    }
    .write(&app.ctx.paths.preferences)
    .unwrap();

    app.handle_key(key(KeyCode::Char('l'))).unwrap();
    assert_eq!(app.ctx.i18n.locale(), "zh-CN");
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.language.as_deref(), Some("zh-CN"));
    assert_eq!(saved.theme.as_deref(), Some("tokyo-night"));
    assert_eq!(saved.transparent_bg, Some(true));
}

#[test]
fn theme_drawer_previews_reverts_on_esc_and_persists_all_toggles_on_enter() {
    let (_temp, mut app) = test_app("zh-CN");
    let initial_id = app.theme.active.id.clone();

    // Open drawer, preview another theme, toggle transparent + motion, then cancel with Esc
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    assert!(app.theme_drawer.is_some());
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    app.handle_key(key(KeyCode::Char('m'))).unwrap();
    assert!(app.theme.transparent_bg);
    assert!(app.theme.reduced_motion);
    assert_ne!(app.theme.active.id, initial_id);

    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.theme_drawer.is_none());
    assert_eq!(app.theme.active.id, initial_id);
    assert!(!app.theme.transparent_bg);
    assert!(!app.theme.reduced_motion);

    // Open again, pick Tokyo Night (index 3), toggle both flags, save with Enter
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.preview_theme_cursor(3);
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    app.handle_key(key(KeyCode::Char('m'))).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.theme_drawer.is_none());
    assert_eq!(app.theme.active.id, "tokyo-night");
    assert!(app.theme.transparent_bg);
    assert!(app.theme.reduced_motion);

    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.theme.as_deref(), Some("tokyo-night"));
    assert_eq!(saved.style, None);
    assert_eq!(saved.transparent_bg, Some(true));
    assert_eq!(saved.reduced_motion, Some(true));
}

#[test]
fn theme_drawer_dual_tabs_mix_palette_and_style_persist_and_revert() {
    use crate::theme::{StyleSelection, UiStyle};
    let (_temp, mut app) = test_app("zh-CN");
    // Default theme is auto -> Rounded
    assert_eq!(app.theme.active.id, "auto");
    assert_eq!(app.theme.active.style, UiStyle::Rounded);

    // Open drawer, pick Obsidian (index 1 -> default Bento), then switch to Style tab
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.preview_theme_cursor(1);
    assert_eq!(app.theme.active.id, "obsidian");
    assert_eq!(app.theme.active.style, UiStyle::Bento);
    assert!(!app.theme.active.flags.framed);

    // Transparent bg on borderless Bento automatically enables crisp framing
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    assert!(app.theme.transparent_bg);
    assert!(app.theme.active.flags.framed);
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    assert!(!app.theme.active.flags.framed);

    // Switch to Style tab via Tab and lock HUD (index 4 in STYLE_CHOICES)
    app.handle_key(key(KeyCode::Tab)).unwrap();
    app.preview_theme_cursor(4);
    assert_eq!(app.theme.active.id, "obsidian");
    assert_eq!(app.theme.active.style, UiStyle::Hud);
    assert!(app.theme.active.flags.framed);

    // Esc reverts both palette and style selection
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert_eq!(app.theme.active.id, "auto");
    assert_eq!(app.theme.active.style, UiStyle::Rounded);
    assert_eq!(app.theme.style_selection, StyleSelection::Auto);

    // Open again, mix Rose Pine (index 5) + Bento (index 2 on Style tab), save with Enter
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.preview_theme_cursor(5);
    assert_eq!(app.theme.active.style, UiStyle::Atelier);
    app.handle_key(key(KeyCode::Right)).unwrap();
    app.preview_theme_cursor(2); // Bento
    assert_eq!(app.theme.active.style, UiStyle::Bento);
    app.handle_key(key(KeyCode::Enter)).unwrap();

    assert_eq!(app.theme.active.id, "rose-pine");
    assert_eq!(app.theme.active.style, UiStyle::Bento);
    assert_eq!(
        app.theme.style_selection,
        StyleSelection::Fixed(UiStyle::Bento)
    );
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.theme.as_deref(), Some("rose-pine"));
    assert_eq!(saved.style.as_deref(), Some("bento"));

    // Reopen, switch Style back to Auto (index 0) and save -> clears style override in config
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.handle_key(key(KeyCode::Tab)).unwrap();
    app.preview_theme_cursor(0);
    assert_eq!(app.theme.active.style, UiStyle::Atelier);
    app.handle_key(key(KeyCode::Enter)).unwrap();
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.style, None);
}

#[test]
fn palette_morph_smoothly_interpolates_and_completes_when_motion_enabled() {
    use std::time::Duration;
    let (_temp, mut app) = test_app("zh-CN");
    app.theme.reduced_motion = false;
    let start = app.now;
    let initial_canvas = app.theme.active.palette.canvas;

    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.preview_theme_cursor(2); // daylight (light theme)
    assert!(app.palette_morph.is_some());
    // At t=0 it starts from the previous palette
    assert_eq!(app.theme.active.palette.canvas, initial_canvas);

    // Halfway through morph, canvas is strictly between dark and light
    app.tick(start + Duration::from_millis(80));
    assert!(app.palette_morph.is_some());
    assert_ne!(app.theme.active.palette.canvas, initial_canvas);
    assert_ne!(
        app.theme.active.palette.canvas,
        crate::theme::builtin::DAYLIGHT.canvas
    );

    // After morph duration, it settles cleanly on Daylight's exact palette
    app.tick(start + Duration::from_millis(220));
    assert!(app.palette_morph.is_none());
    assert_eq!(
        app.theme.active.palette.canvas,
        crate::theme::builtin::DAYLIGHT.canvas
    );
}

#[test]
fn every_ui_style_renders_overview_and_workbench_without_panics() {
    use crate::theme::{StyleSelection, UiStyle};
    let (_temp, mut app) = test_app("zh-CN");
    for style in UiStyle::ALL {
        app.preview_theme_with_style(
            "obsidian",
            StyleSelection::Fixed(style),
            Background::Detected((0x0d, 0x11, 0x17)),
        )
        .unwrap();
        for &(w, h) in &[(120, 36), (80, 24), (59, 24), (40, 16)] {
            app.viewport = Rect::new(0, 0, w, h);
            app.goto_nav(NavTarget::Overview);
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buf = terminal.backend().buffer();
            let text: String = buf.content.iter().map(|c| c.symbol()).collect();
            assert!(text.contains("CPU"), "style {style:?} {w}x{h} missing CPU");
            app.goto_nav(NavTarget::Group(0));
            terminal.draw(|frame| app.draw(frame)).unwrap();
        }
    }
}

#[test]
fn command_palette_supports_ctrl_k_commands_and_tab_jump() {
    let (_temp, mut app) = test_app("zh-CN");
    app.handle_key(ctrl('k')).unwrap();
    assert!(app.command_palette.is_some());

    // Search a workbench command with `>`
    for ch in "> 主题".chars() {
        app.handle_key(key(KeyCode::Char(ch))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.command_palette.is_none());
    assert!(app.theme_drawer.is_some());
    app.handle_key(key(KeyCode::Esc)).unwrap();

    // Search an action and jump to it with Tab
    app.handle_key(key(KeyCode::Char('/'))).unwrap();
    for ch in "bbr.enable".chars() {
        app.handle_key(key(KeyCode::Char(ch))).unwrap();
    }
    app.handle_key(key(KeyCode::Tab)).unwrap();
    assert!(app.command_palette.is_none());
    assert_eq!(
        app.nav,
        NavTarget::Group(GROUPS.iter().position(|g| *g == "network").unwrap())
    );
}

#[test]
fn mouse_clicks_route_to_sidebar_overview_cards_and_modals_safely() {
    let (_temp, mut app) = test_app("zh-CN");
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    let regions = LayoutRegions::compute(area, &app);

    // Click Optimization item in docked sidebar
    let side = regions.sidebar.as_ref().unwrap();
    let (opt_rect, _) = side
        .item_rects
        .iter()
        .find(|(_, target)| *target == NavTarget::Optimization)
        .unwrap();
    app.handle_mouse(click(opt_rect.x + 2, opt_rect.y), area)
        .unwrap();
    assert_eq!(app.nav, NavTarget::Optimization);

    // Click topbar language toggle
    let lang_rect = regions.topbar_hits.lang_btn.unwrap();
    app.handle_mouse(click(lang_rect.x + 1, lang_rect.y), area)
        .unwrap();
    assert_eq!(app.ctx.i18n.locale(), "en-US");

    // Confirmation dialog cannot be dismissed by clicking outside
    let (tx, rx) = mpsc::channel();
    app.confirmation = Some(Confirmation {
        text: "Confirm dangerous op?".into(),
        token: "YES".into(),
        input: super::input::InputBuffer::default(),
        reply: tx,
        scroll: 0,
        accept: false,
    });
    app.handle_mouse(click(0, 0), area).unwrap();
    assert!(app.confirmation.is_some());
    assert!(rx.try_recv().is_err());

    // Clicking Confirm button accepts
    let geom = layout::ConfirmationGeometry::compute(
        area,
        app.confirmation.as_ref().unwrap(),
        &app.ctx.i18n,
    );
    app.handle_mouse(click(geom.buttons[1].x + 1, geom.buttons[1].y), area)
        .unwrap();
    assert!(app.confirmation.is_none());
    assert!(rx.recv().unwrap());
}

#[test]
fn category_order_drops_legacy_optimization_and_rejects_unknown_ids() {
    let default = default_category_order();
    assert_eq!(default.len(), GROUPS.len() - 1);
    assert!(!default.contains(&OPTIMIZATION_GROUP));

    let with_opt = vec!["optimization".into(), "kernel".into(), "system".into()];
    let parsed = category_order_from_ids(&with_opt).unwrap();
    assert_eq!(
        parsed[0],
        GROUPS.iter().position(|g| *g == "kernel").unwrap()
    );
    assert!(!parsed.contains(&OPTIMIZATION_GROUP));

    assert!(category_order_from_ids(&["nonexistent".into()]).is_err());
    assert!(category_order_from_ids(&["system".into(), "system".into()]).is_err());
}

#[test]
fn optimization_recommend_and_toggle_produce_batch_effect() {
    let (_temp, mut app) = test_app("en-US");
    app.goto_nav(NavTarget::Optimization);
    app.handle_key(key(KeyCode::Char('a'))).unwrap();
    assert_eq!(app.marked.len(), 4);
    let effect = app.handle_key(key(KeyCode::Char('x'))).unwrap();
    assert!(matches!(effect, Effect::Run(_)));
}

#[test]
fn plain_theme_in_english_renders_strictly_pure_ascii_borders_and_glyphs() {
    let (_temp, mut app) = test_app("en-US");
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    for nav in [
        NavTarget::Overview,
        NavTarget::Optimization,
        NavTarget::Group(0),
    ] {
        app.goto_nav(nav);
        app.viewport = Rect::new(0, 0, 80, 24);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let mut text = String::new();
        for cell in &buf.content {
            let sym = cell.symbol();
            let forbidden = sym.chars().any(|c| {
                matches!(
                    c,
                    '╭' | '╮'
                        | '╰'
                        | '╯'
                        | '┌'
                        | '┐'
                        | '└'
                        | '┘'
                        | '─'
                        | '│'
                        | '━'
                        | '▎'
                        | '↑'
                        | '↓'
                        | '←'
                        | '→'
                        | '↵'
                        | '◆'
                        | '◈'
                        | '✦'
                        | '●'
                        | '○'
                        | '✓'
                        | '✗'
                )
            });
            assert!(
                !forbidden,
                "Unicode UI glyph {sym:?} leaked in plain mode ({nav:?})"
            );
            text.push_str(sym);
        }
        assert!(text.contains("+-"), "expected ASCII border in {nav:?}");
    }
}

#[test]
fn new_ui_app_uses_valid_locale_keys() {
    for locale in ["en-US", "zh-CN"] {
        let i18n = I18n::load(Some(locale), None).unwrap();
        let keys = [
            "theme.auto.name",
            "theme.obsidian.name",
            "theme.daylight.name",
            "theme.tokyo-night.name",
            "theme.catppuccin.name",
            "theme.rose-pine.name",
            "theme.high-contrast.name",
            "theme.plain.name",
            "nav.overview",
            "nav.section.infra",
            "nav.section.net",
            "nav.section.more",
            "nav.toggle",
            "nav.collapse",
            "overview.quick_title",
            "overview.card_opt_title",
            "overview.card_bbr_title",
            "overview.card_public_title",
            "overview.card_clean_title",
            "overview.card_mihomo_title",
            "overview.card_recent_title",
            "palette.recent",
            "palette.commands",
            "palette.empty",
            "cmd.overview",
            "cmd.optimization",
            "cmd.github_proxy",
            "cmd.theme",
            "cmd.transparent",
            "cmd.motion",
            "cmd.sidebar",
            "cmd.language",
            "cmd.refresh",
            "cmd.public_ip",
            "cmd.logs",
            "cmd.full_logs",
            "ui.style_transparent",
            "ui.style_motion",
            "ui.style_on",
            "ui.style_off",
        ];
        for key in keys {
            let text = i18n.text(key);
            assert_ne!(text, key, "missing key {key} in {locale}");
        }
    }
}

#[test]
fn github_proxy_shortcut_opens_prefilled_form_persists_and_updates_live_context() {
    let (_temp, mut app) = test_app("zh-CN");
    assert_eq!(app.ctx.github_proxy, None);

    // Pressing `P` opens the github.proxy form pre-populated with current state (off + default URL).
    let effect = app.handle_key(key(KeyCode::Char('p'))).unwrap();
    assert!(matches!(effect, Effect::None));
    let form = app.form.as_ref().expect("github.proxy form should open");
    assert_eq!(form.spec.id, "github.proxy");
    assert_eq!(form.values[0].text, "off");
    assert_eq!(form.values[1].text, crate::github_proxy::DEFAULT_URL);

    // Toggle chip to `on` and submit with Ctrl+Enter.
    app.handle_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.form.as_ref().unwrap().values[0].text, "on");
    let effect = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();
    let (spec, params, reviewed) = match effect {
        Effect::Run(super::state::Task::Single {
            spec,
            params,
            reviewed,
        }) => (spec, params, reviewed),
        other => panic!("expected Effect::Run(Single), got {other:?}"),
    };
    assert_eq!(spec.id, "github.proxy");
    assert!(reviewed);

    // Execute the reviewed action and notify TUI completion; live context updates immediately.
    app.task_ids = vec![spec.id.into()];
    crate::actions::execute_reviewed(&app.ctx, &spec, &params).unwrap();
    app.finish(Ok(()));
    assert_eq!(
        app.ctx.github_proxy.as_deref(),
        Some(crate::github_proxy::DEFAULT_URL)
    );

    // Overview now renders the active GitHub acceleration proxy URL.
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let mut text = String::new();
    for cell in terminal.backend().buffer().content() {
        text.push_str(cell.symbol());
    }
    assert!(
        text.contains("https://gh-proxy.com/"),
        "expected Overview to display active GitHub proxy URL"
    );
}

#[test]
fn prompt_preview_studio_cycles_styles_palettes_toggles_and_applies() {
    let (_temp, mut app) = test_app("zh-CN");
    app.viewport = Rect::new(0, 0, 120, 36);

    // Even when platform is non-Debian (Windows preview mode), terminal.preview opens cleanly.
    app.platform_error = Some(crate::error::Error::new("error.platform", "windows"));
    let preview_spec = crate::actions::find("terminal.preview").unwrap();
    app.activate_action(preview_spec).unwrap();
    assert!(app.prompt_preview.is_some());

    // Applying in preview-only mode keeps studio open and shows a warning toast.
    let eff = app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(matches!(eff, Effect::None));
    assert!(app.prompt_preview.is_some());
    assert!(app.toast.is_some());

    // Cycle style, palette, time, git, shell, and TOML view via keyboard.
    let initial_style = app.prompt_preview.as_ref().unwrap().style_idx;
    let initial_color = app.prompt_preview.as_ref().unwrap().color_idx;
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Right)).unwrap();
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    app.handle_key(key(KeyCode::Char('s'))).unwrap();
    app.handle_key(key(KeyCode::Char('v'))).unwrap();
    {
        let p = app.prompt_preview.as_ref().unwrap();
        assert_eq!(
            p.style_idx,
            (initial_style + 1) % crate::ops::terminal::STYLES.len()
        );
        assert_eq!(
            p.color_idx,
            (initial_color + 1) % crate::ops::terminal::COLORS.len()
        );
        assert!(p.show_toml);
        assert_eq!(p.shell(), "zsh");
    }

    // Renders cleanly across all breakpoints in both Preview and TOML modes, including plain ASCII.
    for (w, h) in [(80, 24), (100, 30), (120, 36)] {
        let area = Rect::new(0, 0, w, h);
        app.viewport = area;
        for show_toml in [false, true] {
            app.prompt_preview.as_mut().unwrap().show_toml = show_toml;
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.draw(f)).unwrap();
        }
    }

    // Mouse clicks update style and palette accurately.
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    let geom = layout::PromptPreviewGeometry::compute(area, &app.ctx.i18n);
    assert_eq!(geom.style_rects.len(), crate::ops::terminal::STYLES.len());
    assert_eq!(geom.color_rects.len(), crate::ops::terminal::COLORS.len());
    let target_style = geom.style_rects[4]; // powerline
    app.handle_mouse(click(target_style.x + 2, target_style.y), area)
        .unwrap();
    assert_eq!(app.prompt_preview.as_ref().unwrap().style(), "powerline");
    let target_color = geom.color_rects[4]; // catppuccin
    app.handle_mouse(click(target_color.x + 2, target_color.y), area)
        .unwrap();
    assert_eq!(app.prompt_preview.as_ref().unwrap().color(), "catppuccin");

    // On Debian/Ubuntu, pressing Enter applies the exact previewed options directly.
    app.platform_error = None;
    let effect = app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.prompt_preview.is_none());
    match effect {
        Effect::Run(super::state::Task::Single {
            spec,
            params,
            reviewed,
        }) => {
            assert_eq!(spec.id, "terminal.apply");
            assert!(reviewed);
            assert_eq!(params["style"], "powerline");
            assert_eq!(params["color"], "catppuccin");
            assert_eq!(params["shell"], "zsh");
        }
        other => panic!("expected terminal.apply task, got {other:?}"),
    }
}

#[test]
fn terminal_apply_form_wraps_all_chips_and_renders_live_preview_at_all_sizes() {
    let (_temp, mut app) = test_app("zh-CN");
    let spec = crate::actions::find("terminal.apply").unwrap();
    app.form = Some(app.make_form(spec, super::state::FormMode::Run).unwrap());

    for (w, h) in [(80, 24), (100, 30), (120, 36)] {
        let area = Rect::new(0, 0, w, h);
        app.viewport = area;
        let geom = layout::FormGeometry::compute(area, app.form.as_ref().unwrap(), &app);
        assert!(
            geom.terminal_preview.is_some(),
            "expected live preview at {w}x{h}"
        );
        // Every choice of all 5 fields (3 + 7 + 9 + 2 + 2 = 23 chips) must be laid out.
        assert_eq!(geom.chips.len(), 23, "missing wrapped chips at {w}x{h}");
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
    }

    // Plain ASCII mode must never emit Unicode UI chrome inside the preview studio or form.
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    let area = Rect::new(0, 0, 80, 24);
    app.viewport = area;
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    for cell in term.backend().buffer().content() {
        assert!(
            !matches!(
                cell.symbol(),
                "╭" | "╮" | "╰" | "╯" | "─" | "│" | "●" | "❯" | "✘"
            ),
            "Unicode leaked in plain terminal form: {:?}",
            cell.symbol()
        );
    }
}

#[test]
fn grouped_workbench_section_pills_hierarchy_and_shortcuts_work() {
    let (_temp, mut app) = test_app("zh-CN");
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    let clash = GROUPS.iter().position(|g| *g == "clash").unwrap();

    // Mihomo opens directly on its first section tab (`core`, 6 items) instead of a 37-item wall.
    app.goto_nav(NavTarget::Group(clash));
    assert_eq!(app.section_tab(clash), Some(0));
    assert_eq!(app.selectable_count(clash), 6);

    // Right switches to `subscriptions` (6 items); Left twice reaches `All` (37 items + headers + spacers).
    app.handle_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.section_tab(clash), Some(1));
    assert_eq!(app.selectable_count(clash), 6);
    app.handle_key(key(KeyCode::Left)).unwrap();
    assert_eq!(app.section_tab(clash), Some(0));
    app.handle_key(key(KeyCode::Left)).unwrap();
    assert_eq!(app.section_tab(clash), None);
    assert_eq!(app.selectable_count(clash), 37);

    // In All view, PageDown jumps straight to the first action of the next section (`subscriptions`).
    assert_eq!(app.selected_in(clash), 0);
    app.handle_key(key(KeyCode::PageDown)).unwrap();
    assert_eq!(app.selected_in(clash), 6);
    app.handle_key(key(KeyCode::PageUp)).unwrap();
    assert_eq!(app.selected_in(clash), 0);

    // Clicking a section pill switches straight to that section.
    let regions = LayoutRegions::compute(area, &app);
    let wb = regions.workbench.as_ref().unwrap();
    assert_eq!(wb.section_pills.len(), 9);
    let panel_pill = wb
        .section_pills
        .iter()
        .find(|p| p.tab == Some(3))
        .expect("panel section pill");
    app.handle_mouse(click(panel_pill.rect.x + 1, panel_pill.rect.y), area)
        .unwrap();
    assert_eq!(app.section_tab(clash), Some(3));
    assert_eq!(app.selectable_count(clash), 3);

    // Clicking a section header row in All view drills straight into that section.
    app.set_section_tab(clash, None);
    let regions = LayoutRegions::compute(area, &app);
    let wb = regions.workbench.as_ref().unwrap();
    let rows = app.rows(clash);
    let sub_header_idx = rows
        .iter()
        .position(|r| matches!(r, super::state::Row::Section("subscriptions", _)))
        .unwrap();
    let sub_y = wb.list_inner.y + (sub_header_idx - wb.list_scroll) as u16;
    app.handle_mouse(click(wb.list_inner.x + 2, sub_y), area)
        .unwrap();
    assert_eq!(app.section_tab(clash), Some(1));

    // Command Palette jump automatically activates the owning section tab.
    let dns_spec = crate::actions::find("clash.dns.configure").unwrap();
    app.jump_to_action(&dns_spec);
    assert_eq!(app.section_tab(clash), Some(5));
    assert!(matches!(
        app.selected_row(),
        Some(super::state::Row::Action(ref s)) if s.id == "clash.dns.configure"
    ));

    // Plain ASCII mode renders both single-section and All views cleanly with zero Unicode chrome.
    app.ctx.i18n = I18n::load(Some("en-US"), None).unwrap();
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    for tab in [Some(0), None] {
        app.set_section_tab(clash, tab);
        for (w, h) in [(80, 24), (120, 36)] {
            app.viewport = Rect::new(0, 0, w, h);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.draw(f)).unwrap();
            for cell in term.backend().buffer().content() {
                assert!(
                    !matches!(
                        cell.symbol(),
                        "╭" | "╮" | "╰" | "╯" | "─" | "│" | "▎" | "▸" | "‹" | "›" | "·"
                    ),
                    "Unicode glyph {:?} leaked in plain mode (tab={tab:?}, {w}x{h})",
                    cell.symbol()
                );
            }
        }
    }
}

#[test]
fn copy_shortcuts_border_free_drag_selection_and_plain_view_work() {
    let drag = |col: u16, row: u16| MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let release = |col: u16, row: u16| MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };

    let (_temp, mut app) = test_app("zh-CN");
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    app.goto_nav(NavTarget::Group(0)); // System -> system.info

    // Render once so `selectable_panes` and `last_cmd_rect` are populated.
    let mut term = Terminal::new(TestBackend::new(120, 36)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let regions = LayoutRegions::compute(area, &app);
    let wb = regions.workbench.as_ref().unwrap();
    let copy_btn = wb.inspector_copy_btn.expect("inspector copy button");
    let cmd_rect = app
        .last_cmd_rect
        .get()
        .expect("inspector command strip rect");

    // 1. Pressing `c` copies the exact CLI command; `C` copies full Inspector summary.
    match app.handle_key(key(KeyCode::Char('c'))).unwrap() {
        Effect::Copy(text) => assert_eq!(text, "cqs run system.info"),
        other => panic!("expected Effect::Copy, got {other:?}"),
    }
    match app.handle_key(key(KeyCode::Char('C'))).unwrap() {
        Effect::Copy(text) => {
            assert!(text.contains("system.info"));
            assert!(text.contains("cqs run system.info"));
        }
        other => panic!("expected Effect::Copy, got {other:?}"),
    }

    // 2. Clicking the Copy button or simple-clicking `$ cqs run ...` copies the full command.
    match app
        .handle_mouse(click(copy_btn.x + 1, copy_btn.y), area)
        .unwrap()
    {
        Effect::Copy(text) => assert_eq!(text, "cqs run system.info"),
        other => panic!("expected Effect::Copy from button, got {other:?}"),
    }
    app.handle_mouse(click(cmd_rect.x + 3, cmd_rect.y), area)
        .unwrap();
    match app
        .handle_mouse(release(cmd_rect.x + 3, cmd_rect.y), area)
        .unwrap()
    {
        Effect::Copy(text) => assert_eq!(text, "cqs run system.info"),
        other => panic!("expected Effect::Copy from cmd click, got {other:?}"),
    }

    // 3. Dragging across `$ cqs run system.info` past the right `│` border clamps cleanly & copies ONLY `system.info` (zero `│`!).
    app.handle_mouse(click(cmd_rect.x + 11, cmd_rect.y), area)
        .unwrap();
    app.handle_mouse(drag(area.right() - 1, cmd_rect.y), area)
        .unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    match app
        .handle_mouse(release(area.right() - 1, cmd_rect.y), area)
        .unwrap()
    {
        Effect::Copy(text) => {
            assert_eq!(text, "system.info");
            assert!(!text.contains('│'));
            assert!(!text.contains('|'));
        }
        other => panic!("expected Effect::Copy from inspector drag, got {other:?}"),
    }

    // 4. Logs: border-free multi-line drag inside Log Drawer (even dragging over outer borders!) never captures `│`.
    app.logs.push_back(super::state::LogLine {
        text: "line 1: http://127.0.0.1:9090/ui/".into(),
        tone: super::state::Tone::Plain,
        bold: false,
    });
    app.logs.push_back(super::state::LogLine {
        text: "line 2: hot-reloaded ok".into(),
        tone: super::state::Tone::Success,
        bold: false,
    });
    app.logs_open = true;
    term.draw(|f| app.draw(f)).unwrap();
    let regions = LayoutRegions::compute(area, &app);
    let log_body = regions.log_body;
    app.handle_mouse(click(log_body.x, log_body.y), area)
        .unwrap();
    app.handle_mouse(drag(area.right() - 1, log_body.y + 1), area)
        .unwrap();
    match app
        .handle_mouse(release(area.right() - 1, log_body.y + 1), area)
        .unwrap()
    {
        Effect::Copy(text) => {
            assert_eq!(
                text,
                "line 1: http://127.0.0.1:9090/ui/\nline 2: hot-reloaded ok"
            );
            assert!(!text.contains('│'));
        }
        other => panic!("expected Effect::Copy from log drawer drag, got {other:?}"),
    }

    // Drawer Copy & PlainView buttons work.
    let drawer_copy = regions.log_copy_btn.expect("log drawer copy button");
    match app
        .handle_mouse(click(drawer_copy.x + 1, drawer_copy.y), area)
        .unwrap()
    {
        Effect::Copy(text) => assert!(text.contains("hot-reloaded ok")),
        other => panic!("expected Effect::Copy from drawer button, got {other:?}"),
    }
    let drawer_plain = regions.log_plain_btn.expect("log drawer plain button");
    match app
        .handle_mouse(click(drawer_plain.x + 1, drawer_plain.y), area)
        .unwrap()
    {
        Effect::PlainView { content, .. } => {
            assert_eq!(
                content,
                "line 1: http://127.0.0.1:9090/ui/\nline 2: hot-reloaded ok"
            );
        }
        other => panic!("expected Effect::PlainView from drawer button, got {other:?}"),
    }

    // 5. Full-screen Log Modal: `c` copies all logs, `v` opens borderless PlainView.
    app.log_modal = true;
    term.draw(|f| app.draw(f)).unwrap();
    match app.handle_key(key(KeyCode::Char('c'))).unwrap() {
        Effect::Copy(text) => assert!(text.contains("127.0.0.1:9090/ui/")),
        other => panic!("expected Effect::Copy from log modal c, got {other:?}"),
    }
    match app.handle_key(key(KeyCode::Char('v'))).unwrap() {
        Effect::PlainView { content, .. } => assert!(content.contains("hot-reloaded ok")),
        other => panic!("expected Effect::PlainView from log modal v, got {other:?}"),
    }
    let (modal, _) = super::layout::full_log_modal(area);
    let modal_btns = super::layout::full_log_modal_buttons(modal, &app);
    match app
        .handle_mouse(click(modal_btns.plain.x + 1, modal_btns.plain.y), area)
        .unwrap()
    {
        Effect::PlainView { content, .. } => assert!(content.contains("hot-reloaded ok")),
        other => panic!("expected Effect::PlainView from log modal button, got {other:?}"),
    }
    app.log_modal = false;

    // 6. Notice dialog: `c` copies notice text, `v` opens borderless PlainView.
    app.notice = Some("controller returned HTTP 204".into());
    match app.handle_key(key(KeyCode::Char('c'))).unwrap() {
        Effect::Copy(text) => assert_eq!(text, "controller returned HTTP 204"),
        other => panic!("expected Effect::Copy from notice, got {other:?}"),
    }
    match app.handle_key(key(KeyCode::Char('v'))).unwrap() {
        Effect::PlainView { content, .. } => assert_eq!(content, "controller returned HTTP 204"),
        other => panic!("expected Effect::PlainView from notice, got {other:?}"),
    }
    app.notice = None;

    // 7. Plain ASCII mode renders `[c Copy cmd]`, `[y copy]`, `[V Plain text]` with zero Unicode chrome.
    app.ctx.i18n = I18n::load(Some("en-US"), None).unwrap();
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let plain_text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(plain_text.contains("[c Copy cmd]"));
    assert!(plain_text.contains("[y copy]"));
    assert!(plain_text.contains("[V Plain text]"));
    for cell in term.backend().buffer().content() {
        assert!(
            !matches!(
                cell.symbol(),
                "╭" | "╮"
                    | "╰"
                    | "╯"
                    | "─"
                    | "│"
                    | "▎"
                    | "▸"
                    | "‹"
                    | "›"
                    | "↵"
                    | "◦"
                    | "•"
                    | "◆"
            ),
            "Unicode chrome glyph {:?} leaked in plain mode",
            cell.symbol()
        );
    }
}

#[test]
fn docker_workspace_renders_all_tabs_modals_states_and_pure_ascii_plain_mode() {
    use super::docker::{
        fixture::fixture_docker_state,
        state::{ComposeSubTab, ContainerSubTab, DockerModal, ImageSubTab},
    };
    use crate::ops::docker::model::{DockerAvailability, DockerTab};

    let sizes = [(80, 24), (100, 30), (120, 36)];
    for locale in ["zh-CN", "en-US"] {
        let (_temp, mut app) = test_app(locale);
        app.docker = fixture_docker_state(app.now);
        app.open_docker(DockerTab::Containers, false, false);

        for (w, h) in sizes {
            app.viewport = Rect::new(0, 0, w, h);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            for fullscreen in [false, true] {
                app.docker.fullscreen = fullscreen;
                for tab in DockerTab::ALL {
                    app.docker.set_tab(tab);
                    term.draw(|f| app.draw(f)).unwrap();
                }
            }
            // Exercise every Container / Compose / Image sub-tab & fullscreen logs
            app.docker.set_tab(DockerTab::Containers);
            for sub in ContainerSubTab::ALL {
                app.docker.container_sub = sub;
                term.draw(|f| app.draw(f)).unwrap();
            }
            app.docker.show_raw_json = true;
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.show_raw_json = false;
            app.docker.logs.fullscreen = true;
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.logs.fullscreen = false;

            app.docker.set_tab(DockerTab::Compose);
            for sub in ComposeSubTab::ALL {
                app.docker.compose_sub = sub;
                term.draw(|f| app.draw(f)).unwrap();
            }
            app.docker.set_tab(DockerTab::Images);
            for sub in ImageSubTab::ALL {
                app.docker.image_sub = sub;
                term.draw(|f| app.draw(f)).unwrap();
            }

            // Exercise all Docker modals
            app.docker.set_tab(DockerTab::Containers);
            app.docker.modal = app.docker.build_action_menu();
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.modal = Some(DockerModal::pull_default());
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.modal = Some(DockerModal::network_default());
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.modal = Some(DockerModal::exec_default(
                "c001".into(),
                "nginx-proxy".into(),
            ));
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.modal = Some(DockerModal::prune_default());
            term.draw(|f| app.draw(f)).unwrap();
            app.docker.modal = None;

            // Exercise unavailable guidance states
            for avail in [
                DockerAvailability::Loading,
                DockerAvailability::NotInstalled,
                DockerAvailability::DaemonStopped("Cannot connect".into()),
                DockerAvailability::PermissionDenied("permission denied".into()),
                DockerAvailability::NotApplicable("windows".into()),
                DockerAvailability::Unavailable("timeout".into()),
            ] {
                app.docker.snapshot.availability = avail;
                term.draw(|f| app.draw(f)).unwrap();
            }
            app.docker.snapshot.availability = DockerAvailability::Ready;
        }
    }

    // Strict pure-ASCII verification in `plain` + `en-US`
    let (_temp, mut app) = test_app("en-US");
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, true, false);
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for tab in DockerTab::ALL {
        app.docker.set_tab(tab);
        term.draw(|f| app.draw(f)).unwrap();
        for cell in term.backend().buffer().content() {
            assert!(
                cell.symbol().is_ascii(),
                "Non-ASCII glyph {:?} leaked on Docker tab {tab:?} in plain mode",
                cell.symbol()
            );
        }
    }
}

#[test]
fn docker_workspace_keyboard_mouse_and_safety_flows_work_end_to_end() {
    use super::{
        docker::{
            fixture::fixture_docker_state,
            state::{ContainerSubTab, DockerModal},
        },
        state::Task,
    };
    use crate::ops::docker::{
        actions::{ConfirmPolicy, DockerOp},
        model::DockerTab,
    };

    let (_temp, mut app) = test_app("zh-CN");
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, false, false);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;

    // 1. Toggle Focus mode with `z`, then jump directly to tabs with `1`-`6`
    app.handle_key(key(KeyCode::Char('z'))).unwrap();
    assert!(app.docker.fullscreen);
    app.handle_key(key(KeyCode::Char('2'))).unwrap();
    assert_eq!(app.docker.tab, DockerTab::Compose);
    app.handle_key(key(KeyCode::Char('1'))).unwrap();
    assert_eq!(app.docker.tab, DockerTab::Containers);

    // 2. Filter inline with `/` and clear with `Esc`
    app.handle_key(key(KeyCode::Char('/'))).unwrap();
    for ch in "postgres".chars() {
        app.handle_key(key(KeyCode::Char(ch))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert_eq!(app.docker.filtered_containers().len(), 1);
    assert_eq!(app.docker.selected_container().unwrap().name, "postgres-16");
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert_eq!(app.docker.filtered_containers().len(), 6);

    // 3. Multi-select 3 containers and trigger batch remove -> requires L3 `REMOVE` token!
    app.docker.set_cursor(0);
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    assert_eq!(app.docker.selected_count(), 3);
    match app.handle_key(key(KeyCode::Char('d'))).unwrap() {
        Effect::Run(Task::DockerOp(op)) => {
            assert_eq!(op.confirm_policy(), ConfirmPolicy::Token("REMOVE"));
        }
        other => panic!("expected batch remove DockerOp, got {other:?}"),
    }
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert_eq!(app.docker.selected_count(), 0);

    // 4. Quick exec on running container emits Effect::DockerExec; on exited container shows toast
    app.docker.set_cursor(0);
    match app.handle_key(key(KeyCode::Char('e'))).unwrap() {
        Effect::DockerExec(spec) => assert_eq!(spec.container_name, "api-gateway"),
        other => panic!("expected DockerExec, got {other:?}"),
    }
    // Move to exited container (`backup-cron`, last row)
    app.handle_key(key(KeyCode::End)).unwrap();
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('e'))).unwrap(),
        Effect::None
    ));
    assert!(app.toast.is_some());

    // 5. Env secret masking toggles via Shift+S and auto-resets when moving to another container
    app.docker.set_cursor(0);
    app.handle_key(key(KeyCode::Char('v'))).unwrap();
    assert_eq!(app.docker.container_sub, ContainerSubTab::Env);
    assert!(!app.docker.reveal_secrets);
    app.handle_key(key(KeyCode::Char('S'))).unwrap();
    assert!(app.docker.reveal_secrets);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert!(!app.docker.reveal_secrets);

    // 6. Logs subtab -> second `l` opens fullscreen logs -> copy & PlainView work -> Esc closes
    app.handle_key(key(KeyCode::Char('l'))).unwrap();
    assert_eq!(app.docker.container_sub, ContainerSubTab::Logs);
    app.handle_key(key(KeyCode::Char('l'))).unwrap();
    assert!(app.docker.logs.fullscreen);
    match app.handle_key(key(KeyCode::Char('y'))).unwrap() {
        Effect::Copy(text) => assert!(text.contains("webhook delivery")),
        other => panic!("expected Effect::Copy from fullscreen logs, got {other:?}"),
    }
    match app.handle_key(key(KeyCode::Char('V'))).unwrap() {
        Effect::PlainView { content, .. } => assert!(content.contains("webhook delivery")),
        other => panic!("expected Effect::PlainView from fullscreen logs, got {other:?}"),
    }
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.docker.logs.fullscreen);

    // 7. Built-in network & in-use volume deletion protection
    app.docker.set_tab(DockerTab::Networks);
    app.docker.set_cursor(0); // `bridge` (builtin)
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('d'))).unwrap(),
        Effect::None
    ));
    app.docker.set_tab(DockerTab::Volumes);
    app.docker.set_cursor(0); // `pg_data` (in-use by postgres-16)
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('d'))).unwrap(),
        Effect::None
    ));

    // 8. Prune wizard (`Shift+P`) with volumes checked yields `PRUNE` token policy
    app.handle_key(key(KeyCode::Char('P'))).unwrap();
    assert!(matches!(
        app.docker.modal,
        Some(DockerModal::PruneWizard { .. })
    ));
    // Move to row 3 (volumes) and toggle on, then press Enter
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down)).unwrap();
    }
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    match app.handle_key(key(KeyCode::Enter)).unwrap() {
        Effect::Run(Task::DockerOp(op @ DockerOp::Prune { volumes: true, .. })) => {
            assert_eq!(op.confirm_policy(), ConfirmPolicy::Token("PRUNE"));
        }
        other => panic!("expected Prune op with volumes=true, got {other:?}"),
    }

    // 9. Mouse click on tab pills switches tabs accurately
    let regions = LayoutRegions::compute(area, &app);
    let dk_geom = regions.docker.as_ref().unwrap();
    let images_pill = dk_geom
        .tab_pills
        .iter()
        .find(|p| p.tab == DockerTab::Images)
        .unwrap();
    app.handle_mouse(click(images_pill.rect.x + 1, images_pill.rect.y), area)
        .unwrap();
    assert_eq!(app.docker.tab, DockerTab::Images);
}

#[test]
fn mobile_viewports_including_59x24_and_40x16_render_and_interact_cleanly() {
    use super::docker::{
        fixture::fixture_docker_state,
        state::{ComposeSubTab, ContainerSubTab, DockerModal, ImageSubTab},
    };
    use crate::ops::docker::model::{DockerAvailability, DockerTab};

    // Minimum viewport boundaries & breakpoint classification
    assert!(!layout::fits(Rect::new(0, 0, 39, 16)));
    assert!(!layout::fits(Rect::new(0, 0, 40, 15)));
    assert!(layout::fits(Rect::new(0, 0, 40, 16)));
    assert!(layout::fits(Rect::new(0, 0, 59, 24)));
    assert_eq!(Breakpoint::of(40), Breakpoint::Mobile);
    assert_eq!(Breakpoint::of(59), Breakpoint::Mobile);
    assert_eq!(Breakpoint::of(79), Breakpoint::Mobile);
    assert_eq!(Breakpoint::of(80), Breakpoint::Compact);

    // 1. iOS Termius 59x24 interactive flows: TopBar nav/lang taps, Overview sequential list nav,
    //    Terminal Apply form (all 23 chips + live preview), Prompt Preview Studio stacked clicks.
    let (_temp, mut app) = test_app("zh-CN");
    let termius = Rect::new(0, 0, 59, 24);
    app.viewport = termius;
    let regions = LayoutRegions::compute(termius, &app);
    assert!(regions.sidebar.is_none());
    let ov = regions.overview.as_ref().expect("overview geometry");
    assert!(ov.is_list_mode());

    // Overview quick list steps sequentially (0 -> 1 -> 2 -> 3 -> 0) with Down/Up
    assert_eq!(app.quick_cursor, 0);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.quick_cursor, 1);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.quick_cursor, 2);
    app.handle_key(key(KeyCode::Up)).unwrap();
    assert_eq!(app.quick_cursor, 1);

    // Tap TopBar nav toggle -> opens floating sidebar overlay -> tap System category
    let nav_btn = regions.topbar_hits.nav_toggle.expect("nav toggle button");
    app.handle_mouse(click(nav_btn.x + 1, nav_btn.y), termius)
        .unwrap();
    assert!(app.sidebar_overlay);
    let overlay_regions = LayoutRegions::compute(termius, &app);
    let side = overlay_regions.sidebar.as_ref().expect("sidebar overlay");
    assert!(side.overlay);
    let (sys_rect, _) = side
        .item_rects
        .iter()
        .find(|(_, t)| *t == NavTarget::Group(0))
        .expect("system nav item");
    app.handle_mouse(click(sys_rect.x + 1, sys_rect.y), termius)
        .unwrap();
    assert!(!app.sidebar_overlay);
    assert_eq!(app.nav, NavTarget::Group(0));

    // Tap TopBar language toggle at 59x24 AND at 40x16 (compact `中`/`EN` button)
    let lang_btn = regions.topbar_hits.lang_btn.expect("lang button at 59x24");
    app.handle_mouse(click(lang_btn.x + 1, lang_btn.y), termius)
        .unwrap();
    assert_eq!(app.ctx.i18n.locale(), "en-US");
    let min_area = Rect::new(0, 0, 40, 16);
    app.viewport = min_area;
    let min_regions = LayoutRegions::compute(min_area, &app);
    let min_lang = min_regions
        .topbar_hits
        .lang_btn
        .expect("compact lang button at 40x16");
    app.handle_mouse(click(min_lang.x + 1, min_lang.y), min_area)
        .unwrap();
    assert_eq!(app.ctx.i18n.locale(), "zh-CN");

    // On short screens (height < 18), `G` opens full-screen log modal directly since drawer can't fit
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    assert!(app.log_modal);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.log_modal);

    // Help modal scrolls cleanly at 40x16
    app.handle_key(key(KeyCode::Char('?'))).unwrap();
    assert!(app.help);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.help_scroll, 1);
    let mut term = Terminal::new(TestBackend::new(40, 16)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.help);
    assert_eq!(app.help_scroll, 0);

    // `terminal.apply` form wraps ALL 23 chips + live preview at 59x24, and scrolls fields at 40x16
    let term_spec = crate::actions::find("terminal.apply").unwrap();
    app.form = Some(
        app.make_form(term_spec, super::state::FormMode::Run)
            .unwrap(),
    );
    app.viewport = termius;
    let form_geom = layout::FormGeometry::compute(termius, app.form.as_ref().unwrap(), &app);
    assert_eq!(form_geom.chips.len(), 23, "all 23 chips must fit at 59x24");
    assert!(
        form_geom.terminal_preview.is_some(),
        "live terminal preview should remain visible at 59x24"
    );
    app.viewport = min_area;
    for field_idx in 0..app.form.as_ref().unwrap().spec.fields.len() {
        app.form.as_mut().unwrap().index = field_idx;
        let g = layout::FormGeometry::compute(min_area, app.form.as_ref().unwrap(), &app);
        assert!(
            g.fields[field_idx].height > 0,
            "active form field {field_idx} must scroll into view at 40x16"
        );
        term.draw(|f| app.draw(f)).unwrap();
    }
    app.form = None;

    // Prompt Preview Studio uses stacked layout on 59x24 & 40x16 and mouse clicks select styles/colors
    app.viewport = termius;
    app.open_prompt_preview();
    let p_geom = layout::PromptPreviewGeometry::compute_for(
        termius,
        &app.ctx.i18n,
        app.prompt_preview.as_ref().unwrap().style_idx,
        app.prompt_preview.as_ref().unwrap().color_idx,
    );
    assert!(p_geom.stacked);
    assert!(p_geom.preview_box.width >= 50);
    let visible_style = p_geom
        .style_rects
        .iter()
        .enumerate()
        .find(|(_, r)| r.height > 0 && r.width > 0)
        .map(|(i, r)| (i, *r))
        .expect("visible style row");
    app.handle_mouse(click(visible_style.1.x + 1, visible_style.1.y), termius)
        .unwrap();
    assert_eq!(
        app.prompt_preview.as_ref().unwrap().style_idx,
        visible_style.0
    );
    app.prompt_preview = None;

    // 2. Exhaustive stress render of EVERY view, modal, and Docker state across mobile sizes & locales
    let mobile_sizes = [(40, 16), (48, 20), (59, 24), (80, 18)];
    for locale in ["zh-CN", "en-US"] {
        let (_tmp, mut mapp) = test_app(locale);
        mapp.docker = fixture_docker_state(mapp.now);
        mapp.logs.push_back(super::state::LogLine {
            text: "mobile log line sample 1234567890".into(),
            tone: super::state::Tone::Info,
            bold: false,
        });
        for (w, h) in mobile_sizes {
            let area = Rect::new(0, 0, w, h);
            mapp.viewport = area;
            let mut backend_term = Terminal::new(TestBackend::new(w, h)).unwrap();

            // Main nav targets + sidebar overlay + theme drawer + logs + modals
            for nav in [
                NavTarget::Overview,
                NavTarget::Optimization,
                NavTarget::Group(0),
                NavTarget::Group(GROUPS.iter().position(|g| *g == "clash").unwrap()),
            ] {
                mapp.goto_nav(nav);
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }

            mapp.sidebar_overlay = true;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.sidebar_overlay = false;

            mapp.open_theme_drawer();
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.cancel_theme_drawer();

            mapp.command_palette = Some(super::palette::CommandPalette::default());
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.command_palette = None;

            mapp.logs_open = true;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.log_modal = true;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.log_modal = false;
            mapp.logs_open = false;

            mapp.notice = Some("Mobile viewport notice text wrapping verification.".into());
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.notice = None;

            let (tx, _rx) = mpsc::channel();
            mapp.confirmation = Some(Confirmation {
                text: "Confirm mobile operation with wrapping details.".into(),
                token: "REMOVE".into(),
                input: super::input::InputBuffer::default(),
                reply: tx,
                scroll: 0,
                accept: false,
            });
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.confirmation = None;

            mapp.open_prompt_preview();
            for toml in [false, true] {
                mapp.prompt_preview.as_mut().unwrap().show_toml = toml;
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }
            mapp.prompt_preview = None;

            // Docker: every tab, sub-tab, modal, fullscreen logs/inspect, and unavailable state
            mapp.open_docker(DockerTab::Containers, false, false);
            for tab in DockerTab::ALL {
                mapp.docker.set_tab(tab);
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }
            mapp.docker.set_tab(DockerTab::Containers);
            for sub in ContainerSubTab::ALL {
                mapp.docker.container_sub = sub;
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }
            mapp.docker.logs.fullscreen = true;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.docker.logs.fullscreen = false;
            mapp.docker.inspector_fullscreen = true;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.docker.inspector_fullscreen = false;

            mapp.docker.set_tab(DockerTab::Compose);
            for sub in ComposeSubTab::ALL {
                mapp.docker.compose_sub = sub;
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }
            mapp.docker.set_tab(DockerTab::Images);
            for sub in ImageSubTab::ALL {
                mapp.docker.image_sub = sub;
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }

            mapp.docker.set_tab(DockerTab::Containers);
            for modal in [
                mapp.docker.build_action_menu(),
                Some(DockerModal::pull_default()),
                Some(DockerModal::network_default()),
                Some(DockerModal::exec_default(
                    "c001".into(),
                    "api-gateway".into(),
                )),
                Some(DockerModal::prune_default()),
            ] {
                mapp.docker.modal = modal;
                backend_term.draw(|f| mapp.draw(f)).unwrap();
            }
            mapp.docker.modal = None;

            mapp.docker.snapshot.availability = DockerAvailability::NotInstalled;
            backend_term.draw(|f| mapp.draw(f)).unwrap();
            mapp.docker.snapshot.availability = DockerAvailability::Ready;
        }
    }

    // 3. Pure-ASCII verification in `plain` + `en-US` at 59x24 and 40x16
    let (_tmp, mut papp) = test_app("en-US");
    papp.docker = fixture_docker_state(papp.now);
    papp.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    for (w, h) in [(59, 24), (40, 16)] {
        papp.viewport = Rect::new(0, 0, w, h);
        let mut pterm = Terminal::new(TestBackend::new(w, h)).unwrap();
        for nav in [
            NavTarget::Overview,
            NavTarget::Optimization,
            NavTarget::Group(0),
        ] {
            papp.goto_nav(nav);
            pterm.draw(|f| papp.draw(f)).unwrap();
            for cell in pterm.backend().buffer().content() {
                assert!(
                    !matches!(
                        cell.symbol(),
                        "╭" | "╮"
                            | "╰"
                            | "╯"
                            | "─"
                            | "│"
                            | "▎"
                            | "▸"
                            | "●"
                            | "○"
                            | "◆"
                            | "◈"
                            | "✦"
                    ),
                    "Unicode UI glyph {:?} leaked in plain mode at {w}x{h} ({nav:?})",
                    cell.symbol()
                );
            }
        }
        papp.open_docker(DockerTab::Containers, false, false);
        pterm.draw(|f| papp.draw(f)).unwrap();
        for cell in pterm.backend().buffer().content() {
            assert!(
                cell.symbol().is_ascii(),
                "Non-ASCII glyph {:?} leaked in plain Docker mode at {w}x{h}",
                cell.symbol()
            );
        }
    }
}

#[test]
fn docker_mirror_form_prefills_existing_mirrors_and_exposes_clickable_preset_chips() {
    use super::docker::fixture::fixture_docker_state;
    use crate::ops::docker::model::DockerTab;

    let (_temp, mut app) = test_app("zh-CN");
    app.platform_error = None;
    app.root = true;
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Images, true, false);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;

    // Pressing `g` on Images tab opens the `docker.mirror` form pre-filled from daemon.json snapshot.
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    let form = app
        .form
        .as_ref()
        .expect("docker.mirror form should open via `g`");
    assert_eq!(form.spec.id, "docker.mirror");
    assert_eq!(form.values[0].text, "https://docker.m.daocloud.io");
    assert!(form.preset_chips(0));

    let geom = layout::FormGeometry::compute(area, form, &app);
    assert_eq!(
        geom.chips.len(),
        crate::ops::docker::MIRROR_PRESET_CHOICES.len()
    );
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();

    // Clicking the `clear` chip replaces the field text with `clear`.
    let clear_chip = geom
        .chips
        .iter()
        .find(|(_, ci, _)| crate::ops::docker::MIRROR_PRESET_CHOICES[*ci] == "clear")
        .expect("clear preset chip")
        .2;
    app.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: clear_chip.x + 1,
            row: clear_chip.y,
            modifiers: KeyModifiers::NONE,
        },
        area,
    )
    .unwrap();
    assert_eq!(app.form.as_ref().unwrap().values[0].text, "clear");
}

#[test]
fn docker_alt_arrows_reorder_category_and_focus_sidebar_overlay_works() {
    use super::docker::fixture::fixture_docker_state;
    use super::state::DOCKER_GROUP;
    use crate::ops::docker::model::DockerTab;

    let (_temp, mut app) = test_app("zh-CN");
    app.docker = fixture_docker_state(app.now);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    app.open_docker(DockerTab::Containers, false, false);

    let initial_pos = app
        .category_order
        .iter()
        .position(|&g| g == DOCKER_GROUP)
        .unwrap();
    assert!(initial_pos > 0);

    // Alt+Up in embedded Docker view moves Docker up one slot and persists to config.toml
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT))
        .unwrap();
    let moved_pos = app
        .category_order
        .iter()
        .position(|&g| g == DOCKER_GROUP)
        .unwrap();
    assert_eq!(moved_pos, initial_pos - 1);
    let saved = Preferences::read(&app.ctx.paths.preferences, false).unwrap();
    assert_eq!(saved.category_order[moved_pos], "docker");

    // Ctrl+S / Alt+D must NOT accidentally stop or delete containers
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
            .unwrap(),
        Effect::None
    ));
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT))
            .unwrap(),
        Effect::None
    ));

    // Enter Docker Focus mode on a wide 120x36 viewport: `[` opens sidebar_overlay without collapsing docked sidebar
    app.handle_key(key(KeyCode::Char('z'))).unwrap();
    assert!(app.docker.fullscreen);
    app.handle_key(key(KeyCode::Char('['))).unwrap();
    assert!(app.sidebar_overlay);
    assert!(!app.sidebar_collapsed);

    // Alt+Down inside sidebar_overlay also reorders Docker back down
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(
        app.category_order
            .iter()
            .position(|&g| g == DOCKER_GROUP)
            .unwrap(),
        initial_pos
    );
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.sidebar_overlay);

    // Tab focuses inspector in Focus mode; exiting Focus via Esc resets focus_inspector so list cursor moves
    app.handle_key(key(KeyCode::Tab)).unwrap();
    assert!(app.docker.focus_inspector);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.docker.fullscreen);
    assert!(!app.docker.focus_inspector);
    assert_eq!(app.docker.cursor(), 0);
    app.handle_key(key(KeyCode::Down)).unwrap();
    assert_eq!(app.docker.cursor(), 1);
}

#[test]
fn modal_render_priority_matches_event_routing_when_notice_opens_over_logs() {
    let (_temp, mut app) = test_app("en-US");
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    app.log_modal = true;
    app.notice = Some("UNIQUE_NOTICE_SENTINEL".into());

    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let rendered: String = term
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        rendered.contains("UNIQUE_NOTICE_SENTINEL"),
        "notice must render above log_modal to match keyboard/mouse routing"
    );
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.notice.is_none());
    assert!(app.log_modal);
}
