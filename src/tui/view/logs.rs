//! Task log panel, shared by the inline strip and the full-screen log view. Scroll offsets
//! count wrapped rows from the newest line, so arriving output never moves a reader.
use super::{Regions, popup_full, theme, wrap_cells};
use crate::tui::{App, Phase, Tone};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph},
};

fn frame() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
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
    frame().inner(area)
}
fn rows(app: &App, width: usize) -> Vec<(String, Tone)> {
    app.logs
        .iter()
        .flat_map(|line| {
            wrap_cells(&line.text.replace('\t', "    "), width)
                .into_iter()
                .map(move |row| (row, line.tone))
        })
        .collect()
}
pub(in crate::tui) fn log_row_count(app: &App, width: usize) -> usize {
    rows(app, width).len()
}

pub(super) fn draw(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    if app.log_modal {
        theme::scrim(f.buffer_mut(), area);
        f.render_widget(Clear, area);
    }
    let body = frame().inner(area);
    let rows = rows(app, body.width as usize);
    let visible = body.height as usize;
    let scroll = (app.scroll as usize).min(rows.len().saturating_sub(visible));
    let end = rows.len() - scroll;
    let start = end.saturating_sub(visible);
    let border = if app.phase == Phase::Idle {
        theme::BORDER
    } else {
        theme::color(app.phase.tone())
    };
    let heading = if app.task_ids.is_empty() {
        i.text("ui.logs").to_owned()
    } else {
        format!("{} · {}", i.text("ui.logs"), app.task_title())
    };
    let mut block = frame()
        .border_style(Style::default().fg(border))
        .title(theme::title(heading, Tone::Plain));
    if app.phase != Phase::Idle {
        let tone = app.phase.tone();
        block = block.title(
            Line::from(Span::styled(
                format!(
                    " {} {} ",
                    theme::phase_icon(app.phase, app.elapsed()),
                    i.text(app.phase.key())
                ),
                theme::fg(tone).add_modifier(ratatui::style::Modifier::BOLD),
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
                theme::fg(Tone::Info),
            ))
            .right_aligned(),
        );
    }
    if app.log_modal {
        let hints = [("PgUp/PgDn", i.text("key.scroll"))];
        let mut line = theme::key_hints(&hints, Some(("Esc", i.text("key.back"))), 48);
        line.spans.insert(0, Span::raw(" "));
        line.spans.push(Span::raw(" "));
        block = block.title_bottom(line);
    }
    let lines = if rows.is_empty() {
        vec![Line::styled(
            i.text("ui.empty_log").to_owned(),
            theme::muted(),
        )]
    } else {
        rows[start..end]
            .iter()
            .map(|(text, tone)| Line::styled(text.clone(), theme::fg(*tone)))
            .collect()
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
    theme::scrollbar(f, theme::track(area, body), rows.len(), visible, start);
}
