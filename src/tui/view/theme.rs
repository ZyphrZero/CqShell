//! Design tokens and styled primitives. Panels inherit the terminal's own foreground and
//! background; color is reserved for focus, state and severity.
use crate::tui::{Phase, Tone, motion};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Padding, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

pub(super) const TEXT: Color = Color::Reset;
pub(super) const MUTED: Color = Color::Rgb(145, 162, 176);
pub(super) const BORDER: Color = Color::Rgb(61, 82, 101);
pub(super) const ACCENT: Color = Color::Rgb(69, 205, 182);
const INFO: Color = Color::Rgb(112, 170, 255);
const SUCCESS: Color = Color::Rgb(93, 216, 136);
const WARNING: Color = Color::Rgb(255, 190, 94);
const DANGER: Color = Color::Rgb(255, 117, 133);
/// Text on a tone background; black stays legible on every tone above in dark and light themes.
pub(super) const ON_TONE: Color = Color::Black;

pub(super) fn color(tone: Tone) -> Color {
    match tone {
        Tone::Plain => TEXT,
        Tone::Muted => MUTED,
        Tone::Accent => ACCENT,
        Tone::Info => INFO,
        Tone::Success => SUCCESS,
        Tone::Warning => WARNING,
        Tone::Danger => DANGER,
    }
}
pub(super) fn fg(tone: Tone) -> Style {
    Style::default().fg(color(tone))
}
pub(super) fn muted() -> Style {
    fg(Tone::Muted)
}
pub(super) fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
pub(super) fn icon(tone: Tone) -> &'static str {
    match tone {
        Tone::Success => "✓",
        Tone::Danger => "✗",
        Tone::Warning => "!",
        _ => "•",
    }
}
/// Live phases spin; settled phases keep a stable glyph so the outcome stays scannable.
pub(super) fn phase_icon(phase: Phase, elapsed: Duration) -> &'static str {
    match phase {
        Phase::Running | Phase::Cancelling => motion::spinner(elapsed),
        Phase::Completed => "✓",
        Phase::Failed => "✗",
        Phase::Cancelled => "■",
        Phase::Idle => "•",
    }
}

pub(super) fn title(text: impl Into<String>, tone: Tone) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {} ", text.into()),
        fg(tone).add_modifier(Modifier::BOLD),
    ))
}
/// Dialog titles lead with a tone glyph so severity is readable without color.
pub(super) fn icon_title(glyph: &str, text: &str, tone: Tone) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {glyph} "), fg(tone).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{text} "), bold()),
    ])
}
/// Rounded panel; the border and title carry focus.
pub(super) fn panel(text: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .style(Style::default().fg(TEXT))
        .title(title(
            text,
            if focused { Tone::Accent } else { Tone::Plain },
        ))
}
/// Dialog frame: tone-colored border, breathing room inside, terminal colors preserved.
pub(super) fn dialog(heading: Line<'static>, tone: Tone) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(fg(tone))
        .style(Style::default().fg(TEXT))
        .title(heading)
        .padding(Padding::new(2, 2, 1, 0))
}
pub(super) fn badge(text: &str, tone: Tone) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::default()
            .fg(ON_TONE)
            .bg(color(tone))
            .add_modifier(Modifier::BOLD),
    )
}

/// A key chip and its label, e.g. `("Enter", "打开")`.
pub(super) type Hint<'a> = (&'a str, &'a str);
const HINT_GAP: usize = 2;
fn hint_width((key, label): Hint<'_>) -> usize {
    key.width() + 1 + label.width()
}
fn push_hint(spans: &mut Vec<Span<'static>>, (key, label): Hint<'_>) {
    if !spans.is_empty() {
        spans.push(Span::raw(" ".repeat(HINT_GAP)));
    }
    spans.push(Span::styled(
        key.to_owned(),
        fg(Tone::Accent).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(format!(" {label}"), muted()));
}
/// Hints are listed by priority and dropped from the tail when space runs out. `pinned`
/// (help or close) always survives so the complete reference stays one key away.
pub(super) fn key_hints(
    hints: &[Hint<'_>],
    pinned: Option<Hint<'_>>,
    width: usize,
) -> Line<'static> {
    let mut budget = width.saturating_sub(pinned.map_or(0, |hint| hint_width(hint) + HINT_GAP));
    let mut spans = Vec::new();
    for &hint in hints {
        let cost = hint_width(hint) + if spans.is_empty() { 0 } else { HINT_GAP };
        if cost > budget {
            break;
        }
        budget -= cost;
        push_hint(&mut spans, hint);
    }
    if let Some(hint) = pinned {
        push_hint(&mut spans, hint);
    }
    Line::from(spans)
}

/// Half-cell precision bar: a heavy line for the fill and a thin track for the remainder.
pub(super) fn bar(percent: f64, width: usize, tone: Tone) -> Vec<Span<'static>> {
    let halves = (percent.clamp(0.0, 100.0) / 100.0 * width as f64 * 2.0).round() as usize;
    let (full, half) = (halves / 2, halves % 2);
    let mut spans = vec![Span::styled("━".repeat(full), fg(tone))];
    if half == 1 {
        spans.push(Span::styled("╸", fg(tone)));
    }
    spans.push(Span::styled(
        "─".repeat(width.saturating_sub(full + half)),
        Style::default().fg(BORDER),
    ));
    spans
}
pub(super) fn load_tone(percent: f64) -> Tone {
    if percent >= 90.0 {
        Tone::Danger
    } else if percent >= 70.0 {
        Tone::Warning
    } else {
        Tone::Success
    }
}

