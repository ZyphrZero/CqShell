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
        danger: true,
        reply: tx,
        scroll: 0,
        accept: false,
    });
    app.handle_mouse(click(0, 0), area).unwrap();
    assert!(app.confirmation.is_some());
    assert!(rx.try_recv().is_err());

    // Clicking Confirm button accepts even for danger-tier confirmations (no typed token required)
    let geom = layout::ConfirmationGeometry::compute(
        area,
        app.confirmation.as_ref().unwrap(),
        &app.ctx.i18n,
    );
    app.handle_mouse(click(geom.buttons[1].x + 1, geom.buttons[1].y), area)
        .unwrap();
    assert!(app.confirmation.is_none());
    assert!(rx.recv().unwrap());

    // Pressing `y` / `n` answers danger confirmations immediately
    for (ch, expected) in [('y', true), ('n', false)] {
        let (tx, rx) = mpsc::channel();
        app.confirmation = Some(Confirmation {
            text: "Confirm dangerous op?".into(),
            danger: true,
            reply: tx,
            scroll: 0,
            accept: false,
        });
        app.handle_key(key(KeyCode::Char(ch))).unwrap();
        assert!(app.confirmation.is_none());
        assert_eq!(rx.recv().unwrap(), expected);
    }
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

    for danger in [false, true] {
        let (tx, _rx) = mpsc::channel();
        app.confirmation = Some(Confirmation {
            text: "Confirm plain ASCII operation.".into(),
            danger,
            reply: tx,
            scroll: 0,
            accept: false,
        });
        for (w, h) in [(80, 24), (40, 16)] {
            app.viewport = Rect::new(0, 0, w, h);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.draw(f)).unwrap();
            for cell in &term.backend().buffer().content {
                assert!(
                    cell.symbol().is_ascii(),
                    "Non-ASCII glyph {:?} leaked in plain confirm modal (danger={danger}, {w}x{h})",
                    cell.symbol()
                );
            }
        }
        app.confirmation = None;
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
    let network = GROUPS.iter().position(|g| *g == "network").unwrap();

    // Network has 3 sections (`bbr`: 3, `tcp`: 2, `github`: 1; total 6) and opens in All view.
    app.goto_nav(NavTarget::Group(network));
    assert_eq!(app.section_tab(network), None);
    assert_eq!(app.selectable_count(network), 6);

    // Right switches to `bbr` (3 items), then `tcp` (2 items); Left twice returns to `All`.
    app.handle_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.section_tab(network), Some(0));
    assert_eq!(app.selectable_count(network), 3);
    app.handle_key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.section_tab(network), Some(1));
    assert_eq!(app.selectable_count(network), 2);
    app.handle_key(key(KeyCode::Left)).unwrap();
    assert_eq!(app.section_tab(network), Some(0));
    app.handle_key(key(KeyCode::Left)).unwrap();
    assert_eq!(app.section_tab(network), None);
    assert_eq!(app.selectable_count(network), 6);

    // In All view, PageDown jumps straight to the first action of the next section (`tcp`).
    assert_eq!(app.selected_in(network), 0);
    app.handle_key(key(KeyCode::PageDown)).unwrap();
    assert_eq!(app.selected_in(network), 3);
    app.handle_key(key(KeyCode::PageUp)).unwrap();
    assert_eq!(app.selected_in(network), 0);

    // Clicking a section pill switches straight to that section (`github`, 1 item).
    let regions = LayoutRegions::compute(area, &app);
    let wb = regions.workbench.as_ref().unwrap();
    assert!(!wb.section_pills.is_empty());
    let gh_pill = wb
        .section_pills
        .iter()
        .find(|p| p.tab == Some(2))
        .expect("github section pill");
    app.handle_mouse(click(gh_pill.rect.x + 1, gh_pill.rect.y), area)
        .unwrap();
    assert_eq!(app.section_tab(network), Some(2));
    assert_eq!(app.selectable_count(network), 1);

    // Clicking a section header row in All view drills straight into that section (`tcp`).
    app.set_section_tab(network, None);
    let regions = LayoutRegions::compute(area, &app);
    let wb = regions.workbench.as_ref().unwrap();
    let rows = app.rows(network);
    let tcp_header_idx = rows
        .iter()
        .position(|r| matches!(r, super::state::Row::Section("tcp", _)))
        .unwrap();
    let tcp_y = wb.list_inner.y + (tcp_header_idx - wb.list_scroll) as u16;
    app.handle_mouse(click(wb.list_inner.x + 2, tcp_y), area)
        .unwrap();
    assert_eq!(app.section_tab(network), Some(1));

    // Command Palette jump automatically activates the owning section tab.
    let gh_spec = crate::actions::find("github.proxy").unwrap();
    app.jump_to_action(&gh_spec);
    assert_eq!(app.section_tab(network), Some(2));
    assert!(matches!(
        app.selected_row(),
        Some(super::state::Row::Action(ref s)) if s.id == "github.proxy"
    ));

    // Plain ASCII mode renders both single-section and All views cleanly with zero Unicode chrome.
    app.ctx.i18n = I18n::load(Some("en-US"), None).unwrap();
    app.preview_theme("plain", Background::Unavailable("serial".into()))
        .unwrap();
    for tab in [Some(0), None] {
        app.set_section_tab(network, tab);
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

    // 3. Multi-select 3 containers and trigger batch remove -> requires L3 Danger Yes/No confirm
    app.docker.set_cursor(0);
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    assert_eq!(app.docker.selected_count(), 3);
    match app.handle_key(key(KeyCode::Char('d'))).unwrap() {
        Effect::Run(Task::DockerOp(op)) => {
            assert_eq!(op.confirm_policy(), ConfirmPolicy::Danger);
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

    // 8. Prune wizard (`Shift+P`) with volumes checked yields L3 Danger Yes/No policy
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
            assert_eq!(op.confirm_policy(), ConfirmPolicy::Danger);
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
                danger: true,
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

#[test]
fn drain_input_batch_coalesces_mouse_motion_flood_before_quit_and_clicks() {
    use crossterm::event::Event as Input;
    use ratatui::layout::Position;
    use std::collections::VecDeque;

    let (_temp, mut app) = test_app("zh-CN");
    app.viewport = Rect::new(0, 0, 120, 36);

    let moved = |col: u16, row: u16| {
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };

    // 1. A burst of hover movements followed by `q` resolves in a single batch without
    //    consuming trailing mouse packets queued behind `q`.
    let mut queue = VecDeque::from(vec![
        moved(60, 22),
        moved(65, 24),
        moved(74, 26),
        Input::Key(key(KeyCode::Char('q'))),
        moved(80, 28),
    ]);
    let first = moved(59, 21);
    let effect = super::drain_input_batch(&mut app, first, || Ok(queue.pop_front()))
        .unwrap()
        .unwrap();
    assert!(matches!(effect, Effect::Quit));
    assert_eq!(app.hover, Some(Position::new(74, 26)));
    assert_eq!(
        queue.len(),
        1,
        "events queued after Quit must not be dispatched"
    );

    // 2. Non-coalescible mouse Down stops the batch immediately so the next frame redraws
    //    updated geometry before subsequent events are processed.
    let mut queue = VecDeque::from(vec![
        moved(20, 10),
        Input::Mouse(click(22, 10)),
        moved(30, 12),
    ]);
    let effect = super::drain_input_batch(&mut app, moved(18, 10), || Ok(queue.pop_front()))
        .unwrap()
        .unwrap();
    let _ = effect;
    assert_eq!(app.hover, Some(Position::new(22, 10)));
    assert_eq!(queue.len(), 1);

    // 3. Continuous motion floods are bounded per frame without dropping any read event.
    let total = super::MAX_COALESCED_EVENTS + 10;
    let mut queue: VecDeque<Input> = (1..total).map(|i| moved(i as u16, 5)).collect();
    let effect = super::drain_input_batch(&mut app, moved(0, 5), || Ok(queue.pop_front()))
        .unwrap()
        .unwrap();
    assert!(matches!(effect, Effect::None));
    assert_eq!(
        app.hover,
        Some(Position::new((super::MAX_COALESCED_EVENTS - 1) as u16, 5))
    );
    assert_eq!(queue.len(), 10);
}

#[test]
fn docker_and_global_help_modals_render_without_clipping_and_scroll_accurately() {
    use super::docker::fixture::fixture_docker_state;
    use crate::ops::docker::model::DockerTab;
    use crate::theme::Background;

    fn render_buffer(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        app.viewport = Rect::new(0, 0, w, h);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        term.backend().buffer().clone()
    }

    fn slice_rect(buf: &ratatui::buffer::Buffer, rect: Rect) -> Vec<String> {
        use unicode_width::UnicodeWidthStr;
        (rect.top()..rect.bottom())
            .map(|y| {
                let mut line = String::new();
                let mut x = rect.left();
                while x < rect.right() {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    x += sym.width().max(1) as u16;
                }
                line
            })
            .collect()
    }

    fn find_modal_rect(
        buf: &ratatui::buffer::Buffer,
        tl: &str,
        tr: &str,
        bl: &str,
        title_needle: &str,
    ) -> Rect {
        let br = if bl == "╰" { "╯" } else { "+" };
        let area = buf.area;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol() == tl
                    && let Some(rx) = ((x + 1)..area.right()).find(|&c| buf[(c, y)].symbol() == tr)
                    && rx - x >= 30
                    && let Some(by) = ((y + 1)..area.bottom())
                        .find(|&r| buf[(x, r)].symbol() == bl && buf[(rx, r)].symbol() == br)
                {
                    let rect = Rect::new(x, y, rx - x + 1, by - y + 1);
                    let top_row = slice_rect(buf, Rect::new(x, y, rect.width, 1));
                    if top_row[0].contains(title_needle) {
                        return rect;
                    }
                }
            }
        }
        panic!("modal border {tl}..{tr} with title {title_needle:?} not found");
    }

    for (locale, expected_title, expected_refresh) in [
        ("zh-CN", "Docker 工作台快捷键", "立即刷新状态"),
        ("en-US", "Docker Workspace Shortcuts", "refresh now"),
    ] {
        let (_temp, mut app) = test_app(locale);
        app.docker = fixture_docker_state(app.now);
        app.open_docker(DockerTab::Containers, false, false);

        // Open help via `?` at 120x36: all rows fit without scrolling or clipping
        app.viewport = Rect::new(0, 0, 120, 36);
        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help);
        let wide_buf = render_buffer(&mut app, 120, 36);
        let wide_rect = find_modal_rect(&wide_buf, "╭", "╮", "╰", expected_title);
        let wide_modal = slice_rect(&wide_buf, wide_rect);
        let wide_joined = wide_modal.join("\n");
        assert!(
            wide_joined.contains(expected_title),
            "{locale}: missing Docker help title:\n{wide_joined}"
        );
        assert!(
            wide_joined.contains(expected_refresh),
            "{locale}: missing accurate `r` refresh hint:\n{wide_joined}"
        );
        assert!(
            wide_joined.contains("Shift+P / c / g")
                && wide_joined.contains("Shift+J / Shift+S")
                && wide_joined.contains("Ctrl+K / Shift+T·L")
                && wide_joined.contains("Shift+G / F2"),
            "{locale}: Docker shortcut keys must not be truncated:\n{wide_joined}"
        );
        for row in &wide_modal {
            assert!(
                !row.contains('…') && !row.contains("..."),
                "{locale} 120x36 help modal row must not clip: {row}"
            );
        }
        // Pressing Down when everything fits must not drift `help_scroll`
        app.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(app.help_scroll, 0);

        // 80x24 and 59x24 and 40x16: wrapped neatly, zero clipping, scroll reaches true bottom (>16 at 40x16)
        for (w, h) in [(80, 24), (59, 24), (40, 16)] {
            app.help_scroll = 0;
            let buf = render_buffer(&mut app, w, h);
            let modal_rect = find_modal_rect(&buf, "╭", "╮", "╰", expected_title);
            let modal_rows = slice_rect(&buf, modal_rect);
            for row in &modal_rows[1..(modal_rows.len() - 1)] {
                assert!(
                    !row.contains('…') && !row.contains("..."),
                    "{locale} {w}x{h} help modal body clipped: {row}"
                );
            }
            let max_s = super::render::help_max_scroll(&app, Rect::new(0, 0, w, h));
            app.handle_key(key(KeyCode::End)).unwrap();
            assert_eq!(app.help_scroll, max_s);
            if w == 40 && h == 16 {
                assert!(
                    max_s > 16,
                    "{locale} 40x16 help modal should allow scrolling past old hardcoded cap 16 (max_s={max_s})"
                );
            }
            // Render scrolled to bottom and verify last shortcut line is visible
            let scrolled_buf = render_buffer(&mut app, w, h);
            let scrolled = slice_rect(&scrolled_buf, modal_rect).join("\n");
            assert!(
                scrolled.contains("Esc / q / Ctrl+C"),
                "{locale} {w}x{h}: bottom entry must be reachable when scrolled to End:\n{scrolled}"
            );
            app.handle_key(key(KeyCode::Home)).unwrap();
            assert_eq!(app.help_scroll, 0);
        }

        // Plain ASCII theme must not emit Unicode arrows or ellipsis inside modal
        app.preview_theme("plain", Background::Disabled).unwrap();
        let ascii_buf = render_buffer(&mut app, 120, 36);
        let ascii_rect = find_modal_rect(&ascii_buf, "+", "+", "+", expected_title);
        let ascii_rows = slice_rect(&ascii_buf, ascii_rect);
        for row in &ascii_rows {
            assert!(
                !row.contains('←')
                    && !row.contains('→')
                    && !row.contains('↑')
                    && !row.contains('↓')
                    && !row.contains('…'),
                "{locale} plain theme help modal leaked non-ASCII glyph: {row}"
            );
        }

        // Closing help via Esc works; `?` also opens help inside Docker fullscreen logs & inspector
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        app.docker.logs.fullscreen = true;
        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help);
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        assert!(app.docker.logs.fullscreen);

        app.docker.logs.fullscreen = false;
        app.docker.inspector_fullscreen = true;
        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help);
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        assert!(app.docker.inspector_fullscreen);
    }

    // Non-Docker help at 120x36 must render long copy/plain-view descriptions without clipping
    let (_temp, mut app) = test_app("zh-CN");
    app.viewport = Rect::new(0, 0, 120, 36);
    app.handle_key(key(KeyCode::Char('?'))).unwrap();
    let global_buf = render_buffer(&mut app, 120, 36);
    let global_rect = find_modal_rect(&global_buf, "╭", "╮", "╰", "快捷键");
    let global_joined = slice_rect(&global_buf, global_rect).join("\n");
    assert!(global_joined.contains("面板内可直接拖选"));
    assert!(global_joined.contains("直接原生拖选"));
    assert!(!global_joined.contains('…'));
}

