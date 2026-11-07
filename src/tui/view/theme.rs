//! Styled primitives over the active theme. Every color is a palette role of the `Theme`
//! passed in, so no color is named here. Panels inherit the terminal's own foreground and
//! background; color is reserved for focus, state and severity.
use crate::{
    theme::{
        self as model, Ansi16, BackgroundStyle, BarStyle, BorderStyle, CheckboxStyle, DotStyle,
        GlyphSet, KeyStyle, Level, Role, RuleStyle, SelectionStyle, SparklineStyle, TertiaryStyle,
        Theme, TitleStyle,
    },
    tui::{Phase, Tone, motion},
};
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

/// The ratatui color for a theme color. `Default` is the terminal's own color; ANSI colors
/// keep their palette index, so ratatui's `Gray` is ANSI white (7) and `White` is bright
/// white (15).
fn ratatui_color(color: model::Color) -> Color {
    match color {
        model::Color::Default => Color::Reset,
        model::Color::Ansi(ansi) => match ansi {
            Ansi16::Black => Color::Black,
            Ansi16::Red => Color::Red,
            Ansi16::Green => Color::Green,
            Ansi16::Yellow => Color::Yellow,
            Ansi16::Blue => Color::Blue,
            Ansi16::Magenta => Color::Magenta,
            Ansi16::Cyan => Color::Cyan,
            Ansi16::White => Color::Gray,
            Ansi16::BrightBlack => Color::DarkGray,
            Ansi16::BrightRed => Color::LightRed,
            Ansi16::BrightGreen => Color::LightGreen,
            Ansi16::BrightYellow => Color::LightYellow,
            Ansi16::BrightBlue => Color::LightBlue,
            Ansi16::BrightMagenta => Color::LightMagenta,
            Ansi16::BrightCyan => Color::LightCyan,
            Ansi16::BrightWhite => Color::White,
        },
        model::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
pub(super) fn color(theme: &Theme, role: Role) -> Color {
    ratatui_color(theme.palette.get(role))
}
pub(super) fn fg(theme: &Theme, role: Role) -> Style {
    Style::default().fg(color(theme, role))
}
/// Text at a Text_Level; tertiary text takes DIM in themes that draw it that way.
pub(super) fn text(theme: &Theme, level: Level) -> Style {
    let style = fg(theme, level.role());
    if level == Level::Tertiary && theme.patterns.tertiary == TertiaryStyle::Dim {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}
/// The role a semantic tone draws in. Muted text is secondary, not tertiary: it carries
/// labels, hints and descriptions that have to stay readable.
pub(super) fn role(tone: Tone) -> Role {
    match tone {
        Tone::Plain => Role::Text1,
        Tone::Muted => Role::Text2,
        Tone::Accent => Role::Accent,
        Tone::Info => Role::Info,
        Tone::Success => Role::Success,
        Tone::Warning => Role::Warning,
        Tone::Danger => Role::Danger,
    }
}
/// Foreground for a semantic tone; plain and muted text go through their Text_Level.
pub(super) fn tone(theme: &Theme, tone: Tone) -> Style {
    if theme.patterns.glyphs == GlyphSet::Ascii && tone == Tone::Info {
        return text(theme, Level::Primary);
    }
    if (unframed(theme) || theme.patterns.glyphs == GlyphSet::Ascii)
        && matches!(tone, Tone::Success | Tone::Warning | Tone::Danger)
    {
        return text(theme, Level::Primary);
    }
    match tone {
        Tone::Plain => text(theme, Level::Primary),
        Tone::Muted => text(theme, Level::Secondary),
        other => fg(theme, role(other)),
    }
}
pub(super) fn muted(theme: &Theme) -> Style {
    text(theme, Level::Secondary)
}
pub(super) fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
pub(super) fn icon(theme: &Theme, tone: Tone) -> &'static str {
    match tone {
        Tone::Success => glyph(theme, Glyph::Success),
        Tone::Danger => glyph(theme, Glyph::Failure),
        Tone::Warning => glyph(theme, Glyph::Warning),
        _ if theme.patterns.glyphs == GlyphSet::Ascii => ".",
        _ => "•",
    }
}
/// Live phases spin; settled phases keep a stable glyph so the outcome stays scannable.
pub(super) fn phase_icon(theme: &Theme, phase: Phase, elapsed: Duration) -> &'static str {
    match phase {
        Phase::Running | Phase::Cancelling => spinner(theme, elapsed),
        Phase::Completed => glyph(theme, Glyph::Success),
        Phase::Failed => glyph(theme, Glyph::Failure),
        Phase::Cancelled => glyph(theme, Glyph::Cancelled),
        Phase::Idle => icon(theme, Tone::Plain),
    }
}

pub(super) fn title(theme: &Theme, text: impl Into<String>, role: Role) -> Line<'static> {
    let text = text.into();
    let title = match theme.patterns.title {
        TitleStyle::Bracket => format!("[ {text} ]"),
        TitleStyle::Arrow => format!(" {} {text} ", glyph(theme, Glyph::Execute)),
        _ => format!(" {text} "),
    };
    let style = if theme.patterns.title == TitleStyle::Label {
        self::text(theme, Level::Tertiary)
    } else {
        fg(theme, role).add_modifier(Modifier::BOLD)
    };
    Line::from(Span::styled(title, style))
}
/// Dialog titles lead with a tone glyph so severity is readable without color.
pub(super) fn icon_title(theme: &Theme, glyph: &str, text: &str, role: Role) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {glyph} "),
            fg(theme, role).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{text} "), bold()),
    ])
}
/// Rounded panel; a `focused` panel, one that owns input, draws its border and title in the
/// focus colors. Main_View navigation has no focus, so its panels are never focused.
pub(super) fn panel(theme: &Theme, label: impl Into<String>, focused: bool) -> Block<'static> {
    let (border, heading) = if focused {
        (Role::BorderFocus, Role::Accent)
    } else {
        (Role::Border, Role::Text1)
    };
    Block::default()
        .borders(if unframed(theme) {
            Borders::NONE
        } else {
            Borders::ALL
        })
        .border_set(border_set(theme))
        .border_style(fg(theme, border))
        .padding(Padding::new(0, u16::from(unframed(theme)), 0, 0))
        .style(text(theme, Level::Primary).bg(color(theme, Role::Surface)))
        .title(title(theme, label, heading))
}

