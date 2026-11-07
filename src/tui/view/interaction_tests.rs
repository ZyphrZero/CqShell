use super::*;
use crate::tui::{FormMode, OPTIMIZATION_GROUP, palette::CommandPalette, snapshot};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn mouse(kind: MouseEventKind, point: Position) -> MouseEvent {
    MouseEvent {
        kind,
        column: point.x,
        row: point.y,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, point: Position, area: Rect) {
    assert!(matches!(
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), point), area)
            .unwrap(),
        crate::tui::Effect::None
    ));
}

#[test]
fn footer_priorities_follow_the_top_layer_and_always_keep_help_or_close() {
    for locale in ["zh-CN", "en-US"] {
        for width in [80, 120] {
            for context in 0..7 {
                let mut app = snapshot::fixture_app(locale);
                match context {
                    1 => app.group = OPTIMIZATION_GROUP,
                    2 => app.worker = Some(std::thread::spawn(|| {})),
                    3 => app.command_palette = Some(CommandPalette::default()),
                    4 => {
                        app.form = Some(
                            app.make_form(
                                crate::actions::find("system.clean").unwrap(),
                                FormMode::Run,
                            )
                            .unwrap(),
                        )
                    }
                    5 => app.help = true,
                    _ => (),
                }
                let (hints, pinned) = if context == 6 {
                    style_hints(&app)
                } else {
                    footer_hints(&app)
                };
                let line = theme::key_hints(&app.theme.active, &hints, Some(pinned), width);
                assert!(line.width() <= width);
                assert!(line.to_string().contains(pinned.0));
                assert!(line.to_string().contains(pinned.1));
                if let Some(first) = hints.first() {
                    assert!(line.to_string().contains(first.0));
                }
                if let Some(worker) = app.worker.take() {
                    worker.join().unwrap();
                }
            }
        }
    }
    let mut app = snapshot::fixture_app("en-US");
    app.worker = Some(std::thread::spawn(|| {}));
    app.command_palette = Some(CommandPalette::default());
    assert_eq!(footer_hints(&app).1, ("Esc", "close"));
    app.worker.take().unwrap().join().unwrap();
}

#[test]
fn help_contains_all_key_groups_at_minimum_size_in_both_languages() {
    for locale in ["zh-CN", "en-US"] {
        let mut app = snapshot::fixture_app(locale);
        app.help = true;
        let buffer = snapshot::render(&app, 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for key in [
            "↑↓ / j k",
            "←→/Tab/Shift+Tab",
            "Home/End",
            "1–9",
            "O / 0",
            "Alt+↑↓",
            "Enter",
            "X / F5",
            "Space",
            "Ctrl+U",
            "PgUp/PgDn",
            "Q / Ctrl+C",
            "↑↓/j k/Home/End",
            "?/F1",
        ] {
            assert!(text.contains(key), "missing {key} in {locale}");
        }
        let compact = text.replace(' ', "");
        for label in [
            "help.navigate",
            "help.run",
            "help.view",
            "help.task",
            "help.palette",
            "help.style",
        ] {
            assert!(
                compact.contains(&app.ctx.i18n.text(label).replace(' ', "")),
                "{label}"
            );
        }
    }
}

#[test]
fn mouse_hits_only_category_rows_action_rows_and_the_exact_checkbox_cells() {
    for width in [80, 120] {
        let mut app = snapshot::fixture_app("en-US");
        app.root = true;
        let area = Rect::new(0, 0, width, 24);
        let layout = Regions::new(area, &app);
        let positions = app.positions;
        let group = app.group;
        click(
            &mut app,
            Position::new(layout.category_rows.x, layout.category_rows.y + 1),
            area,
        );
        assert_eq!(app.group, group);
        click(
            &mut app,
            Position::new(layout.action_rows.x + 3, layout.action_rows.y),
            area,
        );
        assert_eq!(app.positions, positions);
        click(
            &mut app,
            Position::new(layout.category_rows.x, layout.category_rows.y),
            area,
        );
        assert_eq!(app.group, OPTIMIZATION_GROUP);
        let layout = Regions::new(area, &app);
        for column in [layout.action_rows.x, layout.checkboxes.right()] {
            click(&mut app, Position::new(column, layout.action_rows.y), area);
            assert!(app.marked.is_empty());
        }
        for column in layout.checkboxes.x..layout.checkboxes.right() {
            click(&mut app, Position::new(column, layout.action_rows.y), area);
            assert_eq!(app.marked.len(), 1);
            click(&mut app, Position::new(column, layout.action_rows.y), area);
            assert!(app.marked.is_empty());
        }
        click(
            &mut app,
            Position::new(layout.action_rows.x + 8, layout.action_rows.y + 1),
            area,
        );
        assert_eq!(app.selected(), 1);
        assert!(app.marked.is_empty());
    }
}

#[test]
fn wheel_targets_its_region_and_modal_background_does_not_act() {
    let mut app = snapshot::fixture_app("en-US");
    let area = Rect::new(0, 0, 120, 36);
    app.viewport = area;
    app.logs_open = true;
    for index in 0..50 {
        app.log(format!("line {index}"));
    }
    let layout = Regions::new(area, &app);
    let group = app.group;
    app.mouse(
        mouse(
            MouseEventKind::ScrollDown,
            Position::new(layout.action_rows.x, layout.action_rows.y),
        ),
        area,
    )
    .unwrap();
    assert_eq!(app.selected(), 1);
    assert_eq!(app.group, group);
    app.mouse(
        mouse(
            MouseEventKind::ScrollDown,
            Position::new(layout.category_rows.x, layout.category_rows.y),
        ),
        area,
    )
    .unwrap();
    assert_ne!(app.group, group);
    app.mouse(
        mouse(
            MouseEventKind::ScrollUp,
            Position::new(layout.logs.x + 2, layout.logs.y + 2),
        ),
        area,
    )
    .unwrap();
    assert!(app.scroll > 0);
    for overlay in 0..3 {
        let group = app.group;
        let positions = app.positions;
        app.help = overlay == 0;
        app.notice = (overlay == 1).then(|| "notice".into());
        app.log_modal = overlay == 2;
        let scroll = app.scroll;
        click(&mut app, Position::new(0, 0), area);
        app.mouse(mouse(MouseEventKind::ScrollDown, Position::new(0, 0)), area)
            .unwrap();
        assert_eq!(
            (app.group, app.positions, app.scroll),
            (group, positions, scroll)
        );
        if overlay == 0 {
            assert!(app.help);
        }
    }
}