#[test]
fn socks5_visual_form_renders_live_preview_enforces_non_loopback_auth_and_prefills() {
    fn render_frame_text(app: &mut App, width: u16, height: u16) -> String {
        use unicode_width::UnicodeWidthStr;
        app.viewport = Rect::new(0, 0, width, height);
        app.tick(app.now);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let sym = buf[(x, y)].symbol();
                out.push_str(sym);
                x += sym.width().max(1) as u16;
            }
            out.push('\n');
        }
        out
    }

    let (_temp, mut app) = test_app("zh-CN");
    app.snapshot.public.ipv4 = crate::dashboard::Reading::Value("203.0.113.88".into());
    app.viewport = Rect::new(0, 0, 120, 36);

    let spec = crate::actions::find("clash.socks.configure").unwrap();
    app.open_form(spec.clone(), super::state::FormMode::Run)
        .unwrap();

    let rendered = render_frame_text(&mut app, 120, 36);
    assert!(rendered.contains("SOCKS5 / 5h 实时连接预览"));
    assert!(rendered.contains("socks5h://cqs:<auto>@203.0.113.88:10808"));
    assert!(rendered.contains("socks5://cqs:<auto>@203.0.113.88:10808"));

    // Switching `socks_auth` to `off` while listening on `0.0.0.0` shows live danger warning and rejects submission.
    app.form.as_mut().unwrap().values[5].replace("off".into());
    let warn_rendered = render_frame_text(&mut app, 120, 36);
    assert!(warn_rendered.contains("非本机监听必须开启认证"));
    assert_eq!(app.submit_form().unwrap_err().key, "error.input");

    // Binding to loopback `127.0.0.1` clears the warning and allows submission.
    app.form.as_mut().unwrap().values[2].replace("127.0.0.1".into());
    let loop_rendered = render_frame_text(&mut app, 120, 36);
    assert!(loop_rendered.contains("socks5h://127.0.0.1:10808"));
    let submit_eff = app.submit_form().unwrap();
    assert!(matches!(submit_eff, Effect::Run(_)));

    // Pure ASCII `plain` theme in `en-US` renders strictly ASCII bytes even with the live preview card.
    let (_temp_en, mut app_en) = test_app("en-US");
    app_en
        .preview_theme("plain", Background::Detected((0x00, 0x00, 0x00)))
        .unwrap();
    app_en
        .open_form(spec.clone(), super::state::FormMode::Run)
        .unwrap();
    let ascii_rendered = render_frame_text(&mut app_en, 120, 36);
    assert!(ascii_rendered.contains("SOCKS5 / 5h Live Preview"));
    assert!(
        ascii_rendered.is_ascii(),
        "non-ASCII character in plain mode:\n{ascii_rendered}"
    );
}