pub(super) fn unframed(theme: &Theme) -> bool {
    matches!(
        theme.patterns.border,
        BorderStyle::Card | BorderStyle::Titlebar
    )
}

pub(super) fn padding(theme: &Theme, width: u16) -> u16 {
    if unframed(theme) && width >= 56 {
        2
    } else if unframed(theme) {
        0
    } else {
        1
    }
}

pub(super) fn panel_padding(theme: &Theme, width: u16) -> Padding {
    let horizontal = padding(theme, width);
    Padding::horizontal(horizontal)
}

pub(super) fn shadow(theme: &Theme, frame: &mut Frame, rect: Rect) {
    if theme.palette.dialog == model::Color::Default {
        return;
    }
    let area = frame.area();
    for row in rect.top() + 1..=rect.bottom().min(area.bottom().saturating_sub(1)) {
        if rect.right() < area.right() {
            frame.buffer_mut()[(rect.right(), row)]
                .set_symbol(" ")
                .set_bg(color(theme, Role::Shadow));
        }
    }
    if rect.bottom() < area.bottom() {
        for column in rect.left() + 1..rect.right().min(area.right()) {
            frame.buffer_mut()[(column, rect.bottom())]
                .set_symbol(" ")
                .set_bg(color(theme, Role::Shadow));
        }
    }
}

