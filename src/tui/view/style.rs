use super::*;
use crate::theme::{self as model, BackgroundStyle, ThemeName};

pub(in crate::tui) struct StyleGeometry {
    pub panel: Rect,
    rows: Rect,
    summary: Rect,
    hints: Rect,
    offset: usize,
    entries: Vec<Option<usize>>,
    summary_lines: Vec<String>,
}

impl StyleGeometry {
    pub fn new(area: Rect, app: &App) -> Self {
        Self::in_panel(Regions::new(area, app).details, app)
    }
    fn in_panel(panel: Rect, app: &App) -> Self {
        let inner =
            theme::panel(&app.theme.active, app.ctx.i18n.text("ui.style_title"), true).inner(panel);
        let summary_lines = summary(app, inner.width as usize);
        let summary_height = summary_lines
            .len()
            .min(inner.height.saturating_sub(3) as usize) as u16;
        let [rows, summary, hints] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(summary_height),
            Constraint::Length(1),
        ])
        .areas(inner);
        let state = app.style_panel.as_ref().expect("open style panel");
        let entries = state.rows();
        let selected = entries
            .iter()
            .position(|entry| *entry == Some(state.cursor))
            .unwrap();
        let offset = list_offset(selected, entries.len(), rows.height as usize);
        Self {
            panel,
            rows,
            summary,
            hints,
            offset,
            entries,
            summary_lines,
        }
    }
    pub fn row_at(&self, point: Position) -> Option<usize> {
        self.rows
            .contains(point)
            .then(|| self.offset + (point.y - self.rows.y) as usize)
            .and_then(|row| self.entries.get(row))
            .copied()
            .flatten()
    }
}

fn name(app: &App, definition: &model::ThemeDefinition) -> String {
    match &definition.name {
        ThemeName::Builtin(key) => app.ctx.i18n.text(key).into(),
        ThemeName::Custom(name) => name.clone(),
    }
}

fn summary(app: &App, width: usize) -> Vec<String> {
    let state = app.style_panel.as_ref().expect("open style panel");
    let entry = &state.entries[state.cursor];
    let i18n = &app.ctx.i18n;
    let mut lines = match entry.definition(&app.theme.definitions) {
        Ok(definition) => vec![
            name(app, definition),
            format!(
                "{} / {}",
                i18n.text(&format!(
                    "ui.style_background.{}",
                    definition.background.name()
                )),
                i18n.text(
                    if definition.patterns.background == BackgroundStyle::Paint {
                        "ui.style_paint"
                    } else {
                        "ui.style_terminal"
                    }
                )
            ),
            format!(
                "{} / {} / {}",
                definition.patterns.border.name(),
                definition.patterns.bar.name(),
                definition.patterns.sparkline.name()
            ),
        ],
        Err(error) => std::iter::once(entry.id.clone())
            .chain(wrap_words(
                &i18n.format("ui.theme_error", &[("detail", error)]),
                width,
            ))
            .collect(),
    };
    if entry.builtin {
        lines.push(i18n.text(&format!("theme.{}.description", entry.id)).into());
    }
    lines.push(
        i18n.format(
            "ui.style_saved",
            &[(
                "theme",
                app.theme
                    .saved_id
                    .as_deref()
                    .unwrap_or_else(|| i18n.text("ui.style_unsaved")),
            )],
        ),
    );
    if matches!(
        app.theme.source,
        model::ThemeSource::Flag | model::ThemeSource::Env
    ) {
        lines.push(i18n.text("ui.style_override").into());
    }
    if let Some(error) = &app.theme.error
        && entry.definition.is_ok()
    {
        lines.splice(
            1..1,
            wrap_words(&i18n.format("ui.theme_error", &[("detail", error)]), width),
        );
    }
    lines
}