#[test]
fn socks5_transient_secret_modal_reveals_copies_masks_and_never_logs_secrets() {
    fn render_frame_text(app: &mut App, width: u16, height: u16) -> String {
        use unicode_width::UnicodeWidthStr;
        app.viewport = Rect::new(0, 0, width, height);
        app.tick(app.now);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let sym = buf[(x, y)].symbol();
                out.push_str(sym);
                x += sym.width().max(1) as u16;
            }
            out.push('\n');
        }
        out
    }

    let (temp, mut app) = test_app("zh-CN");
    app.snapshot.public.ipv4 = crate::dashboard::Reading::Value("203.0.113.88".into());
    app.viewport = Rect::new(0, 0, 120, 36);

    // 1. Before configuration, Shift+S stays closed and displays a warning toast without touching logs.
    let eff = app
        .handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(matches!(eff, Effect::None));
    assert!(app.socks_secret.is_none());
    assert!(app.logs.is_empty());
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("尚未配置"))
    );

    // 2. Write a configured cqs-socks5 listener into Mixin with special chars in password.
    let secret_pw = "S3cret#Pass!99";
    std::fs::create_dir_all(&app.ctx.paths.clash_data).unwrap();
    std::fs::write(
        &app.ctx.paths.clash_mixin,
        format!(
            "listeners:\n  - name: cqs-socks5\n    type: socks\n    port: 10808\n    listen: 0.0.0.0\n    udp: true\n    proxy: DIRECT\n    users:\n      - username: alice\n        password: \"{secret_pw}\"\n"
        ),
    )
    .unwrap();

    // 3. Shift+S opens the transient credential reveal modal.
    app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(app.socks_secret.is_some());
    let expected_5h = "socks5h://alice:S3cret%23Pass!99@203.0.113.88:10808";
    let expected_5 = "socks5://alice:S3cret%23Pass!99@203.0.113.88:10808";
    let expected_curl = format!("curl -x '{expected_5h}' https://ifconfig.me");

    let revealed = render_frame_text(&mut app, 120, 36);
    assert!(revealed.contains("SOCKS5 / 5h 凭据与连接串"));
    assert!(revealed.contains(secret_pw));
    assert!(revealed.contains(expected_5h));
    assert!(revealed.contains(expected_5));

    // 4. Keys 1/2/3/4 emit exact OSC 52 Copy effects without ever leaking secrets into logs or toast.
    let copy_text = |eff: Effect| match eff {
        Effect::Copy(s) => s,
        other => panic!("expected Effect::Copy, got {other:?}"),
    };
    assert_eq!(
        copy_text(
            app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
                .unwrap()
        ),
        expected_5h
    );
    assert!(app.logs.is_empty());
    assert!(!app.toast.as_ref().unwrap().text.contains("S3cret"));

    assert_eq!(
        copy_text(
            app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE))
                .unwrap()
        ),
        expected_5
    );
    assert_eq!(
        copy_text(
            app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE))
                .unwrap()
        ),
        secret_pw
    );
    assert!(!app.toast.as_ref().unwrap().text.contains(secret_pw));
    assert_eq!(
        copy_text(
            app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE))
                .unwrap()
        ),
        expected_curl
    );
    assert!(app.logs.is_empty());

    // 5. Pressing 'm' masks credentials on screen; pressing 'm' again restores them.
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE))
        .unwrap();
    let masked = render_frame_text(&mut app, 120, 36);
    assert!(!masked.contains(secret_pw));
    assert!(!masked.contains("S3cret%23Pass!99"));
    assert!(masked.contains("************************"));
    assert!(masked.contains("socks5h://alice:********@203.0.113.88:10808"));

    // 6. Pressing Esc clears the transient struct from memory immediately.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(app.socks_secret.is_none());

    // 7. Inside `clash.socks.configure` form, Ctrl+S opens the credential card over the form
    //    and Esc returns cleanly back to the open form without losing edits.
    let spec = crate::actions::find("clash.socks.configure").unwrap();
    app.open_form(spec, super::state::FormMode::Run).unwrap();
    let form_with_hint = render_frame_text(&mut app, 120, 36);
    assert!(form_with_hint.contains("Ctrl+S"));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.socks_secret.is_some());
    assert!(app.form.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(app.socks_secret.is_none());
    assert!(app.form.is_some());

    // 8. Pure ASCII `plain` theme in `en-US` renders strictly ASCII on both 120x36 and 40x16.
    let (_temp_en, mut app_en) = test_app("en-US");
    std::fs::create_dir_all(&app_en.ctx.paths.clash_data).unwrap();
    std::fs::copy(&app.ctx.paths.clash_mixin, &app_en.ctx.paths.clash_mixin).unwrap();
    app_en.snapshot.public.ipv4 = crate::dashboard::Reading::Value("203.0.113.88".into());
    app_en
        .preview_theme("plain", Background::Detected((0x00, 0x00, 0x00)))
        .unwrap();
    app_en
        .handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT))
        .unwrap();
    for (w, h) in [(120, 36), (40, 16)] {
        let rendered = render_frame_text(&mut app_en, w, h);
        assert!(
            rendered.is_ascii(),
            "non-ASCII character in plain mode at {w}x{h}:\n{rendered}"
        );
    }
    drop(temp);
}

fn render_clash_frame_text(app: &mut App, w: u16, h: u16) -> String {
    use unicode_width::UnicodeWidthStr;
    app.viewport = Rect::new(0, 0, w, h);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            x += sym.width().max(1) as u16;
        }
        out.push('\n');
    }
    out
}

