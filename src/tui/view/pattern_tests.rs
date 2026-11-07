use super::*;
use crate::{
    theme::{self as model, Background, Color as ThemeColor},
    tui::{FormMode, snapshot},
};

#[test]
fn plain_main_glyphs_are_ascii_and_state_shapes_are_distinct() {
    let mut app = snapshot::fixture_app("en-US");
    app.preview_theme("plain", Background::Disabled).unwrap();
    for (width, height) in [(80, 24), (120, 36)] {
        let buffer = snapshot::render(&app, width, height);
        let regions = Regions::new(buffer.area, &app);
        for (index, cell) in buffer
            .content
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.symbol().is_ascii())
        {
            let point = Position::new(
                (index % width as usize) as u16,
                (index / width as usize) as u16,
            );
            let label = regions
                .action_at(point)
                .and_then(|row| match &app.rows(app.group)[row] {
                    Row::Action(spec) => Some(app.ctx.i18n.text(&spec.label()).to_owned()),
                    _ => None,
                });
            assert!(
                label.is_some_and(|label| label.contains(cell.symbol())),
                "generated non-ASCII glyph {point:?}: {}",
                cell.symbol()
            );
        }
    }
    for definition in [model::builtin::classic(), model::builtin::plain()] {
        let theme = model::resolve(&definition, &Background::Disabled);
        let states = [
            theme::glyph(&theme, theme::Glyph::Success),
            theme::glyph(&theme, theme::Glyph::Failure),
            theme::glyph(&theme, theme::Glyph::On),
            theme::glyph(&theme, theme::Glyph::Off),
            theme::spinner(&theme, Duration::ZERO),
        ];
        assert_eq!(
            states
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            5
        );
    }
}

#[test]
fn gradients_include_partial_cells_and_never_override_warning_colors() {
    let theme = model::resolve(&model::builtin::neon(), &Background::Disabled);
    let bars = theme::bar(&theme, 60.0, 6, Role::Meter);
    assert_eq!(bars[0].style.fg, Some(theme::color(&theme, Role::Accent)));
    assert_eq!(
        bars[bars.len() - 2].style.fg,
        Some(theme::color(&theme, Role::Accent2))
    );
    let warning = theme::bar(&theme, 20.0, 12, Role::Warning);
    assert!(
        warning[..warning.len() - 1]
            .iter()
            .all(|span| span.style.fg == Some(theme::color(&theme, Role::Warning)))
    );
}

#[test]
fn dialog_fading_does_not_fade_the_background_scrim() {
    let mut app = snapshot::fixture_app("en-US");
    app.preview_theme("neon", Background::Detected((11, 17, 25)))
        .unwrap();
    app.form = Some(
        app.make_form(crate::actions::find("system.clean").unwrap(), FormMode::Run)
            .unwrap(),
    );
    let start = app.now;
    app.tick(start);
    let buffer = snapshot::render(&app, 120, 36);
    let popup = FormGeometry::new(buffer.area, app.form.as_ref().unwrap(), &app).popup;
    assert_eq!(
        buffer[(0, 0)].fg,
        theme::color(&app.theme.active, Role::Dim)
    );
    assert_eq!(
        buffer[(popup.x + 1, popup.y + 1)].bg,
        theme::color(&app.theme.active, Role::Background)
    );
    app.tick(start + crate::tui::motion::DIALOG_FADE);
    let finished = snapshot::render(&app, 120, 36);
    assert_eq!(finished[(0, 0)].fg, buffer[(0, 0)].fg);
    assert_eq!(
        finished[(popup.x + 1, popup.y + 1)].bg,
        theme::color(&app.theme.active, Role::Dialog)
    );
}

#[test]
fn paint_and_terminal_backgrounds_and_dialog_shadows_match_the_palette() {
    for id in ["aurora", "classic", "neon", "sand", "plain"] {
        let mut app = snapshot::fixture_app("en-US");
        app.preview_theme(id, Background::Detected((11, 17, 25)))
            .unwrap();
        let buffer = snapshot::render(&app, 120, 36);
        let expected = if app.theme.active.patterns.background == model::BackgroundStyle::Paint {
            theme::color(&app.theme.active, Role::Background)
        } else {
            ratatui::style::Color::Reset
        };
        assert_eq!(buffer[(0, 0)].bg, expected, "{id}");
        app.form = Some(
            app.make_form(crate::actions::find("system.clean").unwrap(), FormMode::Run)
                .unwrap(),
        );
        let buffer = snapshot::render(&app, 120, 36);
        let popup = FormGeometry::new(buffer.area, app.form.as_ref().unwrap(), &app).popup;
        assert_eq!(
            buffer[(popup.x + 1, popup.y + 1)].bg,
            theme::color(&app.theme.active, Role::Dialog)
        );
        if app.theme.active.palette.dialog != ThemeColor::Default {
            assert_eq!(
                buffer[(popup.right(), popup.y + 1)].bg,
                theme::color(&app.theme.active, Role::Shadow)
            );
            assert_eq!(
                buffer[(popup.x + 1, popup.bottom())].bg,
                theme::color(&app.theme.active, Role::Shadow)
            );
        }
    }
}

#[test]
fn aurora_limits_severity_to_glyphs_and_gauge_thresholds() {
    let mut app = snapshot::fixture_app("en-US");
    app.preview_theme("aurora", Background::Detected((11, 17, 25)))
        .unwrap();
    let buffer = snapshot::render(&app, 120, 36);
    let colors = [Role::Success, Role::Warning, Role::Danger]
        .map(|role| theme::color(&app.theme.active, role));
    for row in 0..36 {
        for column in 0..120 {
            let cell = &buffer[(column, row)];
            if colors.contains(&cell.fg) && !cell.symbol().trim().is_empty() {
                assert!(
                    matches!(cell.symbol(), "●" | "○" | "✓" | "✗" | "!" | "■")
                        || (2..=3).contains(&row),
                    "{column},{row}: {}",
                    cell.symbol()
                );
            }
        }
    }
    assert_eq!(theme::gauge(69.9), Role::Meter);
    assert_eq!(theme::gauge(70.0), Role::Warning);
    assert_eq!(theme::gauge(90.0), Role::Danger);
}