pub(super) fn border_set(theme: &Theme) -> ratatui::symbols::border::Set<'static> {
    use ratatui::symbols::border;
    match theme.patterns.border {
        BorderStyle::Square => border::PLAIN,
        BorderStyle::Double => border::DOUBLE,
        BorderStyle::Heavy => border::THICK,
        BorderStyle::Ascii => border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        },
        _ => border::ROUNDED,
    }
}

pub(super) fn joined_border_set(theme: &Theme) -> ratatui::symbols::border::Set<'static> {
    let (left, right) = match theme.patterns.border {
        BorderStyle::Ascii => ("+", "+"),
        BorderStyle::Double => ("╠", "╣"),
        BorderStyle::Heavy => ("┣", "┫"),
        _ => ("├", "┤"),
    };
    ratatui::symbols::border::Set {
        top_left: left,
        top_right: right,
        ..border_set(theme)
    }
}

pub(super) fn selection_marker(theme: &Theme) -> &'static str {
    match theme.patterns.selection {
        SelectionStyle::Bar if theme.patterns.glyphs != GlyphSet::Ascii => "▎ ",
        SelectionStyle::Arrow if theme.patterns.glyphs != GlyphSet::Ascii => "▸ ",
        SelectionStyle::Inverse => "> ",
        _ if theme.patterns.glyphs == GlyphSet::Ascii => "> ",
        _ => "› ",
    }
}

pub(in crate::tui) fn spinner(theme: &Theme, elapsed: Duration) -> &'static str {
    if theme.patterns.glyphs == GlyphSet::Ascii {
        ["-", "\\", "|", "/"][(elapsed.as_millis() / 80 % 4) as usize]
    } else {
        motion::spinner(elapsed)
    }
}
/// The frame every dialog shares: rounded border and breathing room inside. Layout uses it
/// bare, since colors never move content.
pub(super) fn dialog_frame() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::new(2, 2, 1, 0))
}
/// Dialog frame: role-colored border, terminal colors preserved inside.
pub(super) fn dialog(theme: &Theme, heading: Line<'static>, role: Role) -> Block<'static> {
    let role = if (unframed(theme) || theme.patterns.glyphs == GlyphSet::Ascii)
        && matches!(
            role,
            Role::Success | Role::Warning | Role::Danger | Role::Info
        ) {
        Role::Border
    } else {
        role
    };
    dialog_frame()
        .border_set(border_set(theme))
        .border_style(fg(theme, role))
        .style(text(theme, Level::Primary).bg(color(theme, Role::Dialog)))
        .title(heading)
}
/// A symbol drawn by the interface rather than taken from a locale file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glyph {
    /// Joins two Segments.
    Separator,
    /// Marks a row or a step of the breadcrumb.
    Mark,
    /// The status dot of something on, in the theme's dot pattern; also the chosen option of
    /// a Chip_Field.
    On,
    /// The status dot of something off, in the theme's dot pattern; also an option of a
    /// Chip_Field that is not chosen.
    Off,
    /// The Enter key in a hint.
    Enter,
    /// After the name of an action that hands the terminal to another program.
    External,
    /// After the name of a Form_Action.
    Ellipsis,
    /// A Last_Result that completed.
    Success,
    /// A Last_Result that failed.
    Failure,
    /// A Last_Result that was cancelled.
    Cancelled,
    /// A state that needs a look, such as a Mihomo process that cannot be verified.
    Warning,
    Checked,
    Unchecked,
    Execute,
    Previous,
    Next,
    Pending,
    Skipped,
    Swatch,
    UpDown,
    LeftRight,
}
/// `glyph` in the theme's glyph set (design 13.6), or for the status dot in its dot
/// pattern.
pub(super) fn glyph(theme: &Theme, glyph: Glyph) -> &'static str {
    let ascii = theme.patterns.glyphs == GlyphSet::Ascii;
    match glyph {
        Glyph::Separator if ascii => "|",
        Glyph::Separator => "·",
        Glyph::Mark if ascii => ">",
        Glyph::Mark => "›",
        Glyph::External if ascii => "^",
        Glyph::External => "↗",
        Glyph::Ellipsis if ascii => "...",
        Glyph::Ellipsis => "…",
        Glyph::Success if ascii => "+",
        Glyph::Success => "✓",
        Glyph::Failure if ascii => "x",
        Glyph::Failure => "✗",
        Glyph::Cancelled if ascii => "=",
        Glyph::Cancelled => "■",
        Glyph::Warning => "!",
        Glyph::Checked if ascii => "[x]",
        Glyph::Checked => match theme.patterns.checkbox {
            CheckboxStyle::Box => "[✓]",
            CheckboxStyle::Filled => "[■]",
            CheckboxStyle::Diamond => "◆",
            CheckboxStyle::Radio => "◉",
            CheckboxStyle::Square => "■",
            CheckboxStyle::Ascii => "[x]",
        },
        Glyph::Unchecked => match theme.patterns.checkbox {
            CheckboxStyle::Diamond => "◇",
            CheckboxStyle::Radio => "○",
            CheckboxStyle::Square => "□",
            _ => "[ ]",
        },
        Glyph::Pending if ascii => ".",
        Glyph::Pending => "○",
        Glyph::Skipped if ascii => "-",
        Glyph::Skipped => "–",
        Glyph::Swatch if ascii => "##",
        Glyph::Swatch => "▬▬",
        Glyph::UpDown if ascii => "Up/Dn",
        Glyph::UpDown => "↑↓",
        Glyph::LeftRight if ascii => "Lt/Rt",
        Glyph::LeftRight => "←→",
        Glyph::Execute if ascii => ">",
        Glyph::Execute => "▶",
        Glyph::Previous if ascii => "<",
        Glyph::Previous => "‹",
        Glyph::Next if ascii => ">",
        Glyph::Next => "›",
        Glyph::Enter if ascii => "Enter",
        Glyph::Enter => "↵",
        Glyph::On => match theme.patterns.dot {
            DotStyle::Circle => "●",
            DotStyle::Diamond => "◆",
            DotStyle::Square => "■",
            DotStyle::Ascii => "*",
        },
        Glyph::Off => match theme.patterns.dot {
            DotStyle::Circle => "○",
            DotStyle::Diamond => "◇",
            DotStyle::Square => "□",
            DotStyle::Ascii => "o",
        },
    }
}