#[test]
fn clash_workspace_renders_all_tabs_modals_states_and_pure_ascii_plain_mode() {
    use super::clash::{
        fixture::{sample_probe, sample_snapshot},
        state::{ClashConfirmAction, ClashModal},
    };
    use crate::ops::clash::model::{ClashAvailability, ClashTab};

    let sizes = [(120, 36), (80, 24), (59, 24), (40, 16)];
    for locale in ["zh-CN", "en-US"] {
        let (_temp, mut app) = test_app(locale);
        app.clash.snapshot = sample_snapshot();
        app.clash.exit_probe = Some(sample_probe());
        app.clash.logs = vec![
            "time=\"2026-11-26T16:00:00Z\" level=info msg=\"Start Mihomo\"".into(),
            "time=\"2026-11-26T16:00:02Z\" level=warning msg=\"DNS fallback\"".into(),
        ];
        app.open_clash(ClashTab::Overview, false, false);

        for (w, h) in sizes {
            app.viewport = Rect::new(0, 0, w, h);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            for fullscreen in [false, true] {
                app.clash.fullscreen = fullscreen;
                for tab in ClashTab::ALL {
                    app.clash.set_tab(tab);
                    term.draw(|f| app.draw(f)).unwrap();
                }
            }

            // Embedded & fullscreen logs and fullscreen connection inspector
            app.clash.set_tab(ClashTab::Tools);
            app.clash.tools_log_mode = true;
            term.draw(|f| app.draw(f)).unwrap();
            app.clash.tools_log_fullscreen = true;
            term.draw(|f| app.draw(f)).unwrap();
            app.clash.tools_log_fullscreen = false;
            app.clash.tools_log_mode = false;

            app.clash.set_tab(ClashTab::Connections);
            app.clash.conn_inspector_fullscreen = true;
            term.draw(|f| app.draw(f)).unwrap();
            app.clash.conn_inspector_fullscreen = false;

            // Exercise all 5 local modals
            app.clash.set_tab(ClashTab::Proxies);
            for modal in [
                ClashModal::DelaySettings {
                    url: super::input::InputBuffer::new(
                        "https://www.gstatic.com/generate_204".into(),
                    ),
                    timeout: super::input::InputBuffer::new("5000".into()),
                    active_field: 0,
                    error: None,
                },
                ClashModal::GroupPicker { cursor: 1 },
                ClashModal::Providers {
                    show_rules: false,
                    cursor: 0,
                },
                ClashModal::Providers {
                    show_rules: true,
                    cursor: 0,
                },
                ClashModal::OutboundIp { scroll: 0 },
                ClashModal::Confirm {
                    title: "Confirm".into(),
                    message: "Remove profile?".into(),
                    danger: true,
                    accept: true,
                    action: ClashConfirmAction::CloseAllConnections,
                },
            ] {
                app.clash.modal = Some(modal);
                term.draw(|f| app.draw(f)).unwrap();
            }
            app.clash.modal = None;

            // Exercise every offline / unavailable state
            for avail in [
                ClashAvailability::Loading,
                ClashAvailability::NotInstalled,
                ClashAvailability::Stopped,
                ClashAvailability::ControllerUnconfigured,
                ClashAvailability::ControllerUnreachable("connection refused".into()),
                ClashAvailability::UnknownProcess,
            ] {
                app.clash.snapshot.availability = avail;
                app.clash.set_tab(ClashTab::Overview);
                term.draw(|f| app.draw(f)).unwrap();
                // Profiles & Tools remain usable offline
                app.clash.set_tab(ClashTab::Profiles);
                term.draw(|f| app.draw(f)).unwrap();
                app.clash.set_tab(ClashTab::Tools);
                term.draw(|f| app.draw(f)).unwrap();
            }
            app.clash.snapshot.availability = ClashAvailability::Ready;
        }
    }

    // Plain ASCII theme in en-US must be 100% pure 7-bit ASCII across every Mihomo tab, modal,
    // log/inspector overlay, and offline state.
    let (_temp, mut app) = test_app("en-US");
    app.clash.snapshot = sample_snapshot();
    app.clash.exit_probe = Some(sample_probe());
    app.clash.logs = vec!["level=info msg=\"plain ascii check\"".into()];
    app.preview_theme("plain", Background::Detected((0, 0, 0)))
        .unwrap();
    app.open_clash(ClashTab::Overview, false, false);
    for (w, h) in [(120, 36), (80, 24), (40, 16)] {
        for fullscreen in [false, true] {
            app.clash.fullscreen = fullscreen;
            for tab in ClashTab::ALL {
                app.clash.set_tab(tab);
                let rendered = render_clash_frame_text(&mut app, w, h);
                assert!(
                    rendered.is_ascii(),
                    "non-ASCII character in Mihomo plain mode (fullscreen={fullscreen}) on {tab:?} at {w}x{h}:\n{rendered}"
                );
            }
        }
        app.clash.fullscreen = false;
        // Embedded & fullscreen redacted logs + fullscreen connection inspector in plain mode
        app.clash.set_tab(ClashTab::Tools);
        app.clash.tools_log_mode = true;
        let log_emb = render_clash_frame_text(&mut app, w, h);
        assert!(
            log_emb.is_ascii(),
            "non-ASCII in plain embedded logs at {w}x{h}"
        );
        app.clash.tools_log_fullscreen = true;
        app.clash.filtering = true;
        app.clash.filter = super::input::InputBuffer::new("plain".into());
        let log_full = render_clash_frame_text(&mut app, w, h);
        assert!(
            log_full.is_ascii(),
            "non-ASCII in plain fullscreen logs at {w}x{h}"
        );
        app.clash.filtering = false;
        app.clash.filter.clear();
        app.clash.tools_log_fullscreen = false;
        app.clash.tools_log_mode = false;

        app.clash.set_tab(ClashTab::Connections);
        app.clash.conn_inspector_fullscreen = true;
        let conn_full = render_clash_frame_text(&mut app, w, h);
        assert!(
            conn_full.is_ascii(),
            "non-ASCII in plain conn inspector at {w}x{h}"
        );
        app.clash.conn_inspector_fullscreen = false;

        // Every local ClashModal in plain mode must be pure ASCII
        app.clash.set_tab(ClashTab::Proxies);
        for modal in [
            ClashModal::DelaySettings {
                url: super::input::InputBuffer::new("https://www.gstatic.com/generate_204".into()),
                timeout: super::input::InputBuffer::new("5000".into()),
                active_field: 0,
                error: Some("invalid URL".into()),
            },
            ClashModal::GroupPicker { cursor: 0 },
            ClashModal::Providers {
                show_rules: false,
                cursor: 0,
            },
            ClashModal::Providers {
                show_rules: true,
                cursor: 0,
            },
            ClashModal::OutboundIp { scroll: 0 },
            ClashModal::Confirm {
                title: "Confirm".into(),
                message: "Close all active connections?".into(),
                danger: true,
                accept: true,
                action: ClashConfirmAction::CloseAllConnections,
            },
        ] {
            app.clash.modal = Some(modal);
            let modal_text = render_clash_frame_text(&mut app, w, h);
            assert!(
                modal_text.is_ascii(),
                "non-ASCII character in Mihomo modal plain mode at {w}x{h}:\n{modal_text}"
            );
        }
        app.clash.modal = None;

        // Offline guidance states in plain mode must also be pure ASCII
        for avail in [
            ClashAvailability::Loading,
            ClashAvailability::NotInstalled,
            ClashAvailability::Stopped,
            ClashAvailability::ControllerUnconfigured,
            ClashAvailability::ControllerUnreachable("connection refused".into()),
            ClashAvailability::UnknownProcess,
        ] {
            app.clash.snapshot.availability = avail;
            app.clash.set_tab(ClashTab::Overview);
            let off_text = render_clash_frame_text(&mut app, w, h);
            assert!(
                off_text.is_ascii(),
                "non-ASCII character in Mihomo offline plain mode at {w}x{h}:\n{off_text}"
            );
        }
        app.clash.snapshot.availability = ClashAvailability::Ready;

        // Help modal in plain mode must also be pure ASCII
        app.help = true;
        let help_text = render_clash_frame_text(&mut app, w, h);
        app.help = false;
        assert!(
            help_text.is_ascii(),
            "non-ASCII character in Mihomo help plain mode at {w}x{h}:\n{help_text}"
        );
    }
}

