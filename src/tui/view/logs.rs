//! Task log panel, shared by the inline strip and the full-screen log view. Scroll offsets
//! count wrapped rows from the newest line, so arriving output never moves a reader.
use super::{Regions, popup_full, theme, wrap_cells};
use crate::{
    theme::Role,
    tui::{App, Phase, Tone},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph},
};

fn frame(theme: &crate::theme::Theme) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1));
    if theme::unframed(theme) {
        block
            .borders(Borders::NONE)
            .padding(Padding::horizontal(2))
            .title(Line::default())
    } else {
        block
    }
}
/// Where log rows render: the full-screen view when open, else the inline strip (empty when
/// the terminal is too short to show it).
pub(in crate::tui) fn log_body(app: &App, viewport: Rect) -> Rect {
    let area = if app.log_modal {
        popup_full(viewport)
    } else {
        Regions::new(viewport, app).logs
    };
    if area.height == 0 {
        return Rect::default();
    }
    frame(&app.theme.active).inner(area)
}
fn rows(app: &App, width: usize) -> Vec<(String, Tone, bool)> {
    app.logs
        .iter()
        .flat_map(|line| {
            let prefix = (theme::unframed(&app.theme.active)
                || app.theme.active.patterns.glyphs == crate::theme::GlyphSet::Ascii)
                && matches!(line.tone, Tone::Success | Tone::Warning | Tone::Danger);
            wrap_cells(
                &line.text.replace('\t', "    "),
                width.saturating_sub(usize::from(prefix) * 2),
            )
            .into_iter()
            .map(move |row| (row, line.tone, line.bold))
        })
        .collect()
}
pub(in crate::tui) fn log_row_count(app: &App, width: usize) -> usize {
    rows(app, width).len()
}

pub(super) fn draw(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    if app.log_modal {
        theme::scrim(t, f.buffer_mut(), area);
        f.render_widget(Clear, area);
        theme::shadow(t, f, area);
    }
    let body = frame(t).inner(area);
    let rows = rows(app, body.width as usize);
    let visible = body.height as usize;
    let scroll = (app.scroll as usize).min(rows.len().saturating_sub(visible));
    let end = rows.len() - scroll;
    let start = end.saturating_sub(visible);
    let border = if app.phase == Phase::Idle
        || theme::unframed(t)
        || t.patterns.glyphs == crate::theme::GlyphSet::Ascii
    {
        Role::Border
    } else {
        theme::role(app.phase.tone())
    };
    let step = app
        .progress
        .as_ref()
        .and_then(|progress| progress.current().map(|index| (progress, index)));
    let heading = if let Some((progress, index)) = step {
        i.format(
            "ui.log_step_title",
            &[
                ("index", &(index + 1).to_string()),
                ("total", &progress.steps.len().to_string()),
                ("action", i.text(&format!("action.{}", app.task_ids[index]))),
                (
                    "elapsed",
                    &super::plan::elapsed_text(app.now.saturating_duration_since(progress.started)),
                ),
            ],
        )
    } else if app.task_ids.is_empty() {
        i.text("ui.logs").to_owned()
    } else {
        format!("{} · {}", i.text("ui.logs"), app.task_title())
    };
    let mut block = frame(t)
        .border_set(theme::border_set(t))
        .style(
            theme::text(t, crate::theme::Level::Primary).bg(theme::color(
                t,
                if app.log_modal {
                    Role::Dialog
                } else {
                    Role::Surface
                },
            )),
        )
        .border_style(Style::default().fg(theme::color(t, border)))
        .title(theme::title(t, heading, Role::Text1));
    if app.phase != Phase::Idle && step.is_none() {
        let tone = app.phase.tone();
        block = block.title(
            Line::from(Span::styled(
                format!(
                    " {} {} ",
                    theme::phase_icon(t, app.phase, app.elapsed()),
                    i.text(app.phase.key())
                ),
                theme::tone(t, tone).add_modifier(ratatui::style::Modifier::BOLD),
            ))
            .right_aligned(),
        );
    }
    if scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(
                    " {} ",
                    i.format("ui.scrolled", &[("count", &scroll.to_string())])
                ),
                theme::fg(t, Role::Info),
            ))
            .right_aligned(),
        );
    }
    if app.log_modal {
        let hints = [("PgUp/PgDn", i.text("key.scroll"))];
        let mut line = theme::key_hints(t, &hints, Some(("Esc", i.text("key.back"))), 48);
        line.spans.insert(0, Span::raw(" "));
        line.spans.push(Span::raw(" "));
        block = block.title_bottom(line);
    }
    let lines = if rows.is_empty() {
        vec![Line::styled(
            i.text("ui.empty_log").to_owned(),
            theme::muted(t),
        )]
    } else {
        rows[start..end]
            .iter()
            .map(|(text, tone, bold)| {
                let style = theme::tone(t, *tone);
                if (theme::unframed(t) || t.patterns.glyphs == crate::theme::GlyphSet::Ascii)
                    && matches!(tone, Tone::Success | Tone::Warning | Tone::Danger)
                {
                    return Line::from(vec![
                        Span::styled(
                            format!("{} ", theme::icon(t, *tone)),
                            theme::fg(t, theme::role(*tone)),
                        ),
                        Span::styled(text.clone(), style),
                    ]);
                }
                Line::styled(
                    text.clone(),
                    if *bold {
                        style.add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        style
                    },
                )
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
    theme::scrollbar(t, f, theme::track(area, body), rows.len(), visible, start);
}