fn entry_line(app: &App, index: usize, width: usize) -> Line<'static> {
    let state = app.style_panel.as_ref().unwrap();
    let entry = &state.entries[index];
    let active = &app.theme.active;
    let selected = state.cursor == index;
    let mut spans = vec![Span::styled(
        if selected {
            theme::selection_marker(active)
        } else {
            "  "
        },
        theme::fg(active, Role::SelectionMark),
    )];
    match entry.definition(&app.theme.definitions) {
        Ok(definition) => {
            let own = model::resolve(definition, &app.theme.background);
            let label_width = width.saturating_sub(29);
            let label = clip_with(&name(app, definition), label_width, ellipsis(active));
            spans.push(Span::styled(
                pad(&label, label_width),
                if selected {
                    theme::fg(active, Role::SelectionText).add_modifier(Modifier::BOLD)
                } else {
                    theme::text(active, Level::Primary)
                },
            ));
            for role in [
                Role::Background,
                Role::Surface,
                Role::Text1,
                Role::Accent,
                Role::Accent2,
            ] {
                spans.push(Span::styled(
                    theme::glyph(active, theme::Glyph::Swatch),
                    theme::fg(&own, role),
                ));
            }
            spans.push(Span::styled(
                format!(
                    " {}",
                    if theme::unframed(&own) {
                        " "
                    } else {
                        theme::border_set(&own).top_left
                    }
                ),
                theme::fg(&own, Role::Border),
            ));
            spans.extend(theme::bar(&own, 60.0, 6, Role::Meter));
            spans.push(Span::styled(
                theme::sparkline(&own, &[20., 60., 40., 90., 50.], 5),
                theme::fg(&own, Role::Accent),
            ));
        }
        Err(_) => {
            spans.push(Span::styled(
                format!("{} ", theme::glyph(active, theme::Glyph::Warning)),
                theme::fg(active, Role::Warning),
            ));
            spans.push(Span::raw(entry.id.clone()));
        }
    }
    if app.theme.saved_id.as_deref() == Some(&entry.id) {
        spans.push(Span::styled(
            format!(" {}", theme::glyph(active, theme::Glyph::Success)),
            theme::fg(active, Role::Success),
        ));
    }
    Line::from(clip_spans(spans, width, ellipsis(active)))
}

pub(super) fn draw(app: &App, frame: &mut Frame, regions: &Regions) {
    let geometry = StyleGeometry::in_panel(regions.details, app);
    let active = &app.theme.active;
    frame.render_widget(
        theme::panel(active, app.ctx.i18n.text("ui.style_title"), true),
        regions.details,
    );
    let state = app.style_panel.as_ref().unwrap();
    for (row, entry) in geometry
        .entries
        .iter()
        .skip(geometry.offset)
        .take(geometry.rows.height as usize)
        .enumerate()
    {
        let area = Rect::new(
            geometry.rows.x,
            geometry.rows.y + row as u16,
            geometry.rows.width,
            1,
        );
        let line = if let Some(index) = entry {
            entry_line(app, *index, area.width as usize)
        } else {
            Line::styled(
                clip_with(
                    &app.ctx.i18n.format(
                        "ui.style_custom",
                        &[("path", &app.ctx.paths.themes.display().to_string())],
                    ),
                    area.width as usize,
                    ellipsis(active),
                ),
                theme::muted(active),
            )
        };
        let style = if *entry == Some(state.cursor)
            && active.patterns.selection != model::SelectionStyle::Arrow
        {
            Style::default().bg(theme::color(active, Role::Selection))
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(line).style(style), area);
    }
    let lines = geometry
        .summary_lines
        .into_iter()
        .map(|text| {
            Line::styled(
                clip_with(&text, geometry.summary.width as usize, ellipsis(active)),
                theme::muted(active),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), geometry.summary);
    let (keys, close) = style_hints(app);
    frame.render_widget(
        Paragraph::new(theme::key_hints(
            active,
            &keys,
            Some(close),
            geometry.hints.width as usize,
        )),
        geometry.hints,
    );
}