#[test]
fn clash_workspace_keyboard_mouse_delay_sorting_and_modals_work_end_to_end() {
    use super::clash::{
        fixture::sample_snapshot,
        state::{ClashModal, ConnectionScope, ProxySort},
    };
    use crate::ops::clash::{model::ClashTab, monitor::ClashMonitorCmd};

    let (_temp, mut app) = test_app("zh-CN");
    app.clash.snapshot = sample_snapshot();
    app.open_clash(ClashTab::Overview, false, false);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;

    // 1. Overview: `m` cycles outbound mode -> emits SetMode("global")
    let eff = app.handle_key(key(KeyCode::Char('m'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::SetMode { ref mode }) if mode == "global"
    ));

    // 2. Overview: `N` triggers explicit manual exit IP probe; `w` copies WebUI URL
    let eff = app.handle_key(key(KeyCode::Char('N'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::ProbeExit)
    ));
    assert!(app.clash.exit_probing);

    // 2a. Overview `u` deduplicates in-flight profile updates and removes only the completed name
    let u1 = app.handle_key(key(KeyCode::Char('u'))).unwrap();
    assert!(matches!(
        u1,
        super::state::Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { ref name }) if name == "PrimarySub"
    ));
    let u2 = app.handle_key(key(KeyCode::Char('u'))).unwrap();
    assert!(matches!(u2, super::state::Effect::None));
    app.clash.busy_profiles.insert("other-profile".into());
    app.clash.apply_event(
        crate::ops::clash::monitor::ClashEvent::ActionDone {
            tag: crate::ops::clash::monitor::ClashActionTag::UpdateProfile("PrimarySub".into()),
            result: Ok("reloaded:PrimarySub".into()),
        },
        &app.ctx.i18n,
    );
    assert!(!app.clash.busy_profiles.contains("PrimarySub"));
    assert!(app.clash.busy_profiles.contains("other-profile"));
    app.clash.busy_profiles.clear();

    let ui_eff = app.handle_key(key(KeyCode::Char('w'))).unwrap();
    assert!(matches!(
        ui_eff,
        super::state::Effect::Copy(ref url) if url == "http://127.0.0.1:9090/ui/"
    ));
    let rendered_ov = render_clash_frame_text(&mut app, 120, 36);
    assert!(
        rendered_ov.contains("Zashboard v3.30.0")
            && rendered_ov.contains("http://127.0.0.1:9090/ui/"),
        "Overview must display WebUI running summary:\n{rendered_ov}"
    );
    assert!(
        rendered_ov.contains("[p] 切换") && !rendered_ov.contains("[3] 切换"),
        "Overview profile card must advertise [p] instead of misleading [3]:\n{rendered_ov}"
    );
    // `p` on Overview switches directly to Profiles in both embedded and fullscreen modes
    app.handle_key(key(KeyCode::Char('p'))).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Profiles);
    // `{` / `}` and `Alt+Left` / `Alt+Right` switch Mihomo tabs cleanly in embedded mode
    app.handle_key(key(KeyCode::Char('{'))).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Proxies);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.clash.tab, ClashTab::Overview);
    // Embedded `Tab` / `BackTab` cycle CqShell sidebar categories (never trapped inside Mihomo)
    app.handle_key(key(KeyCode::Tab)).unwrap();
    assert!(!app.is_clash_view());
    app.handle_key(key(KeyCode::BackTab)).unwrap();
    assert!(app.is_clash_view());
    assert_eq!(app.clash.tab, ClashTab::Overview);
    // When WebUI is not installed, `w` opens the panel install form directly
    app.clash.snapshot.summary.panel_installed = false;
    app.clash.snapshot.summary.panel_url = None;
    app.handle_key(key(KeyCode::Char('w'))).unwrap();
    assert!(app.form.is_some());
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.form.is_none());
    app.clash.snapshot.summary.panel_installed = true;
    app.clash.snapshot.summary.panel_url = Some("http://127.0.0.1:9090/ui/".into());

    // 2b. Overview mouse click on connections_card jumps straight to Connections; back to Overview
    let regions = LayoutRegions::compute(area, &app);
    if let Some(super::clash::layout::ClashBodyGeom::Overview(ov)) =
        regions.clash.as_ref().map(|g| &g.body)
    {
        app.handle_mouse(
            click(ov.connections_card.x + 2, ov.connections_card.y + 1),
            area,
        )
        .unwrap();
        assert_eq!(app.clash.tab, ClashTab::Connections);
    }
    app.clash.set_tab(ClashTab::Overview);

    // 3. Alt+2 jumps to Proxies tab; test sort cycle (`o`), hide timeouts (`h`), fastest pick (`f`)
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.clash.tab, ClashTab::Proxies);
    assert_eq!(app.clash.proxy_sort, ProxySort::Default);

    app.handle_key(key(KeyCode::Char('o'))).unwrap();
    assert_eq!(app.clash.proxy_sort, ProxySort::Delay);
    let sorted = app.clash.filtered_nodes();
    assert_eq!(sorted[0].name, "HK-HY2-01"); // 68 ms fastest
    assert_eq!(sorted[1].name, "JP-Opt-02"); // 112 ms
    assert!(sorted.last().unwrap().delay_ms.is_none()); // untested/timeout at bottom

    // Flip delay order (`O`): reachable reverses, unreachable still pinned at bottom
    app.handle_key(key(KeyCode::Char('O'))).unwrap();
    let flipped = app.clash.filtered_nodes();
    assert_eq!(flipped[0].name, "SG-AnyTLS"); // 218 ms slowest reachable first
    assert!(flipped.last().unwrap().delay_ms.is_none());

    // Hide timeout (`h`) hides `US-LA-Dead` (tested && delay_ms=None)
    let before_hide = app.clash.filtered_nodes().len();
    app.handle_key(key(KeyCode::Char('h'))).unwrap();
    assert!(app.clash.hide_timeout);
    assert_eq!(app.clash.filtered_nodes().len(), before_hide - 1);
    assert!(
        !app.clash
            .filtered_nodes()
            .iter()
            .any(|n| n.name == "US-LA-Dead")
    );

    // Locate active node (`c`) moves cursor to `JP-Opt-02`
    app.handle_key(key(KeyCode::Char('c'))).unwrap();
    assert_eq!(app.clash.selected_node().unwrap().name, "JP-Opt-02");

    // Fastest node (`f`) picks `HK-HY2-01` (68 ms)
    let eff = app.handle_key(key(KeyCode::Char('f'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::SelectProxy { ref group, ref node })
        if group == "Proxy" && node == "HK-HY2-01"
    ));

    // Batch group delay test (`t`) & single node ping (`p`)
    let eff = app.handle_key(key(KeyCode::Char('t'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::TestGroupDelay { ref group, .. })
        if group == "Proxy"
    ));
    let eff = app.handle_key(key(KeyCode::Char('p'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::TestNodeDelay { ref node, .. })
        if node == "JP-Opt-02"
    ));

    // Switch policy group with `.` / `,` (keeps `Tab` free for sidebar in embedded mode)
    app.handle_key(key(KeyCode::Char('.'))).unwrap();
    assert_eq!(app.clash.current_group().unwrap().name, "Auto");
    app.handle_key(key(KeyCode::Char(','))).unwrap();
    assert_eq!(app.clash.current_group().unwrap().name, "Proxy");
    app.handle_key(key(KeyCode::Char('.'))).unwrap();
    assert_eq!(app.clash.current_group().unwrap().name, "Auto");
    // Embedded `Tab` on Proxies still leaves to the next sidebar category
    app.handle_key(key(KeyCode::Tab)).unwrap();
    assert!(!app.is_clash_view());
    app.handle_key(key(KeyCode::BackTab)).unwrap();
    assert!(app.is_clash_view());
    assert_eq!(app.clash.tab, ClashTab::Proxies);
    let eff = app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::SelectProxy { ref group, .. })
        if group == "Auto"
    ));
    let eff = app.handle_key(key(KeyCode::Char('x'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::UnfixGroup { ref group })
        if group == "Auto"
    ));

    // GroupPicker (`g`) + DelaySettings (`e`) modal validation & overlay priority alignment
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    assert!(matches!(
        app.clash.modal,
        Some(ClashModal::GroupPicker { .. })
    ));
    assert_eq!(app.overlay_kind(), 13);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.modal.is_none());

    // DelaySettings (`d` alias + mouse field focus) & Providers modal mouse sub-tab switch
    app.handle_key(key(KeyCode::Char('d'))).unwrap();
    assert!(matches!(
        app.clash.modal,
        Some(ClashModal::DelaySettings {
            active_field: 0,
            ..
        })
    ));
    let delay_geom = super::clash::layout::ClashModalGeometry::compute(
        area,
        app.clash.modal.as_ref().unwrap(),
        &app.clash,
        &app.ctx.i18n,
    );
    let field1 = delay_geom.field_rects[1];
    app.handle_mouse(click(field1.x + 1, field1.y + 1), area)
        .unwrap();
    assert!(matches!(
        app.clash.modal,
        Some(ClashModal::DelaySettings {
            active_field: 1,
            ..
        })
    ));
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(app.clash.modal.is_none());

    app.handle_key(key(KeyCode::Char('P'))).unwrap();
    let prov_geom = super::clash::layout::ClashModalGeometry::compute(
        area,
        app.clash.modal.as_ref().unwrap(),
        &app.clash,
        &app.ctx.i18n,
    );
    let rules_sub = prov_geom.subtab_rules.unwrap();
    app.handle_mouse(click(rules_sub.x + 1, rules_sub.y), area)
        .unwrap();
    assert!(matches!(
        app.clash.modal,
        Some(ClashModal::Providers {
            show_rules: true,
            ..
        })
    ));
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.modal.is_none());

    // 4. Profiles tab (Alt+3): switch+reload (`Enter`), update (`u`), update all (`U`), confirm delete (`d`)
    app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.clash.tab, ClashTab::Profiles);
    let eff = app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::SwitchProfile { ref name })
        if name == "PrimarySub"
    ));
    let eff = app.handle_key(key(KeyCode::Char('u'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { ref name })
        if name == "PrimarySub"
    ));
    // While a single profile update is in flight, `U` is suppressed to avoid clash_lock collisions.
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('U'))).unwrap(),
        super::state::Effect::None
    ));
    app.clash.apply_event(
        crate::ops::clash::monitor::ClashEvent::ActionDone {
            tag: crate::ops::clash::monitor::ClashActionTag::UpdateProfile("PrimarySub".into()),
            result: Ok("updated:PrimarySub".into()),
        },
        &app.ctx.i18n,
    );
    let eff = app.handle_key(key(KeyCode::Char('U'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::UpdateAllProfiles)
    ));
    app.handle_key(key(KeyCode::Char('d'))).unwrap();
    assert!(matches!(app.clash.modal, Some(ClashModal::Confirm { .. })));
    let eff = app.handle_key(key(KeyCode::Char('y'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::RemoveProfile { ref name })
        if name == "PrimarySub"
    ));

    // 5. Connections tab (Alt+4): scope cycle (`a`), facet cycle (`.` / `,` / `f`), close single (`x`), close all (`X`)
    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.clash.tab, ClashTab::Connections);
    assert_eq!(app.clash.conn_scope, ConnectionScope::Running);
    app.handle_key(key(KeyCode::Char('a'))).unwrap();
    assert_eq!(app.clash.conn_scope, ConnectionScope::Closed);
    assert_eq!(app.clash.filtered_connections().len(), 1);
    app.handle_key(key(KeyCode::Char('a'))).unwrap();
    assert_eq!(app.clash.conn_scope, ConnectionScope::All);
    assert_eq!(app.clash.filtered_connections().len(), 3);
    app.handle_key(key(KeyCode::Char('a'))).unwrap();
    assert_eq!(app.clash.conn_scope, ConnectionScope::Running);

    app.handle_key(key(KeyCode::Char('f'))).unwrap();
    let first_facet = app.clash.conn_facet.clone();
    assert!(first_facet.is_some());
    app.handle_key(key(KeyCode::Char(','))).unwrap();
    assert!(app.clash.conn_facet.is_none());
    app.handle_key(key(KeyCode::Char('.'))).unwrap();
    assert_eq!(app.clash.conn_facet, first_facet);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.conn_facet.is_none());

    let eff = app.handle_key(key(KeyCode::Char('x'))).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::CloseConnection { .. })
    ));
    // `X` opens close-all confirmation modal
    app.handle_key(key(KeyCode::Char('X'))).unwrap();
    assert!(matches!(app.clash.modal, Some(ClashModal::Confirm { .. })));
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.modal.is_none());

    // 6. Focus fullscreen toggle (`z`): numbered tab pills, uniform `Tab`/`BackTab` tab cycling,
    // `[` keeps toggling sidebar even on Proxies, and clicking top tab pill via mouse works.
    app.handle_key(key(KeyCode::Char('z'))).unwrap();
    assert!(app.clash.fullscreen);
    let rendered_fs = render_clash_frame_text(&mut app, 120, 36);
    assert!(
        rendered_fs.contains("1 仪表盘")
            && rendered_fs.contains("2 代理")
            && rendered_fs.contains("3 配置"),
        "Fullscreen tab bar must display 1-6 shortcuts:\n{rendered_fs}"
    );
    app.handle_key(key(KeyCode::Tab)).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Rules);
    // Rules facet cycling uses `.` / `,` / `f`, never hijacking fullscreen `Tab`
    app.handle_key(key(KeyCode::Char('f'))).unwrap();
    assert!(app.clash.rule_facet.is_some());
    app.handle_key(key(KeyCode::Char(','))).unwrap();
    assert!(app.clash.rule_facet.is_none());
    app.handle_key(key(KeyCode::BackTab)).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Connections);
    app.handle_key(key(KeyCode::Char('2'))).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Proxies);
    let group_before = app.clash.group_cursor;
    app.handle_key(key(KeyCode::Char('['))).unwrap();
    assert_eq!(app.clash.group_cursor, group_before);
    app.sidebar_overlay = false;
    app.sidebar_collapsed = false;
    let regions = LayoutRegions::compute(area, &app);
    let cl_geom = regions.clash.as_ref().unwrap();
    let rules_pill = cl_geom
        .tab_pills
        .iter()
        .find(|p| p.tab == ClashTab::Rules)
        .unwrap();
    app.handle_mouse(click(rules_pill.rect.x + 1, rules_pill.rect.y), area)
        .unwrap();
    assert_eq!(app.clash.tab, ClashTab::Rules);

    // 7. Tools tab (fullscreen `6`): toggle embedded redacted logs (`l`), copy (`y`),
    // fullscreen (`f`) + inline search (`/`)
    app.handle_key(key(KeyCode::Char('6'))).unwrap();
    assert_eq!(app.clash.tab, ClashTab::Tools);
    app.clash.logs = vec![
        "level=info msg=\"matched-line\"".into(),
        "level=warning msg=\"other-line\"".into(),
    ];
    let eff = app.handle_key(key(KeyCode::Char('l'))).unwrap();
    assert!(app.clash.tools_log_mode);
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::FetchLogs)
    ));
    let copy_eff = app.handle_key(key(KeyCode::Char('y'))).unwrap();
    assert!(matches!(
        copy_eff,
        super::state::Effect::Copy(ref text) if text.contains("matched-line")
    ));
    app.handle_key(key(KeyCode::Char('f'))).unwrap();
    assert!(app.clash.tools_log_fullscreen);
    app.handle_key(key(KeyCode::Char('/'))).unwrap();
    assert!(app.clash.filtering);
    for ch in "matched".chars() {
        app.handle_key(key(KeyCode::Char(ch))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(!app.clash.filtering);
    assert_eq!(app.clash.filtered_logs().len(), 1);
    // First Esc clears filter while staying in fullscreen; second Esc exits fullscreen
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.filter.text.is_empty());
    assert!(app.clash.tools_log_fullscreen);
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(!app.clash.tools_log_fullscreen);

    // 8. CLI parsing for `cqs mihomo`
    let i18n = crate::i18n::I18n::load(Some("en-US"), None).unwrap();
    let m = crate::cli::command(&i18n)
        .try_get_matches_from(["cqs", "mihomo", "--tab", "proxies", "--embedded"])
        .unwrap();
    let (sub, sub_m) = m.subcommand().unwrap();
    assert_eq!(sub, "mihomo");
    assert_eq!(
        sub_m.get_one::<String>("tab").map(String::as_str),
        Some("proxies")
    );
    assert!(sub_m.get_flag("embedded"));
}

