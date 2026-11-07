use super::*;
use crate::{
    theme::{self as model, Background},
    tui::{
        Confirmation, FormMode, GROUPS, InputBuffer, Loading, OPTIMIZATION_GROUP, Toast,
        palette::CommandPalette, snapshot,
    },
};
use proptest::prelude::*;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use std::sync::{Arc, atomic::AtomicBool, mpsc};

fn assert_text_inside(buffer: &Buffer, rect: Rect) {
    for row in buffer.area.top()..buffer.area.bottom() {
        for column in buffer.area.left()..buffer.area.right() {
            let symbol = buffer[(column, row)].symbol();
            if symbol != " " && !symbol.is_empty() {
                assert!(
                    rect.contains(Position::new(column, row)),
                    "{symbol:?} at {column},{row} outside {rect:?}"
                );
                assert!(
                    column as usize + symbol.width() <= rect.right() as usize,
                    "wide glyph {symbol:?} crosses {rect:?}"
                );
            }
        }
    }
}

fn assert_regions(app: &App, width: u16, height: u16) {
    let area = Rect::new(0, 0, width, height);
    let regions = Regions::new(area, app);
    type DrawRegion = fn(&App, &mut Frame, &Regions);
    let parts: [(Rect, DrawRegion); 7] = [
        (regions.header, |app, frame, regions| {
            header::draw(app, frame, regions.header, regions.selected_row(app))
        }),
        (regions.dashboard, |app, frame, regions| {
            dashboard::draw(app, frame, regions.dashboard)
        }),
        (regions.categories, draw_categories),
        (regions.actions, |app, frame, regions| {
            draw_actions(app, frame, regions, &regions.rows)
        }),
        (regions.details, |app, frame, regions| {
            let selected = regions.selected_row(app);
            draw_details(
                app,
                frame,
                regions,
                match selected {
                    Row::Action(spec) => Some(spec),
                    _ => None,
                },
            );
        }),
        (regions.logs, |app, frame, regions| {
            if regions.logs.height > 0 {
                logs::draw(app, frame, regions.logs);
            }
        }),
        (regions.footer, draw_footer),
    ];
    for (rect, draw) in parts {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(app, frame, &regions)).unwrap();
        assert_text_inside(terminal.backend().buffer(), rect);
    }
    assert_eq!(app.category_rows().len(), 13);
    assert_eq!(regions.category_offset, 0);
    assert!(regions.category_rows.height >= 13);
    for index in 0..13 {
        assert_eq!(
            regions.category_at(Position::new(
                regions.category_rows.x,
                regions.category_rows.y + index
            )),
            Some(index as usize)
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    // Feature: tui-redesign, Property 10: 渲染不越界
    #[test]
    fn rendering_stays_in_bounds(
        (width, height) in (80u16..=200, 24u16..=60),
        (theme_index, background, rgb) in (any::<usize>(), 0u8..3, any::<(u8, u8, u8)>()),
        (english, group, selected, checklist) in (any::<bool>(), 0usize..GROUPS.len(), any::<usize>(), any::<u16>()),
        (overlay, repetitions, word, log_count) in (0u8..13, 1usize..240, prop::sample::select(vec!["主机界", "host", "🦀", "e\u{301}"]), 0usize..60),
    ) {
        let mut app = snapshot::fixture_app(if english { "en-US" } else { "zh-CN" });
        let definitions = model::builtin::all();
        let background = match background { 0 => Background::Detected(rgb), 1 => Background::Unavailable("test".into()), _ => Background::Disabled };
        app.theme.active = model::resolve(&definitions[theme_index % definitions.len()], &background);
        app.theme.background = background;
        app.group = group;
        app.positions[group] = selected % app.selectable_count(group);
        app.marked = crate::actions::OPTIMIZATION.iter().enumerate().filter(|(index, _)| checklist & (1 << index) != 0).map(|(_, id)| (*id).to_owned()).collect();
        let long = word.repeat(repetitions);
        app.snapshot.local.host = Reading::Value(long.clone());
        app.logs_open = true;
        for index in 0..log_count { app.log(format!("{index} {long}")); }
        if overlay == 10 {
            app.group = OPTIMIZATION_GROUP;
            app.task_ids = crate::actions::OPTIMIZATION.iter().map(|id| (*id).to_owned()).collect();
            let mut progress = plan::Progress::new(app.task_ids.len(), app.now);
            progress.steps[selected % app.task_ids.len()] = plan::StepView::Running(app.now);
            app.progress = Some(progress);
        }
        assert_regions(&app, width, height);
        match overlay {
            1 => app.help = true,
            2 => app.notice = Some(long.clone()),
            3 => app.overview = true,
            4 => app.log_modal = true,
            5 => app.confirmation = Some(Confirmation { text: long.clone(), token: "YES".into(), input: InputBuffer::default(), reply: mpsc::channel().0, scroll: 0, accept: false }),
            6 => {
                let mut form = app.make_form(crate::actions::find("clash.sub.add").unwrap(), FormMode::Run).unwrap();
                form.index = selected % form.values.len();
                let field = &form.spec.fields[form.index];
                let value = if form.chip_field(form.index) {
                    field.choices[selected % field.choices.len()].to_owned()
                } else {
                    long.clone()
                };
                form.values[form.index].replace(value);
                app.form = Some(form);
            }
            7 => {
                let mut form = app.make_form(crate::actions::find("clash.sub.use").unwrap(), FormMode::Run).unwrap();
                form.attach(vec![(0..12).map(|index| crate::ops::Choice { value: format!("{index}-{long}"), detail: long.clone(), current: index == 0, children: Vec::new() }).collect()]);
                form.open_picker(String::new(), true);
                app.form = Some(form);
            }
            8 => app.command_palette = Some(CommandPalette { query: InputBuffer::new(long.clone()), ..CommandPalette::default() }),
            9 => app.loading = Some(Loading { spec: crate::actions::find("clash.proxy.select").unwrap(), mode: FormMode::Run, receiver: mpsc::channel().1, cancel: Arc::new(AtomicBool::new(false)), started: app.now - Duration::from_secs(1) }),
            11 => app.toast = Some(Toast { text: long, tone: Tone::Warning, shown: app.now }),
            12 => { app.open_style(); app.preview_style(selected % model::builtin::all().len()); }
            _ => (),
        }
        let buffer = snapshot::render(&app, width, height);
        assert_text_inside(&buffer, buffer.area);
    }
}

#[test]
fn redesigned_localization_families_are_complete() {
    let app = snapshot::fixture_app("en-US");
    let mut keys = std::collections::BTreeSet::new();
    for (group, id) in GROUPS.iter().enumerate() {
        for row in app.rows(group) {
            if let Row::Section(section) = row {
                keys.insert(format!("section.{id}.{section}"));
            }
        }
    }
    for spec in crate::actions::specs() {
        for field in &spec.fields {
            if !field.allow_custom {
                for value in field.choices {
                    keys.insert(format!("option.{}.{}.{value}", spec.id, field.name));
                }
            }
            if crate::actions::OPTIMIZATION.contains(&spec.id)
                && field.required
                && field.default.is_empty()
            {
                keys.insert(format!("field_short.{}", field.name));
            }
        }
    }
    keys.extend(["age.seconds", "age.minutes", "age.hours", "age.days"].map(str::to_owned));
    for locale in ["zh-CN", "en-US"] {
        let i18n = crate::i18n::I18n::load(Some(locale), None).unwrap();
        for key in &keys {
            assert_ne!(i18n.text(key), key, "missing {locale} {key}");
        }
    }
}
