use super::*;
use crate::{
    actions,
    tui::{
        Effect, Task,
        snapshot::{self, Background, Case},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn form_app(id: &str, mode: FormMode, locale: &str) -> App {
    let mut app = snapshot::fixture_app(locale);
    if id == "terminal.apply" {
        app.parameters
            .insert(id.to_owned(), actions::find(id).unwrap().defaults());
    }
    app.form = Some(app.make_form(actions::find(id).unwrap(), mode).unwrap());
    app
}

fn text(buffer: &ratatui::buffer::Buffer, rect: Rect) -> String {
    (rect.top()..rect.bottom())
        .map(|row| {
            let mut line = String::new();
            let mut column = rect.x;
            while column < rect.right() {
                let symbol = buffer[(column, row)].symbol();
                line.push_str(symbol);
                column += symbol.width().max(1) as u16;
            }
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn plain_dialogs_use_ascii_markers_and_keep_selected_buttons_distinct() {
    let mut app = form_app("system.clean", FormMode::Run, "en-US");
    app.preview_theme("plain", crate::theme::Background::Disabled)
        .unwrap();
    let buffer = snapshot::render(&app, 80, 24);
    let geometry = FormGeometry::new(buffer.area, app.form.as_ref().unwrap(), &app);
    let label = geometry.fields[0].label;
    assert_eq!(buffer[(label.x, label.y)].symbol(), ">");
    app.form = None;
    app.command_palette = Some(crate::tui::palette::CommandPalette::default());
    let geometry = CommandPaletteGeometry::new(buffer.area, &app, &app.palette_results());
    let buffer = snapshot::render(&app, 80, 24);
    assert_eq!(buffer[(geometry.input.x, geometry.input.y)].symbol(), ">");
    app.command_palette = None;
    app.confirmation = Some(Confirmation {
        text: "Restore?".into(),
        token: "RESTORE".into(),
        input: crate::tui::InputBuffer::new("RESTORE".into()),
        reply: std::sync::mpsc::channel().0,
        scroll: 0,
        accept: false,
    });
    let geometry = ConfirmationGeometry::new(
        buffer.area,
        app.confirmation.as_ref().unwrap(),
        &app.ctx.i18n,
    );
    let buffer = snapshot::render(&app, 80, 24);
    let control = geometry.sections[1];
    assert_eq!(buffer[(control.x, control.y + 1)].symbol(), ">");
    assert!(text(&buffer, control).contains('+'));
    assert!(!text(&buffer, control).contains('✓'));
    app.confirmation.as_mut().unwrap().token = "YES".into();
    let geometry = ConfirmationGeometry::new(
        buffer.area,
        app.confirmation.as_ref().unwrap(),
        &app.ctx.i18n,
    );
    let buffer = snapshot::render(&app, 80, 24);
    let cancel = geometry.buttons[0];
    assert!(
        buffer[(cancel.x, cancel.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
}

fn compact(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn every_chip_choice_has_both_descriptions_and_custom_fields_stay_editable() {
    for locale in ["zh-CN", "en-US"] {
        let i18n = I18n::load(Some(locale), None).unwrap();
        let mut count = 0;
        for spec in actions::specs() {
            for field in &spec.fields {
                if field.choices.is_empty() || field.allow_custom {
                    continue;
                }
                for choice in field.choices {
                    let key = format!("option.{}.{}.{choice}", spec.id, field.name);
                    assert_ne!(i18n.text(&key), key, "{locale}");
                    assert!(!i18n.text(&key).is_empty());
                    count += 1;
                }
            }
        }
        assert_eq!(count, 66);
    }
    let app = form_app("system.timezone", FormMode::Run, "en-US");
    assert!(!app.form.as_ref().unwrap().chip_field(0));
}

#[test]
fn chip_windows_keep_the_selected_choice_whole_and_mark_hidden_sides() {
    let spec = actions::find("tools.install").unwrap();
    let choices = spec.fields[0].choices;
    for width in [12, 18, 26, 50, 80] {
        let area = Rect::new(4, 7, width, 1);
        for selected in 0..choices.len() {
            let geometry = ChipGeometry::new(area, choices, selected);
            let (_, current) = geometry
                .choices
                .iter()
                .find(|(index, _)| *index == selected)
                .unwrap();
            assert_eq!(current.width as usize, choices[selected].width() + 2);
            assert!(
                geometry
                    .choices
                    .iter()
                    .all(|(_, rect)| rect.intersection(area) == *rect)
            );
            assert_eq!(geometry.previous.is_some(), geometry.choices[0].0 > 0);
            assert_eq!(
                geometry.next.is_some(),
                geometry.choices.last().unwrap().0 + 1 < choices.len()
            );
            for pair in geometry.choices.windows(2) {
                assert_eq!(pair[0].0 + 1, pair[1].0);
                assert!(pair[0].1.right() < pair[1].1.x);
            }
        }
    }
}

#[test]
fn chips_cycle_without_accepting_text_edits_and_mouse_selects_exact_values() {
    let mut app = form_app("bbr.enable", FormMode::Run, "en-US");
    app.key(key(KeyCode::Left)).unwrap();
    assert_eq!(app.form.as_ref().unwrap().values[0].text, "cake");
    app.key(key(KeyCode::Right)).unwrap();
    assert_eq!(app.form.as_ref().unwrap().values[0].text, "fq");
    for event in [
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        key(KeyCode::Backspace),
        key(KeyCode::Delete),
        key(KeyCode::Char('z')),
    ] {
        app.key(event).unwrap();
        assert_eq!(app.form.as_ref().unwrap().values[0].text, "fq");
    }
    let area = Rect::new(0, 0, 80, 24);
    let click = |point: Position| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: point.x,
        row: point.y,
        modifiers: KeyModifiers::NONE,
    };
    for choice in 0..4 {
        let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
        let rect = geometry.fields[0]
            .chips
            .as_ref()
            .unwrap()
            .choices
            .iter()
            .find(|(index, _)| *index == choice)
            .unwrap()
            .1;
        let point = Position::new(rect.x + 1, rect.y);
        assert_eq!(geometry.chip_at(point), Some((0, choice)));
        for _ in 0..2 {
            assert!(matches!(
                app.mouse(click(point), area).unwrap(),
                Effect::None
            ));
            let form = app.form.as_ref().unwrap();
            assert_eq!(form.values[0].text, form.spec.fields[0].choices[choice]);
        }
    }
    let held = app.form.as_ref().unwrap().values[0].text.clone();
    let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
    let gap = geometry.fields[0].chips.as_ref().unwrap().choices[0]
        .1
        .right();
    let point = Position::new(gap, geometry.fields[0].input.y);
    assert_eq!(geometry.chip_at(point), None);
    app.mouse(click(point), area).unwrap();
    app.mouse(click(Position::new(0, 0)), area).unwrap();
    assert_eq!(app.form.as_ref().unwrap().values[0].text, held);
    assert!(app.worker.is_none());
}

#[test]
fn forms_show_current_state_impact_mode_and_live_command() {
    for locale in ["zh-CN", "en-US"] {
        for (id, mode, subtitle, primary) in [
            (
                "bbr.enable",
                FormMode::Run,
                "ui.form_reviewed_subtitle",
                "ui.confirm_execute",
            ),
            (
                "system.clean",
                FormMode::Run,
                "ui.form_single_title",
                "ui.continue",
            ),
            (
                "bbr.enable",
                FormMode::Optimization,
                "ui.form_batch_title",
                "ui.save_selection",
            ),
        ] {
            let mut app = form_app(id, mode, locale);
            for (width, height) in [(80, 24), (120, 36)] {
                let buffer = snapshot::render(&app, width, height);
                let form = app.form.as_ref().unwrap();
                let geometry = FormGeometry::new(buffer.area, form, &app);
                let header = text(&buffer, geometry.sections[0]);
                assert!(
                    compact(&header).contains(&compact(app.ctx.i18n.text(subtitle))),
                    "{header}"
                );
                let state = if id == "system.clean" {
                    "dash.disk"
                } else {
                    "related.congestion"
                };
                let state_at = header.find(app.ctx.i18n.text(state)).unwrap();
                let impact_at = header.find(app.ctx.i18n.text("ui.mutating")).unwrap();
                assert!(state_at < impact_at, "{header}");
                assert_eq!(
                    text(&buffer, geometry.buttons[0]),
                    button_label(app.ctx.i18n.text(primary))
                );
                assert_eq!(
                    text(&buffer, geometry.buttons[1]),
                    button_label(app.ctx.i18n.text("ui.cancel"))
                );
                assert!(text(&buffer, geometry.command).contains(&format!("cqs run {id}")));
            }
            if id == "system.clean" {
                app.key(key(KeyCode::Right)).unwrap();
                let buffer = snapshot::render(&app, 80, 24);
                let geometry = FormGeometry::new(buffer.area, app.form.as_ref().unwrap(), &app);
                let command = text(&buffer, geometry.command);
                assert!(
                    command.contains("scope=full") && !command.contains("scope=standard"),
                    "{command}"
                );
                let (_, rect) = geometry.fields[0]
                    .chips
                    .as_ref()
                    .unwrap()
                    .choices
                    .iter()
                    .find(|(index, _)| *index == 1)
                    .unwrap();
                assert_eq!(
                    buffer[(rect.x + 1, rect.y)].fg,
                    theme::color(&app.theme.active, Role::OnAccent)
                );
                assert_eq!(
                    buffer[(rect.x + 1, rect.y)].bg,
                    theme::color(&app.theme.active, Role::Accent)
                );
            }
        }
    }
}

#[test]
fn all_forms_keep_current_field_its_description_errors_and_both_buttons_visible() {
    for locale in ["zh-CN", "en-US"] {
        for spec in actions::specs()
            .into_iter()
            .filter(|spec| !spec.fields.is_empty())
        {
            let mut app = form_app(spec.id, FormMode::Run, locale);
            let count = app.form.as_ref().unwrap().values.len();
            for index in 0..count + 2 {
                let form = app.form.as_mut().unwrap();
                form.index = index;
                form.error = Some("Invalid fixture value".into());
                let buffer = snapshot::render(&app, 80, 24);
                let form = app.form.as_ref().unwrap();
                let geometry = FormGeometry::new(buffer.area, form, &app);
                let active = index.min(count - 1);
                let layout = geometry
                    .fields
                    .iter()
                    .find(|field| field.index == active)
                    .unwrap_or_else(|| panic!("{locale} {} field {active} is hidden", spec.id));
                assert_eq!(layout.area.intersection(geometry.sections[1]), layout.area);
                assert!(text(&buffer, layout.label).contains(&format!("{}/{}", active + 1, count)));
                if let Some(rect) = layout.description {
                    let field = &form.spec.fields[active];
                    let key = format!(
                        "option.{}.{}.{}",
                        spec.id, field.name, form.values[active].text
                    );
                    let expected = super::super::clip_with(
                        app.ctx.i18n.text(&key),
                        rect.width as usize,
                        ellipsis(&app.theme.active),
                    );
                    assert_eq!(text(&buffer, rect), expected, "{locale} {key}");
                }
                assert!(text(&buffer, geometry.sections[2]).contains("Invalid fixture value"));
                for (rect, key) in [
                    (geometry.buttons[0], primary_key(form)),
                    (geometry.buttons[1], "ui.cancel"),
                ] {
                    assert_eq!(rect.intersection(geometry.popup), rect);
                    assert_eq!(text(&buffer, rect), button_label(app.ctx.i18n.text(key)));
                }
            }
        }
    }
}

#[test]
fn run_forms_dispatch_reviewed_tasks_and_optimization_still_only_saves() {
    for id in ["bbr.enable", "system.clean", "tcp.tune"] {
        let mut app = form_app(id, FormMode::Run, "en-US");
        let Effect::Run(Task::Single {
            spec,
            params,
            reviewed,
        }) = app.submit_form().unwrap()
        else {
            panic!("single task")
        };
        assert_eq!(spec.id, id);
        assert_eq!(params, spec.defaults());
        assert!(reviewed);
        assert!(app.form.is_none());
        let mut app = form_app(id, FormMode::Optimization, "en-US");
        assert!(matches!(app.submit_form().unwrap(), Effect::None));
        assert!(app.marked.contains(id) && app.worker.is_none());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn reviewed_dispatch_skips_generic_only_for_reversible_forms() {
    use crate::runtime::Event;
    use crate::tui::InputBuffer;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    for (id, values, confirmations) in [
        ("clash.proxy.select", vec!["Manual", "Direct"], 0),
        ("clash.sub.use", vec!["work"], 1),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let mut app = App::new(crate::tests::context(temporary.path()));
        app.ctx.yes = false;
        let mut form = app
            .make_form(actions::find(id).unwrap(), FormMode::Run)
            .unwrap();
        for (input, value) in form.values.iter_mut().zip(values) {
            *input = InputBuffer::new(value.into());
        }
        app.form = Some(form);
        let Effect::Run(task) = app.submit_form().unwrap() else {
            panic!("single task")
        };
        let (sender, receiver) = mpsc::channel();
        let mut context = app.ctx.clone();
        context.events = Some(sender);
        let worker = thread::spawn(move || crate::tui::perform(&context, &task));
        let mut observed = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !worker.is_finished() {
            assert!(
                Instant::now() < deadline,
                "reviewed dispatcher did not finish"
            );
            if let Ok(Event::Confirm { reply, token, .. }) =
                receiver.recv_timeout(Duration::from_millis(20))
            {
                assert_eq!(token, "YES");
                observed += 1;
                reply.send(false).unwrap();
            }
        }
        let report = worker.join().unwrap();
        assert_eq!(observed, confirmations, "{id}");
        let error = report.result.unwrap_err();
        if confirmations == 1 {
            assert_eq!(error.key, "error.cancelled");
        } else {
            assert!(matches!(error.key, "error.io" | "error.data"));
            assert!(!app.ctx.paths.clash_config.exists());
        }
    }
}

#[test]
fn classic_chip_form_snapshots_are_separate_from_historical_baselines() {
    for locale in ["zh-CN", "en-US"] {
        for (width, height) in [(80, 24), (120, 36)] {
            for (state, id, mode) in [
                ("clean", "system.clean", FormMode::Run),
                ("bbr", "bbr.enable", FormMode::Run),
                ("selection", "bbr.enable", FormMode::Optimization),
                ("overflow", "tools.install", FormMode::Run),
            ] {
                let mut app = form_app(id, mode, locale);
                if state == "overflow" {
                    app.form.as_mut().unwrap().values[0].replace("fzf".into());
                }
                let case = Case {
                    name: format!("classic_stage2_chip_form_{state}_{width}x{height}_{locale}"),
                    theme: "classic".into(),
                    background: Background::Unavailable,
                    locale: locale.into(),
                };
                snapshot::assert_snapshot(&case, &snapshot::render(&app, width, height));
            }
        }
    }
}