#[test]
fn clash_help_modal_renders_without_clipping_and_scrolls_accurately() {
    use super::clash::fixture::sample_snapshot;
    use crate::ops::clash::model::ClashTab;
    use unicode_width::UnicodeWidthStr;

    fn modal_slice(app: &mut App, w: u16, h: u16, title_needle: &str) -> String {
        app.viewport = Rect::new(0, 0, w, h);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer();
        for y in 0..h {
            for x in 0..w {
                if buf[(x, y)].symbol() == "╭"
                    && let Some(rx) = ((x + 1)..w).find(|&c| buf[(c, y)].symbol() == "╮")
                    && rx - x >= 24
                    && let Some(by) = ((y + 1)..h)
                        .find(|&r| buf[(x, r)].symbol() == "╰" && buf[(rx, r)].symbol() == "╯")
                {
                    let mut top = String::new();
                    let mut cx = x;
                    while cx <= rx {
                        let s = buf[(cx, y)].symbol();
                        top.push_str(s);
                        cx += s.width().max(1) as u16;
                    }
                    if top.contains(title_needle) {
                        let mut out = String::new();
                        for ry in y..=by {
                            let mut cx = x;
                            while cx <= rx {
                                let s = buf[(cx, ry)].symbol();
                                out.push_str(s);
                                cx += s.width().max(1) as u16;
                            }
                            out.push('\n');
                        }
                        return out;
                    }
                }
            }
        }
        panic!("Mihomo help modal with title {title_needle:?} not found at {w}x{h}");
    }

    for (locale, title) in [
        ("zh-CN", "Mihomo 可视化面板快捷键"),
        ("en-US", "Mihomo Workspace Shortcuts"),
    ] {
        let (_temp, mut app) = test_app(locale);
        app.clash.snapshot = sample_snapshot();
        app.open_clash(ClashTab::Proxies, false, false);
        app.help = true;

        for (w, h) in [(120, 36), (80, 24), (59, 24), (40, 16)] {
            let text = modal_slice(&mut app, w, h, title);
            assert!(
                !text.contains('…'),
                "Mihomo help modal clipped with ellipsis at {w}x{h} ({locale}):\n{text}"
            );
            let max_s = super::render::help_max_scroll(&app, Rect::new(0, 0, w, h));
            app.help_scroll = max_s;
            let scrolled = modal_slice(&mut app, w, h, title);
            assert!(
                !scrolled.contains('…'),
                "Mihomo scrolled help modal clipped at {w}x{h} ({locale}):\n{scrolled}"
            );
            app.help_scroll = 0;
        }
    }
}