/// A key chip and its label, e.g. `("Enter", "打开")`.
pub(super) type Hint<'a> = (&'a str, &'a str);
const HINT_GAP: usize = 2;
pub(super) fn keycap(theme: &Theme, key: &str) -> Span<'static> {
    let key = if theme.patterns.glyphs == GlyphSet::Ascii {
        key.replace("↑↓", glyph(theme, Glyph::UpDown))
            .replace("←→", glyph(theme, Glyph::LeftRight))
            .replace('–', "-")
            .replace('…', "...")
    } else if key == "Enter" && theme.patterns.keys == KeyStyle::Keycap {
        glyph(theme, Glyph::Enter).into()
    } else {
        key.into()
    };
    let (key, style) = match theme.patterns.keys {
        KeyStyle::Keycap => (
            format!(" {key} "),
            fg(theme, Role::Accent)
                .bg(color(theme, Role::SurfaceAlt))
                .add_modifier(Modifier::BOLD),
        ),
        KeyStyle::Bracket => (
            format!("[{}]", key.to_ascii_uppercase()),
            fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
        ),
        KeyStyle::Plain => (key, fg(theme, Role::Accent).add_modifier(Modifier::BOLD)),
    };
    Span::styled(key, style)
}
fn hint_width(theme: &Theme, (key, label): Hint<'_>) -> usize {
    keycap(theme, key).width() + 1 + label.width()
}
fn push_hint(theme: &Theme, spans: &mut Vec<Span<'static>>, (key, label): Hint<'_>) {
    if !spans.is_empty() {
        spans.push(Span::raw(" ".repeat(HINT_GAP)));
    }
    spans.push(keycap(theme, key));
    spans.push(Span::styled(format!(" {label}"), muted(theme)));
}
/// Hints are listed by priority and dropped from the tail when space runs out. `pinned`
/// (help or close) always survives so the complete reference stays one key away.
pub(super) fn key_hints(
    theme: &Theme,
    hints: &[Hint<'_>],
    pinned: Option<Hint<'_>>,
    width: usize,
) -> Line<'static> {
    let mut budget =
        width.saturating_sub(pinned.map_or(0, |hint| hint_width(theme, hint) + HINT_GAP));
    let mut spans = Vec::new();
    for &hint in hints {
        let cost = hint_width(theme, hint) + if spans.is_empty() { 0 } else { HINT_GAP };
        if cost > budget {
            break;
        }
        budget -= cost;
        push_hint(theme, &mut spans, hint);
    }
    if let Some(hint) = pinned {
        push_hint(theme, &mut spans, hint);
    }
    Line::from(spans)
}