/// Recede everything outside `keep` so a dialog reads as the only active layer. Colors are
/// rewritten instead of relying on the DIM attribute, which many terminals ignore; the
/// terminal background is preserved.
pub(super) fn scrim(buffer: &mut Buffer, keep: Rect) {
    let area = *buffer.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if keep.contains(Position { x, y }) {
                continue;
            }
            let cell = &mut buffer[(x, y)];
            cell.fg = BORDER;
            cell.bg = Color::Reset;
            cell.modifier = Modifier::empty();
        }
    }
}
/// Linear RGB blend used for fades; non-RGB colors switch at the midpoint.
pub(super) fn mix(from: Color, to: Color, amount: f64) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let channel = |a: u8, b: u8| {
                (f64::from(a) + (f64::from(b) - f64::from(a)) * amount).round() as u8
            };
            Color::Rgb(channel(r1, r2), channel(g1, g2), channel(b1, b2))
        }
        _ if amount < 0.5 => from,
        _ => to,
    }
}
/// The border column beside `rows`, where a panel's scrollbar runs.
pub(super) fn track(panel: Rect, rows: Rect) -> Rect {
    Rect::new(panel.right().saturating_sub(1), rows.y, 1, rows.height)
}
/// Scrollbar drawn into `track`; nothing is drawn when the content fits.
pub(super) fn scrollbar(frame: &mut Frame, track: Rect, total: usize, visible: usize, top: usize) {
    if total <= visible || track.height < 2 {
        return;
    }
    let mut state = ScrollbarState::new(total - visible + 1)
        .position(top)
        .viewport_content_length(visible);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(BORDER))
            .thumb_symbol("┃")
            .thumb_style(fg(Tone::Accent)),
        track,
        &mut state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn key_hints_drop_low_priority_entries_but_keep_the_pinned_one() {
        let hints = [("↑↓", "move"), ("Enter", "open"), ("Tab", "focus")];
        let wide = text(&key_hints(&hints, Some(("?", "help")), 80));
        assert_eq!(wide, "↑↓ move  Enter open  Tab focus  ? help");
        // "↑↓ move  Enter open  ? help" is exactly 27 cells.
        let exact = text(&key_hints(&hints, Some(("?", "help")), 27));
        assert_eq!(exact, "↑↓ move  Enter open  ? help");
        let narrow = text(&key_hints(&hints, Some(("?", "help")), 26));
        assert_eq!(narrow, "↑↓ move  ? help");
        let tiny = text(&key_hints(&hints, Some(("?", "help")), 6));
        assert_eq!(tiny, "? help");
    }

    #[test]
    fn bars_fill_by_half_cells_and_keep_their_width() {
        let width = |spans: &[Span<'_>]| spans.iter().map(|s| s.content.width()).sum::<usize>();
        for percent in [0.0, 12.5, 50.0, 99.0, 100.0, 140.0] {
            assert_eq!(width(&bar(percent, 8, Tone::Success)), 8, "{percent}");
        }
        let half = bar(6.25, 8, Tone::Success);
        assert_eq!(half[1].content, "╸");
        assert_eq!(load_tone(95.0), Tone::Danger);
        assert_eq!(load_tone(75.0), Tone::Warning);
        assert_eq!(load_tone(10.0), Tone::Success);
    }

    #[test]
    fn scrim_recolors_the_background_without_painting_it() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        buffer[(0, 0)].set_symbol("x").set_fg(ACCENT).set_bg(ACCENT);
        buffer[(3, 1)].set_symbol("y").set_fg(ACCENT);
        scrim(&mut buffer, Rect::new(3, 1, 1, 1));
        assert_eq!(buffer[(0, 0)].symbol(), "x");
        assert_eq!(buffer[(0, 0)].fg, BORDER);
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        assert_eq!(
            buffer[(3, 1)].fg,
            ACCENT,
            "the kept dialog area is untouched"
        );
        assert_eq!(
            mix(Color::Rgb(0, 0, 0), Color::Rgb(100, 200, 50), 0.5),
            Color::Rgb(50, 100, 25)
        );
    }
}