#[test]
fn clash_traffic_atlas_modes_fullscreen_pause_mouse_and_pure_ascii_work() {
    use super::clash::{
        fixture::sample_snapshot,
        layout::ClashBodyGeom,
        state::{TrafficBreakdownKind, TrafficGraphMode, TrafficScale, TrafficWindow},
    };
    use crate::ops::clash::model::ClashTab;

    let (_temp, mut app) = test_app("zh-CN");
    app.clash.snapshot = sample_snapshot();
    app.open_clash(ClashTab::Overview, false, false);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;

    // 1. Hero traffic atlas renders across every responsive tier including compact 80x24 & 40x16.
    for (w, h) in [(120, 36), (100, 30), (80, 24), (59, 24), (40, 16)] {
        let frame = render_clash_frame_text(&mut app, w, h);
        assert!(
            frame.contains("实时流量图谱"),
            "Overview traffic atlas must remain visible at {w}x{h}:\n{frame}"
        );
    }

    // 2. Wide Overview shows live Exit Share mini-atlas alongside the Braille waveform.
    let wide = render_clash_frame_text(&mut app, 120, 36);
    assert!(
        wide.contains("出口节点分布"),
        "Wide overview missing exit share:\n{wide}"
    );
    assert!(
        wide.contains("JP-Opt-02"),
        "Wide overview missing exit node:\n{wide}"
    );
    assert!(
        wide.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
        "Wide overview waveform should render Unicode Braille glyphs:\n{wide}"
    );

    // 3. Keyboard controls on Overview: g (mode), t (window), y (scale), Space (pause/resume), v (fullscreen)
    assert_eq!(app.clash.traffic_mode, TrafficGraphMode::Mirror);
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    assert_eq!(app.clash.traffic_mode, TrafficGraphMode::Overlay);
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    assert_eq!(app.clash.traffic_mode, TrafficGraphMode::Stacked);
    app.handle_key(key(KeyCode::Char('g'))).unwrap();
    assert_eq!(app.clash.traffic_mode, TrafficGraphMode::Mirror);

    assert_eq!(app.clash.traffic_window, TrafficWindow::Sec60);
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    assert_eq!(app.clash.traffic_window, TrafficWindow::Min3);
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    assert_eq!(app.clash.traffic_window, TrafficWindow::Min5);
    app.handle_key(key(KeyCode::Char('t'))).unwrap();
    assert_eq!(app.clash.traffic_window, TrafficWindow::Sec60);

    assert_eq!(app.clash.traffic_scale, TrafficScale::Linear);
    app.handle_key(key(KeyCode::Char('y'))).unwrap();
    assert_eq!(app.clash.traffic_scale, TrafficScale::Log);
    app.handle_key(key(KeyCode::Char('y'))).unwrap();
    assert_eq!(app.clash.traffic_scale, TrafficScale::Linear);

    // Pause freezes displayed snapshot even while live snapshot changes underneath.
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    assert!(app.clash.is_traffic_paused());
    let frozen_down = app.clash.traffic_snapshot().down_speed;
    app.clash.snapshot.down_speed = 99_999_999;
    assert_eq!(app.clash.traffic_snapshot().down_speed, frozen_down);
    app.handle_key(key(KeyCode::Char(' '))).unwrap();
    assert!(!app.clash.is_traffic_paused());
    assert_eq!(app.clash.traffic_snapshot().down_speed, 99_999_999);
    app.clash.snapshot.down_speed = frozen_down;

    // 4. Mouse click on Overview traffic chips & plot area
    let regions = LayoutRegions::compute(area, &app);
    let Some(ClashBodyGeom::Overview(ov)) = regions.clash.as_ref().map(|g| &g.body) else {
        panic!("expected Overview geometry");
    };
    let mode_chip = ov.traffic_mode_chip.expect("expected traffic_mode_chip");
    app.handle_mouse(click(mode_chip.x + 1, mode_chip.y), area)
        .unwrap();
    assert_eq!(app.clash.traffic_mode, TrafficGraphMode::Overlay);

    let win_chip = ov
        .traffic_window_chip
        .expect("expected traffic_window_chip");
    app.handle_mouse(click(win_chip.x + 1, win_chip.y), area)
        .unwrap();
    assert_eq!(app.clash.traffic_window, TrafficWindow::Min3);

    // Click waveform plot expands to fullscreen Traffic Atlas
    app.handle_mouse(click(ov.traffic_plot.x + 2, ov.traffic_plot.y + 1), area)
        .unwrap();
    assert!(app.clash.traffic_fullscreen);

    // Fullscreen Traffic Atlas renders waveform + distribution panels cleanly across all sizes
    for (w, h) in [(120, 36), (100, 30), (80, 24), (59, 24), (40, 16)] {
        let fs_text = render_clash_frame_text(&mut app, w, h);
        assert!(
            fs_text.contains("实时流量图谱"),
            "Fullscreen traffic atlas header missing at {w}x{h}:\n{fs_text}"
        );
    }
    let fs_wide = render_clash_frame_text(&mut app, 120, 36);
    assert!(fs_wide.contains("出口节点分布"));
    assert!(fs_wide.contains("分流规则分布"));
    assert!(fs_wide.contains("实时热门连接"));

    // Keyboard & mouse controls inside Fullscreen Traffic Atlas
    assert_eq!(app.clash.traffic_breakdown, TrafficBreakdownKind::Exit);
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    assert_eq!(app.clash.traffic_breakdown, TrafficBreakdownKind::Rule);
    app.handle_key(key(KeyCode::Char('b'))).unwrap();
    assert_eq!(app.clash.traffic_breakdown, TrafficBreakdownKind::Process);
    let fs_proc = render_clash_frame_text(&mut app, 120, 36);
    assert!(fs_proc.contains("进程流量分布"));

    let fs_regions = LayoutRegions::compute(area, &app);
    let Some(ClashBodyGeom::TrafficFullscreen(tf)) = fs_regions.clash.as_ref().map(|g| &g.body)
    else {
        panic!("expected TrafficFullscreen geometry");
    };
    let close_btn = tf.close_btn.expect("expected close_btn");
    app.handle_mouse(click(close_btn.x + 1, close_btn.y), area)
        .unwrap();
    assert!(!app.clash.traffic_fullscreen);

    // 5. Pure ASCII `plain` theme in `en-US` must be 100% pure 7-bit ASCII across every mode,
    // scale, paused state, fullscreen Traffic Atlas, and viewport size.
    let (_temp_en, mut app_en) = test_app("en-US");
    app_en.clash.snapshot = sample_snapshot();
    app_en
        .preview_theme("plain", Background::Detected((0, 0, 0)))
        .unwrap();
    app_en.open_clash(ClashTab::Overview, false, false);

    for (w, h) in [(120, 36), (80, 24), (59, 24), (40, 16)] {
        for mode in [
            TrafficGraphMode::Mirror,
            TrafficGraphMode::Overlay,
            TrafficGraphMode::Stacked,
        ] {
            app_en.clash.traffic_mode = mode;
            for scale in [TrafficScale::Linear, TrafficScale::Log] {
                app_en.clash.traffic_scale = scale;
                for fs in [false, true] {
                    app_en.clash.traffic_fullscreen = fs;
                    let rendered = render_clash_frame_text(&mut app_en, w, h);
                    assert!(
                        rendered.is_ascii(),
                        "non-ASCII character in plain traffic atlas (mode={mode:?}, scale={scale:?}, fs={fs}) at {w}x{h}:\n{rendered}"
                    );
                }
            }
        }
    }
    app_en.clash.toggle_traffic_pause();
    app_en.clash.traffic_fullscreen = true;
    let paused_ascii = render_clash_frame_text(&mut app_en, 120, 36);
    assert!(paused_ascii.is_ascii());
    assert!(paused_ascii.contains("PAUSED"));
}