/// Half-cell precision bar: a heavy line in `role` for the fill and a thin rule-colored
/// track for the remainder. The fill follows `percent`, which may be an animated value,
/// while `role` follows the measured level.
pub(super) fn bar(theme: &Theme, percent: f64, width: usize, role: Role) -> Vec<Span<'static>> {
    let halves = (percent.clamp(0.0, 100.0) / 100.0 * width as f64 * 2.0).round() as usize;
    let (full, half) = (halves / 2, halves % 2);
    let (filled, partial, empty) = match theme.patterns.bar {
        BarStyle::Line => ("━", "╸", "─"),
        BarStyle::Block => ("█", "▌", "░"),
        BarStyle::Segment => ("▰", "▰", "▱"),
        BarStyle::Square => ("■", "■", "□"),
        BarStyle::Hash => ("#", "#", "-"),
    };
    let gradient = theme.patterns.gradient
        && percent < 70.0
        && matches!(role, Role::Meter | Role::Accent)
        && matches!(
            (theme.palette.accent, theme.palette.accent2),
            (model::Color::Rgb(..), model::Color::Rgb(..))
        );
    let fill_style = |index: usize| {
        if gradient {
            Style::default().fg(mix(
                color(theme, Role::Accent),
                color(theme, Role::Accent2),
                index as f64 / (full + half).saturating_sub(1).max(1) as f64,
            ))
        } else {
            fg(theme, role)
        }
    };
    let mut spans = if gradient {
        (0..full)
            .map(|index| Span::styled(filled, fill_style(index)))
            .collect()
    } else {
        vec![Span::styled(filled.repeat(full), fg(theme, role))]
    };
    if half == 1 {
        spans.push(Span::styled(partial, fill_style(full)));
    }
    spans.push(Span::styled(
        empty.repeat(width.saturating_sub(full + half)),
        fg(theme, Role::Rule),
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
/// A gauge's color by load: the theme's `meter` below the warning threshold, then the
/// warning and danger tones of `load_tone`.
pub(super) fn gauge(percent: f64) -> Role {
    match load_tone(percent) {
        Tone::Success => Role::Meter,
        other => role(other),
    }
}

/// Which of `levels` equal steps of 0–100 % `sample` falls in.
fn level(sample: f32, levels: usize) -> usize {
    ((sample.clamp(0.0, 100.0) / 100.0 * levels as f32) as usize).min(levels - 1)
}
/// The newest percentage `samples` (oldest first) as a sparkline exactly `width` cells wide
/// in the theme's sparkline pattern, newest at the right. A short history leaves the left
/// end blank. Blocks and ASCII draw one sample per cell in eight levels; braille draws two
/// per cell, each a column of one to four dots rising from the bottom.
pub(super) fn sparkline(theme: &Theme, samples: &[f32], width: usize) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    const ASCII: [char; 8] = ['.', ':', '-', '=', '+', '*', '#', '@'];
    // Dot bits of a braille cell's left and right columns, bottom row first.
    const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
    let glyphs = match theme.patterns.sparkline {
        SparklineStyle::Blocks => BLOCKS,
        SparklineStyle::Ascii => ASCII,
        SparklineStyle::Braille => {
            let newest = &samples[samples.len().saturating_sub(2 * width)..];
            let blank = 2 * width - newest.len();
            let dots = |slot: usize, column: &[u32; 4]| {
                slot.checked_sub(blank).map_or(0, |index| {
                    column[..=level(newest[index], 4)].iter().sum::<u32>()
                })
            };
            return (0..width)
                .map(
                    |cell| match dots(2 * cell, &LEFT) | dots(2 * cell + 1, &RIGHT) {
                        0 => ' ',
                        bits => char::from_u32(0x2800 + bits).expect("braille patterns are chars"),
                    },
                )
                .collect();
        }
    };
    let newest = &samples[samples.len().saturating_sub(width)..];
    let mut line = " ".repeat(width - newest.len());
    line.extend(
        newest
            .iter()
            .map(|sample| glyphs[level(*sample, glyphs.len())]),
    );
    line
}

/// Cells outside every Surface: the terminal's own background, or the theme's painted one.
fn backdrop(theme: &Theme) -> Color {
    match theme.patterns.background {
        BackgroundStyle::Terminal => ratatui_color(model::Color::Default),
        BackgroundStyle::Paint => color(theme, Role::Background),
    }
}
/// Recede everything outside `keep` so a dialog reads as the only active layer. Colors are
/// rewritten to the `dim` role instead of relying on the DIM attribute, which many
/// terminals ignore; the background stays the theme's backdrop.
pub(super) fn scrim(theme: &Theme, buffer: &mut Buffer, keep: Rect) {
    let (dim, backdrop) = (color(theme, Role::Dim), backdrop(theme));
    let area = *buffer.area();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if keep.contains(Position { x, y }) {
                continue;
            }
            let cell = &mut buffer[(x, y)];
            cell.fg = dim;
            cell.bg = backdrop;
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
/// A rule `width` cells long in the theme's rule pattern and color; `none` leaves it blank.
pub(super) fn rule(theme: &Theme, width: usize) -> Span<'static> {
    let glyph = match theme.patterns.rule {
        RuleStyle::Solid => "─",
        RuleStyle::Dotted => "·",
        RuleStyle::Dashed => "┄",
        RuleStyle::Ascii => "-",
        RuleStyle::None => " ",
    };
    Span::styled(glyph.repeat(width), fg(theme, Role::Rule))
}
/// The border column beside `rows`, where a panel's scrollbar runs.
pub(super) fn track(panel: Rect, rows: Rect) -> Rect {
    Rect::new(panel.right().saturating_sub(1), rows.y, 1, rows.height)
}
/// Scrollbar drawn into `track`; nothing is drawn when the content fits. The track takes
/// the border's place, so it keeps the border color.
pub(super) fn scrollbar(
    theme: &Theme,
    frame: &mut Frame,
    track: Rect,
    total: usize,
    visible: usize,
    top: usize,
) {
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
            .track_symbol(Some(if theme.patterns.glyphs == GlyphSet::Ascii {
                "|"
            } else {
                "│"
            }))
            .track_style(fg(theme, Role::Border))
            .thumb_symbol(if theme.patterns.glyphs == GlyphSet::Ascii {
                "#"
            } else {
                "┃"
            })
            .thumb_style(fg(theme, Role::Accent)),
        track,
        &mut state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Background, builtin};

    fn classic_dark() -> Theme {
        model::resolve(&builtin::classic(), &Background::Disabled)
    }
    fn content(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn theme_colors_map_to_ratatui_colors() {
        assert_eq!(ratatui_color(model::Color::Default), Color::Reset);
        assert_eq!(
            ratatui_color(model::Color::Rgb(1, 2, 3)),
            Color::Rgb(1, 2, 3)
        );
        let by_index = [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ];
        for (ansi, expected) in Ansi16::ALL.into_iter().zip(by_index) {
            assert_eq!(
                ratatui_color(model::Color::Ansi(ansi)),
                expected,
                "{ansi:?}"
            );
        }
    }

    #[test]
    fn text_levels_read_their_role_and_tertiary_text_may_dim() {
        let mut theme = classic_dark();
        theme.palette.text3 = model::Color::Rgb(9, 9, 9);
        assert_eq!(text(&theme, Level::Tertiary), fg(&theme, Role::Text3));
        assert_eq!(muted(&theme), fg(&theme, Role::Text2));
        assert_eq!(tone(&theme, Tone::Plain), fg(&theme, Role::Text1));
        theme.patterns.tertiary = TertiaryStyle::Dim;
        assert_eq!(
            text(&theme, Level::Tertiary),
            fg(&theme, Role::Text3).add_modifier(Modifier::DIM)
        );
        assert_eq!(text(&theme, Level::Secondary), fg(&theme, Role::Text2));
    }

    #[test]
    fn glyphs_follow_the_glyph_set_and_the_dot_follows_its_pattern() {
        let mut theme = classic_dark();
        let all = [
            Glyph::Separator,
            Glyph::Mark,
            Glyph::On,
            Glyph::Off,
            Glyph::Enter,
            Glyph::External,
            Glyph::Ellipsis,
            Glyph::Success,
            Glyph::Failure,
            Glyph::Cancelled,
            Glyph::Warning,
        ];
        assert_eq!(
            all.map(|g| glyph(&theme, g)),
            ["·", "›", "●", "○", "↵", "↗", "…", "✓", "✗", "■", "!"]
        );
        theme.patterns.glyphs = GlyphSet::Ascii;
        assert_eq!(
            all.map(|g| glyph(&theme, g)),
            ["|", ">", "●", "○", "Enter", "^", "...", "+", "x", "=", "!"]
        );
        for (dot, on, off) in [
            (DotStyle::Circle, "●", "○"),
            (DotStyle::Diamond, "◆", "◇"),
            (DotStyle::Square, "■", "□"),
            (DotStyle::Ascii, "*", "o"),
        ] {
            theme.patterns.dot = dot;
            assert_eq!(glyph(&theme, Glyph::On), on, "{dot:?}");
            assert_eq!(glyph(&theme, Glyph::Off), off, "{dot:?}");
        }
    }

    #[test]
    fn key_hints_drop_low_priority_entries_but_keep_the_pinned_one() {
        let theme = classic_dark();
        let hints = [("↑↓", "move"), ("Enter", "open"), ("Tab", "focus")];
        let wide = content(&key_hints(&theme, &hints, Some(("?", "help")), 80));
        assert_eq!(wide, "↑↓ move  Enter open  Tab focus  ? help");
        // "↑↓ move  Enter open  ? help" is exactly 27 cells.
        let exact = content(&key_hints(&theme, &hints, Some(("?", "help")), 27));
        assert_eq!(exact, "↑↓ move  Enter open  ? help");
        let narrow = content(&key_hints(&theme, &hints, Some(("?", "help")), 26));
        assert_eq!(narrow, "↑↓ move  ? help");
        let tiny = content(&key_hints(&theme, &hints, Some(("?", "help")), 6));
        assert_eq!(tiny, "? help");
    }

    #[test]
    fn bars_fill_by_half_cells_and_keep_their_width() {
        let theme = classic_dark();
        let width = |spans: &[Span<'_>]| spans.iter().map(|s| s.content.width()).sum::<usize>();
        for percent in [0.0, 12.5, 50.0, 99.0, 100.0, 140.0] {
            assert_eq!(width(&bar(&theme, percent, 8, Role::Meter)), 8, "{percent}");
        }
        let half = bar(&theme, 6.25, 8, Role::Meter);
        assert_eq!(half[1].content, "╸");
        assert_eq!(half[0].style, fg(&theme, Role::Meter));
        assert_eq!(half[2].style, fg(&theme, Role::Rule));
        assert_eq!(load_tone(95.0), Tone::Danger);
        assert_eq!(load_tone(75.0), Tone::Warning);
        assert_eq!(load_tone(10.0), Tone::Success);
        assert_eq!(gauge(95.0), Role::Danger);
        assert_eq!(gauge(75.0), Role::Warning);
        assert_eq!(gauge(10.0), Role::Meter);
    }

    #[test]
    fn sparklines_draw_the_newest_samples_in_the_theme_pattern() {
        let mut theme = classic_dark();
        let history = [
            22.0, 35.0, 61.0, 48.0, 30.0, 18.0, 25.0, 12.0, 9.0, 14.0, 20.0, 12.0,
        ];
        assert_eq!(sparkline(&theme, &history, 12), "▂▃▅▄▃▂▃▁▁▂▂▁");
        assert_eq!(sparkline(&theme, &history, 4), "▁▂▂▁", "the newest samples");
        assert_eq!(
            sparkline(&theme, &history[..2], 4),
            "  ▂▃",
            "blank before the oldest"
        );
        // Eight equal steps; values outside 0–100 % are clamped.
        let edges = [0.0, 12.4, 12.5, 50.0, 87.5, 100.0, 140.0, -3.0];
        assert_eq!(sparkline(&theme, &edges, 8), "▁▁▂▅███▁");
        assert_eq!(sparkline(&theme, &[], 3), "   ");
        assert_eq!(sparkline(&theme, &history, 0), "");

        theme.patterns.sparkline = SparklineStyle::Ascii;
        assert_eq!(sparkline(&theme, &[0.0, 50.0, 100.0], 4), " .+@");

        // Two samples per cell, one to four dots each, rising from the bottom.
        theme.patterns.sparkline = SparklineStyle::Braille;
        assert_eq!(sparkline(&theme, &[0.0, 100.0], 1), "\u{28f8}");
        assert_eq!(sparkline(&theme, &[30.0], 2), " \u{28a0}");
        assert_eq!(
            sparkline(&theme, &[60.0, 99.0, 10.0], 2),
            "\u{28b0}\u{28c7}"
        );
        for style in [
            SparklineStyle::Blocks,
            SparklineStyle::Braille,
            SparklineStyle::Ascii,
        ] {
            theme.patterns.sparkline = style;
            for width in 0..30 {
                assert_eq!(
                    sparkline(&theme, &history, width).width(),
                    width,
                    "{style:?}"
                );
            }
        }
    }

    #[test]
    fn scrim_recolors_the_background_without_painting_it() {
        let theme = classic_dark();
        let accent = color(&theme, Role::Accent);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        buffer[(0, 0)].set_symbol("x").set_fg(accent).set_bg(accent);
        buffer[(3, 1)].set_symbol("y").set_fg(accent);
        scrim(&theme, &mut buffer, Rect::new(3, 1, 1, 1));
        assert_eq!(buffer[(0, 0)].symbol(), "x");
        assert_eq!(buffer[(0, 0)].fg, color(&theme, Role::Dim));
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        assert_eq!(
            buffer[(3, 1)].fg,
            accent,
            "the kept dialog area is untouched"
        );
        assert_eq!(
            mix(Color::Rgb(0, 0, 0), Color::Rgb(100, 200, 50), 0.5),
            Color::Rgb(50, 100, 25)
        );
    }
}