#[test]
fn clash_outbound_ip_detection_modal_hide_ip_copy_and_generation_guard_work() {
    use super::clash::{
        fixture::{sample_probe, sample_probe_quality_failed, sample_snapshot},
        layout::{ClashBodyGeom, ClashModalGeometry},
        state::ClashModal,
    };
    use crate::{
        config::Preferences,
        ops::clash::{
            model::{
                ClashTab, IpQuality, IpQualityOutcome, IpQualitySource, IpType, OutboundIpInfo,
            },
            monitor::{ClashEvent, ClashMonitorCmd},
        },
    };

    let (_temp, mut app) = test_app("zh-CN");
    app.clash.snapshot = sample_snapshot();
    app.open_clash(ClashTab::Overview, false, false);
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;

    // 1. Progressive event pipeline & stale generation protection
    app.clash.apply_event(
        ClashEvent::OutboundProbeStarted {
            generation: 2,
            manual: true,
        },
        &app.ctx.i18n,
    );
    assert!(app.clash.exit_probing);
    assert!(app.clash.is_animating());

    // Stale generation=1 event must be ignored
    app.clash.apply_event(
        ClashEvent::OutboundIpResolved {
            generation: 1,
            result: Ok(OutboundIpInfo {
                ip: "198.51.100.1".into(),
                country: "US".into(),
                ip_source: "ip.sb".into(),
                fallback_org: "Stale ISP".into(),
            }),
            manual: true,
        },
        &app.ctx.i18n,
    );
    assert!(app.clash.exit_probe.is_none());

    // Generation=2 IP arrives -> immediately populates IP with quality=Loading
    let toast = app.clash.apply_event(
        ClashEvent::OutboundIpResolved {
            generation: 2,
            result: Ok(OutboundIpInfo {
                ip: "203.0.113.106".into(),
                country: "SG".into(),
                ip_source: "cloudflare.com".into(),
                fallback_org: String::new(),
            }),
            manual: true,
        },
        &app.ctx.i18n,
    );
    assert!(toast.is_some());
    assert!(!app.clash.exit_probing);
    assert!(app.clash.exit_probe.as_ref().unwrap().is_quality_loading());
    assert!(app.clash.is_animating());

    // Generation=2 quality & latency arrive
    app.clash.apply_event(
        ClashEvent::OutboundQualityResolved {
            generation: 2,
            ip: "203.0.113.106".into(),
            outcome: IpQualityOutcome::Ready(IpQuality {
                ip: "203.0.113.106".into(),
                source: IpQualitySource::IpApiCom,
                ip_type: IpType::Hosting,
                organization: Some("Example Hosting".into()),
                asn: Some(64502),
                is_proxy: true,
                is_vpn: false,
                is_tor: false,
                is_abuser: false,
            }),
        },
        &app.ctx.i18n,
    );
    app.clash.apply_event(
        ClashEvent::OutboundLatencyResolved {
            generation: 2,
            google_ms: Some(118),
            github_ms: Some(205),
        },
        &app.ctx.i18n,
    );
    assert!(!app.clash.is_animating());

    // 2. Overview status card renders country badge, IP, type/quality chips, org, and WebUI
    let ov_text = render_clash_frame_text(&mut app, 120, 36);
    for needle in [
        "SG",
        "203.0.113.106",
        "机房",
        "质量 普通",
        "Example Hosting",
        "AS64502",
        "Zashboard v3.30.0",
    ] {
        assert!(
            ov_text.contains(needle),
            "Overview status card missing {needle:?}:\n{ov_text}"
        );
    }

    // 3. Mouse click on country badge triggers fresh ProbeExit; clicking IP row opens detail modal
    let regions = LayoutRegions::compute(area, &app);
    let Some(ClashBodyGeom::Overview(ov)) = regions.clash.as_ref().map(|g| &g.body) else {
        panic!("expected Overview geometry");
    };
    let badge = ov.outbound_badge_hit.expect("outbound_badge_hit");
    let eff = app.handle_mouse(click(badge.x + 1, badge.y), area).unwrap();
    assert!(matches!(
        eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::ProbeExit)
    ));
    app.clash.exit_probing = false;

    let ip_hit = ov.outbound_ip_hit.expect("outbound_ip_hit");
    app.handle_mouse(click(ip_hit.x + 1, ip_hit.y), area)
        .unwrap();
    assert!(matches!(
        app.clash.modal,
        Some(ClashModal::OutboundIp { .. })
    ));

    // 4. Detail modal renders FlClash-aligned sections, copy actions, and hide-IP persistence
    let modal_text = render_clash_frame_text(&mut app, 120, 36);
    for needle in [
        "出站 IP",
        "依赖第三方 API",
        "基础信息",
        "203.0.113.106",
        "普通",
        "机房",
        "[代理]",
        "网络",
        "Example Hosting",
        "AS64502",
        "ip-api.com",
        "隐藏 IP",
    ] {
        assert!(
            modal_text.contains(needle),
            "OutboundIp modal missing {needle:?}:\n{modal_text}"
        );
    }

    // Copy IP (`1`), Org (`2`), ASN (`3`)
    let copy_ip = app.handle_key(key(KeyCode::Char('1'))).unwrap();
    assert!(matches!(copy_ip, super::state::Effect::Copy(ref v) if v == "203.0.113.106"));
    let copy_org = app.handle_key(key(KeyCode::Char('2'))).unwrap();
    assert!(matches!(copy_org, super::state::Effect::Copy(ref v) if v == "Example Hosting"));
    let copy_asn = app.handle_key(key(KeyCode::Char('3'))).unwrap();
    assert!(matches!(copy_asn, super::state::Effect::Copy(ref v) if v == "AS64502"));

    // Toggle Hide IP (`h`) masks IP to `*****` in modal & overview and persists to Preferences
    app.handle_key(key(KeyCode::Char('h'))).unwrap();
    assert!(app.clash.hide_ip);
    let saved = Preferences::read(&app.ctx.paths.preferences, true).unwrap();
    assert_eq!(saved.clash_hide_ip, Some(true));
    let hidden_modal = render_clash_frame_text(&mut app, 120, 36);
    assert!(hidden_modal.contains("*****"));
    assert!(!hidden_modal.contains("203.0.113.106"));
    // Copying IP while hidden still copies the real IP without exposing it in toast
    let copy_hidden = app.handle_key(key(KeyCode::Char('c'))).unwrap();
    assert!(matches!(copy_hidden, super::state::Effect::Copy(ref v) if v == "203.0.113.106"));
    assert!(
        !app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("203.0.113.106"))
    );

    // Retry inside modal (`r`) emits ProbeExit
    let retry_eff = app.handle_key(key(KeyCode::Char('r'))).unwrap();
    assert!(matches!(
        retry_eff,
        super::state::Effect::ClashCmd(ClashMonitorCmd::ProbeExit)
    ));
    app.clash.exit_probing = false;

    // Close modal via Esc and verify Overview also masks IP
    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(app.clash.modal.is_none());
    let hidden_ov = render_clash_frame_text(&mut app, 120, 36);
    assert!(hidden_ov.contains("*****"));
    assert!(!hidden_ov.contains("203.0.113.106"));

    // 5. Quality failure view renders per-source diagnostics and scrolls cleanly at 40x16
    app.clash.hide_ip = false;
    app.clash.exit_probe = Some(sample_probe_quality_failed());
    app.handle_key(key(KeyCode::Char('i'))).unwrap();
    let failed_text = render_clash_frame_text(&mut app, 120, 36);
    for needle in [
        "类型查询失败",
        "各来源",
        "ident.me",
        "出站 IP 不一致",
        "proxycheck.io",
        "限流",
    ] {
        assert!(
            failed_text.contains(needle),
            "Failed quality modal missing {needle:?}:\n{failed_text}"
        );
    }
    let _ = render_clash_frame_text(&mut app, 40, 16);
    assert!(app.clash.max_outbound_modal_scroll.get() > 0);
    app.handle_key(key(KeyCode::End)).unwrap();
    let scrolled_40 = render_clash_frame_text(&mut app, 40, 16);
    assert!(scrolled_40.contains("隐藏 IP"));

    // Click close button via shared ClashModalGeometry
    let mgeom = ClashModalGeometry::compute(
        area,
        app.clash.modal.as_ref().unwrap(),
        &app.clash,
        &app.ctx.i18n,
    );
    let close_btn = mgeom.outbound_close_btn.expect("outbound_close_btn");
    app.handle_mouse(click(close_btn.x + 1, close_btn.y), area)
        .unwrap();
    assert!(app.clash.modal.is_none());

    // 6. Plain ASCII theme in en-US must be 100% pure 7-bit ASCII across all OutboundIp states
    let (_temp_en, mut app_en) = test_app("en-US");
    app_en.clash.snapshot = sample_snapshot();
    app_en
        .preview_theme("plain", Background::Detected((0, 0, 0)))
        .unwrap();
    app_en.open_clash(ClashTab::Overview, false, false);
    app_en.clash.modal = Some(ClashModal::OutboundIp { scroll: 0 });
    for (w, h) in [(120, 36), (80, 24), (40, 16)] {
        // Ready
        app_en.clash.exit_probe = Some(sample_probe());
        app_en.clash.exit_probing = false;
        app_en.clash.exit_probe_error = None;
        let t = render_clash_frame_text(&mut app_en, w, h);
        assert!(
            t.is_ascii(),
            "non-ASCII in Ready OutboundIp modal at {w}x{h}:\n{t}"
        );

        // Quality failed
        app_en.clash.exit_probe = Some(sample_probe_quality_failed());
        let t = render_clash_frame_text(&mut app_en, w, h);
        assert!(
            t.is_ascii(),
            "non-ASCII in Failed-quality OutboundIp modal at {w}x{h}:\n{t}"
        );

        // IP failed
        app_en.clash.exit_probe = None;
        app_en.clash.exit_probe_error = Some("all outbound IP sources failed".into());
        let t = render_clash_frame_text(&mut app_en, w, h);
        assert!(
            t.is_ascii(),
            "non-ASCII in IP-failed OutboundIp modal at {w}x{h}:\n{t}"
        );

        // Probing
        app_en.clash.exit_probing = true;
        app_en.clash.exit_probe_error = None;
        let t = render_clash_frame_text(&mut app_en, w, h);
        assert!(
            t.is_ascii(),
            "non-ASCII in Probing OutboundIp modal at {w}x{h}:\n{t}"
        );
    }
}
