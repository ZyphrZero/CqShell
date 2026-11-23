//! Style tokens, universal single-width-safe Unicode/ASCII glyphs, 8-style surface chrome,
//! and high-visibility resource gauge primitives.

use super::super::{
    fx,
    state::{Phase, Tone},
};
use crate::theme::{Ansi16, Color as ThemeColor, Level, Role, Theme, UiStyle, derive};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders},
};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Glyph {
    Brand,
    Overview,
    Optimize,
    ActiveBar,
    DotOn,
    DotOff,
    Success,
    Failure,
    Warning,
    Cancelled,
    Pending,
    Skipped,
    Chevron,
    Separator,
    External,
    Enter,
    Execute,
    Rule,
    Menu,
}

pub(in crate::tui) fn glyph(theme: &Theme, kind: Glyph) -> &'static str {
    if theme.flags.ascii {
        return match kind {
            Glyph::Brand => "*",
            Glyph::Overview => "@",
            Glyph::Optimize => "+",
            Glyph::ActiveBar => ">",
            Glyph::DotOn => "*",
            Glyph::DotOff => "o",
            Glyph::Success => "+",
            Glyph::Failure => "x",
            Glyph::Warning => "!",
            Glyph::Cancelled => "=",
            Glyph::Pending => ".",
            Glyph::Skipped => "-",
            Glyph::Chevron => ">",
            Glyph::Separator => "|",
            Glyph::External => "^",
            Glyph::Enter => "Enter",
            Glyph::Execute => ">",
            Glyph::Rule => "-",
            Glyph::Menu => "=",
        };
    }
    match theme.style {
        UiStyle::Hud => match kind {
            Glyph::Brand => "◈",
            Glyph::Overview => "◇",
            Glyph::Optimize => "✦",
            Glyph::ActiveBar => "┃",
            Glyph::DotOn => "◆",
            Glyph::DotOff => "◇",
            Glyph::Success => "✓",
            Glyph::Failure => "✗",
            Glyph::Warning => "!",
            Glyph::Cancelled => "■",
            Glyph::Pending => "◇",
            Glyph::Skipped => "–",
            Glyph::Chevron => "»",
            Glyph::Separator => "·",
            Glyph::External => "↗",
            Glyph::Enter => "↵",
            Glyph::Execute => "▸",
            Glyph::Rule => "─",
            Glyph::Menu => "≡",
        },
        UiStyle::Studio => match kind {
            Glyph::Brand => "◆",
            Glyph::Overview => "◈",
            Glyph::Optimize => "✦",
            Glyph::ActiveBar => "▸",
            Glyph::DotOn => "●",
            Glyph::DotOff => "○",
            Glyph::Success => "✓",
            Glyph::Failure => "✗",
            Glyph::Warning => "!",
            Glyph::Cancelled => "■",
            Glyph::Pending => "○",
            Glyph::Skipped => "–",
            Glyph::Chevron => "›",
            Glyph::Separator => "·",
            Glyph::External => "↗",
            Glyph::Enter => "↵",
            Glyph::Execute => "▸",
            Glyph::Rule => "─",
            Glyph::Menu => "≡",
        },
        UiStyle::Atelier => match kind {
            Glyph::Brand => "◆",
            Glyph::Overview => "◈",
            Glyph::Optimize => "✦",
            Glyph::ActiveBar => "│",
            Glyph::DotOn => "■",
            Glyph::DotOff => "□",
            Glyph::Success => "✓",
            Glyph::Failure => "✗",
            Glyph::Warning => "!",
            Glyph::Cancelled => "■",
            Glyph::Pending => "□",
            Glyph::Skipped => "–",
            Glyph::Chevron => "›",
            Glyph::Separator => "·",
            Glyph::External => "↗",
            Glyph::Enter => "↵",
            Glyph::Execute => "▸",
            Glyph::Rule => "┄",
            Glyph::Menu => "≡",
        },
        UiStyle::Heavy => match kind {
            Glyph::Brand => "◆",
            Glyph::Overview => "◈",
            Glyph::Optimize => "✦",
            Glyph::ActiveBar => "█",
            Glyph::DotOn => "●",
            Glyph::DotOff => "○",
            Glyph::Success => "✓",
            Glyph::Failure => "✗",
            Glyph::Warning => "!",
            Glyph::Cancelled => "■",
            Glyph::Pending => "○",
            Glyph::Skipped => "–",
            Glyph::Chevron => "»",
            Glyph::Separator => "·",
            Glyph::External => "↗",
            Glyph::Enter => "↵",
            Glyph::Execute => "▸",
            Glyph::Rule => "━",
            Glyph::Menu => "≡",
        },
        UiStyle::Rounded | UiStyle::Bento | UiStyle::Editorial | UiStyle::Ascii => match kind {
            Glyph::Brand => "◆",
            Glyph::Overview => "◈",
            Glyph::Optimize => "✦",
            Glyph::ActiveBar => "▎",
            Glyph::DotOn => "●",
            Glyph::DotOff => "○",
            Glyph::Success => "✓",
            Glyph::Failure => "✗",
            Glyph::Warning => "!",
            Glyph::Cancelled => "■",
            Glyph::Pending => "○",
            Glyph::Skipped => "–",
            Glyph::Chevron => "›",
            Glyph::Separator => "·",
            Glyph::External => "↗",
            Glyph::Enter => "↵",
            Glyph::Execute => "▸",
            Glyph::Rule => "─",
            Glyph::Menu => "≡",
        },
    }
}

pub(super) fn to_color(color: ThemeColor) -> Color {
    match color {
        ThemeColor::Default => Color::Reset,
        ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ThemeColor::Ansi(ansi) => match ansi {
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
    }
}

pub(in crate::tui) fn color(theme: &Theme, role: Role) -> Color {
    to_color(theme.palette.get(role))
}

pub(in crate::tui) fn fg(theme: &Theme, role: Role) -> Style {
    Style::default().fg(color(theme, role))
}

pub(in crate::tui) fn text(theme: &Theme, level: Level) -> Style {
    let style = fg(theme, level.role());
    if theme.flags.ascii && level == Level::Tertiary {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

pub(in crate::tui) fn tone_role(tone: Tone) -> Role {
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

pub(in crate::tui) fn tone_style(theme: &Theme, tone: Tone) -> Style {
    match tone {
        Tone::Plain => text(theme, Level::Primary),
        Tone::Muted => text(theme, Level::Secondary),
        other => fg(theme, tone_role(other)),
    }
}

pub(in crate::tui) fn selected_fg(theme: &Theme) -> Color {
    if theme.flags.solid_selection {
        color(theme, Role::OnAccent)
    } else {
        color(theme, Role::Text1)
    }
}

pub(in crate::tui) fn selected_accent(theme: &Theme) -> Color {
    if theme.flags.solid_selection {
        color(theme, Role::OnAccent)
    } else {
        color(theme, Role::Accent)
    }
}

/// Active accent color with restrained ambient style breathing when motion is enabled.
pub(in crate::tui) fn animated_selected_accent(
    theme: &Theme,
    elapsed: Duration,
    reduced: bool,
) -> Color {
    let base = selected_accent(theme);
    if theme.flags.solid_selection || theme.flags.ascii {
        base
    } else {
        fx::focus_glow(theme, base, elapsed, reduced)
    }
}

/// Background color used for a card's header strip (`SurfaceAlt` for unframed `Studio`).
pub(in crate::tui) fn card_title_bg(theme: &Theme, bg_role: Role) -> Color {
    if theme.style == UiStyle::Studio && !theme.flags.framed && theme.flags.paint_bg {
        if bg_role == Role::Selection {
            color(theme, Role::Selection)
        } else {
            color(theme, Role::SurfaceAlt)
        }
    } else if theme.flags.paint_bg || matches!(bg_role, Role::Elevated | Role::Selection) {
        color(theme, bg_role)
    } else {
        Color::Reset
    }
}

/// Rows reserved at the top of a card before its body starts.
pub(in crate::tui) fn card_top_inset(theme: &Theme, card_height: u16) -> u16 {
    if theme.style == UiStyle::Editorial && !theme.flags.framed && card_height >= 5 {
        2
    } else {
        1
    }
}

/// Rows reserved at the bottom of a card after its body ends.
pub(in crate::tui) fn card_bottom_inset(theme: &Theme, card_height: u16) -> u16 {
    if theme.flags.framed
        || (matches!(theme.style, UiStyle::Bento | UiStyle::Studio) && card_height >= 9)
    {
        1
    } else {
        0
    }
}

/// Format a card header title according to the active [`UiStyle`].
pub(in crate::tui) fn format_card_title(theme: &Theme, title: &str, framed: bool) -> String {
    if theme.flags.ascii {
        return format!("[{title}]");
    }
    match theme.style {
        UiStyle::Atelier => format!(" [ {title} ] "),
        UiStyle::Hud => format!(" {} {title} ", glyph(theme, Glyph::Brand)),
        UiStyle::Bento if !framed => format!("{} {title}", glyph(theme, Glyph::ActiveBar)),
        UiStyle::Studio | UiStyle::Editorial if !framed => title.to_owned(),
        _ if framed => format!(" {title} "),
        _ => title.to_owned(),
    }
}

/// Title foreground color tailored to the active [`UiStyle`].
pub(in crate::tui) fn card_title_fg(theme: &Theme, emphasized: bool) -> Color {
    if emphasized {
        return color(theme, Role::Accent);
    }
    match theme.style {
        UiStyle::Hud | UiStyle::Atelier | UiStyle::Studio => color(theme, Role::Accent),
        _ => color(theme, Role::Text1),
    }
}

pub(super) fn phase_icon(theme: &Theme, phase: Phase, elapsed: Duration) -> &'static str {
    match phase {
        Phase::Running | Phase::Cancelling => fx::spinner(elapsed, theme.flags.ascii),
        Phase::Completed => glyph(theme, Glyph::Success),
        Phase::Failed => glyph(theme, Glyph::Failure),
        Phase::Cancelled => glyph(theme, Glyph::Cancelled),
        Phase::Idle => glyph(theme, Glyph::DotOff),
    }
}

/// Fill every cell inside `rect` with `bg` and clear text.
pub(super) fn fill_surface(buf: &mut Buffer, rect: Rect, theme: &Theme, bg_role: Role) {
    let area = rect.intersection(buf.area);
    let bg = if theme.flags.paint_bg || matches!(bg_role, Role::Elevated | Role::Selection) {
        color(theme, bg_role)
    } else {
        Color::Reset
    };
    let fg_col = color(theme, Role::Text1);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(Style::default().fg(fg_col).bg(bg));
        }
    }
}

const ASCII_BORDER: ratatui::symbols::border::Set<'static> = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Draw a style-aware surface card without ambient border motion (deterministic/static callers).
pub(in crate::tui) fn draw_surface_card(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    bg_role: Role,
    border_role: Role,
    force_border: bool,
) {
    draw_surface_card_fx(
        frame,
        area,
        theme,
        bg_role,
        border_role,
        force_border,
        Duration::ZERO,
        true,
    );
}

/// Draw a style-aware surface card across all 8 built-in [`UiStyle`] design languages,
/// with optional restrained ambient focus glow.
#[allow(clippy::too_many_arguments)]
pub(in crate::tui) fn draw_surface_card_fx(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    bg_role: Role,
    border_role: Role,
    force_border: bool,
    elapsed: Duration,
    reduced: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    fill_surface(frame.buffer_mut(), area, theme, bg_role);

    let bg_col = if theme.flags.paint_bg
        || force_border
        || matches!(bg_role, Role::Elevated | Role::Selection)
    {
        color(theme, bg_role)
    } else {
        Color::Reset
    };
    let focused = border_role == Role::BorderFocus;
    let raw_border_col = color(theme, border_role);
    let border_col = if focused && !theme.flags.ascii {
        fx::focus_glow(theme, raw_border_col, elapsed, reduced)
    } else {
        raw_border_col
    };

    let draw_box = theme.flags.framed || force_border;
    if !draw_box {
        let buf = frame.buffer_mut();
        match theme.style {
            UiStyle::Studio => {
                // Full-width frosted SurfaceAlt header bar along row 0
                let hdr_bg = card_title_bg(theme, bg_role);
                for x in area.left()..area.right() {
                    buf[(x, area.y)].set_symbol(" ").set_bg(hdr_bg);
                }
                if focused && area.height >= 2 {
                    let rail_fg = fx::focus_glow(theme, selected_accent(theme), elapsed, reduced);
                    for y in area.top()..area.bottom() {
                        let y_bg = if y == area.y { hdr_bg } else { bg_col };
                        buf[(area.x, y)]
                            .set_symbol("▎")
                            .set_style(Style::default().fg(rail_fg).bg(y_bg));
                    }
                }
            }
            UiStyle::Editorial => {
                // Clean Swiss editorial top rule (no side or bottom walls)
                let rule_y = if area.height >= 5 { area.y + 1 } else { area.y };
                if rule_y < area.bottom() {
                    let rule_fg = if focused {
                        color(theme, Role::Accent)
                    } else {
                        color(theme, Role::Border)
                    };
                    let sym = if focused { "━" } else { "─" };
                    let x0 = area.x + u16::from(area.width >= 6);
                    let x1 = area.right().saturating_sub(u16::from(area.width >= 6));
                    for x in x0..x1 {
                        buf[(x, rule_y)]
                            .set_symbol(sym)
                            .set_style(Style::default().fg(rule_fg).bg(bg_col));
                    }
                }
            }
            UiStyle::Bento if focused => {
                // Borderless layered tonal surface; subtle left accent rail when focused
                let rail_fg = fx::focus_glow(theme, selected_accent(theme), elapsed, reduced);
                for y in area.top()..area.bottom() {
                    buf[(area.x, y)]
                        .set_symbol("▎")
                        .set_style(Style::default().fg(rail_fg).bg(bg_col));
                }
            }
            _ => {}
        }
        return;
    }

    // Framed box styles (and forced modal / transparent-bg borders)
    if theme.style == UiStyle::Hud && !theme.flags.ascii {
        let edge_col = if focused {
            color(theme, Role::Border)
        } else {
            color(theme, Role::Rule)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(ratatui::symbols::border::PLAIN)
            .border_style(Style::default().fg(edge_col).bg(bg_col));
        frame.render_widget(block, area);

        // Stamp crisp glowing heavy HUD corners (`┏━ ━┓ / ┗━ ━┛`)
        let corner_base = if focused {
            color(theme, Role::BorderFocus)
        } else {
            color(theme, Role::Accent2)
        };
        let corner_col = fx::focus_glow(theme, corner_base, elapsed, reduced);
        let corner_style = Style::default()
            .fg(corner_col)
            .bg(bg_col)
            .add_modifier(Modifier::BOLD);
        let buf = frame.buffer_mut();
        let l = area.left();
        let r = area.right().saturating_sub(1);
        let t = area.top();
        let b = area.bottom().saturating_sub(1);
        buf[(l, t)].set_symbol("┏").set_style(corner_style);
        buf[(r, t)].set_symbol("┓").set_style(corner_style);
        buf[(l, b)].set_symbol("┗").set_style(corner_style);
        buf[(r, b)].set_symbol("┛").set_style(corner_style);
        if area.width >= 8 {
            buf[(l + 1, t)].set_symbol("━").set_style(corner_style);
            buf[(r - 1, t)].set_symbol("━").set_style(corner_style);
            buf[(l + 1, b)].set_symbol("━").set_style(corner_style);
            buf[(r - 1, b)].set_symbol("━").set_style(corner_style);
        }
        return;
    }

    let border_set = if theme.flags.ascii {
        ASCII_BORDER
    } else {
        match theme.style {
            UiStyle::Atelier | UiStyle::Editorial => ratatui::symbols::border::PLAIN,
            UiStyle::Heavy => ratatui::symbols::border::THICK,
            UiStyle::Rounded | UiStyle::Bento | UiStyle::Studio | UiStyle::Hud | UiStyle::Ascii => {
                ratatui::symbols::border::ROUNDED
            }
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border_set)
        .border_style(Style::default().fg(border_col).bg(bg_col));
    frame.render_widget(block, area);
}

/// Draw a clean horizontal divider tailored to the active [`UiStyle`].
pub(in crate::tui) fn draw_rule(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    bg: Color,
) {
    if width == 0 || y >= buf.area.bottom() {
        return;
    }
    let sym = glyph(theme, Glyph::Rule);
    let style = fg(theme, Role::Rule).bg(bg);
    let end = (x + width).min(buf.area.right());
    for col in x..end {
        buf[(col, y)].set_symbol(sym).set_style(style);
    }
}

/// Keycap spans for footer and dialog hints, styled per [`UiStyle`].
pub(super) fn keycap(theme: &Theme, key: &str, label: &str) -> Vec<Span<'static>> {
    if theme.flags.ascii {
        let ascii_key = key
            .replace("↑↓←→", "Arrows")
            .replace("↑↓", "Up/Dn")
            .replace("←→", "Lt/Rt")
            .replace('↵', "Enter");
        return vec![
            Span::styled(
                format!("[{ascii_key}]"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), text(theme, Level::Secondary)),
        ];
    }
    match theme.style {
        UiStyle::Heavy => vec![
            Span::styled(
                format!("[{key}]"),
                Style::default()
                    .fg(color(theme, Role::Accent))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), text(theme, Level::Secondary)),
        ],
        UiStyle::Editorial => vec![
            Span::styled(
                key.to_owned(),
                Style::default()
                    .fg(color(theme, Role::Accent))
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            Span::styled(format!(" {label}  "), text(theme, Level::Secondary)),
        ],
        UiStyle::Atelier | UiStyle::Hud => vec![
            Span::styled(
                format!("[{key}]"),
                Style::default()
                    .fg(color(theme, Role::Accent))
                    .bg(if theme.flags.paint_bg {
                        color(theme, Role::SurfaceAlt)
                    } else {
                        Color::Reset
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), text(theme, Level::Secondary)),
        ],
        _ => vec![
            Span::styled(
                format!(" {key} "),
                Style::default()
                    .fg(color(theme, Role::Accent))
                    .bg(if theme.flags.paint_bg {
                        color(theme, Role::SurfaceAlt)
                    } else {
                        Color::Reset
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), text(theme, Level::Secondary)),
        ],
    }
}

/// Build whole `(key, label)` keycap spans that fit within `max_w` cells without clipping mid-label.
pub(super) fn fit_keycaps(
    theme: &Theme,
    pairs: &[(&str, &str)],
    max_w: usize,
) -> Vec<Span<'static>> {
    let mut kept = Vec::new();
    let mut used = 0usize;
    for &(k, lbl) in pairs {
        let group = keycap(theme, k, lbl);
        let gw: usize = group.iter().map(Span::width).sum();
        if used > 0 && used + gw > max_w {
            break;
        }
        if used == 0 && gw > max_w {
            break;
        }
        used += gw;
        kept.extend(group);
    }
    kept
}

/// Subtle pill/bracket badge span tailored to [`UiStyle`].
pub(super) fn badge_span(theme: &Theme, label: impl Into<String>, tone: Tone) -> Span<'static> {
    let label = label.into();
    if theme.flags.ascii || matches!(theme.style, UiStyle::Heavy | UiStyle::Atelier) {
        return Span::styled(
            format!("[{label}]"),
            tone_style(theme, tone).add_modifier(Modifier::BOLD),
        );
    }
    if theme.style == UiStyle::Editorial {
        return Span::styled(
            format!("· {label}"),
            tone_style(theme, tone).add_modifier(Modifier::BOLD),
        );
    }
    Span::styled(
        format!(" {label} "),
        tone_style(theme, tone)
            .bg(if theme.flags.paint_bg {
                color(theme, Role::SurfaceAlt)
            } else {
                Color::Reset
            })
            .add_modifier(Modifier::BOLD),
    )
}

/// Visible recessed track color for unfilled progress bar cells against Surface/Canvas.
fn track_empty_fg(theme: &Theme) -> Color {
    if theme.flags.ascii || theme.style == UiStyle::Heavy {
        return color(theme, Role::Rule);
    }
    match (theme.palette.border.rgb(), theme.palette.text3.rgb()) {
        (Some(b), Some(t)) => {
            let ratio = if theme.light { 0.08 } else { 0.18 };
            let (r, g, bl) = derive::mix(b, t, ratio);
            Color::Rgb(r, g, bl)
        }
        _ => color(theme, Role::Rule),
    }
}

/// High-visibility, style-adapted horizontal gauge bar with smooth semantic gradient,
/// uniform bar height across filled & track cells, crisp built-in vertical separation
/// (never fuses across stacked rows in zero-leading terminals), optional high-load (>=80%)
/// pulse, and refresh sheen sweep.
///
/// - Smooth styles (`Rounded`, `Bento`, `Editorial`, `Studio`): uniform 50%-height ribbon (`▄`)
///   for both fill and recessed track with transparent background (50% vertical gap).
/// - Geometric styles (`Hud`, `Atelier`): crisp discrete segmented blocks (`▪`/`▫`) with
///   transparent background.
/// - High-contrast (`Heavy`): uniform 62.5%-height bar (`▅`) for both fill and mid-gray track
///   (37.5% vertical gap).
/// - Pure ASCII (`Ascii`): bracketed `[####----]`.
pub(in crate::tui) fn gauge_spans_fx(
    pct: f64,
    width: usize,
    theme: &Theme,
    elapsed: Duration,
    reduced: bool,
    shimmer_col: Option<usize>,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let clamped = pct.clamp(0.0, 100.0);

    if theme.flags.ascii {
        if width >= 4 {
            let inner = width - 2;
            let filled = ((clamped / 100.0) * inner as f64).round() as usize;
            let filled = filled.min(inner);
            let empty = inner.saturating_sub(filled);
            return vec![
                Span::styled("[", text(theme, Level::Secondary)),
                Span::styled(
                    "#".repeat(filled),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("-".repeat(empty), text(theme, Level::Tertiary)),
                Span::styled("]", text(theme, Level::Secondary)),
            ];
        }
        let filled = ((clamped / 100.0) * width as f64).round() as usize;
        let filled = filled.min(width);
        return vec![
            Span::styled(
                "#".repeat(filled),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("-".repeat(width - filled), text(theme, Level::Tertiary)),
        ];
    }

    let empty_fg = track_empty_fg(theme);
    let empty_style = Style::default().fg(empty_fg);
    let mut filled = ((clamped / 100.0) * width as f64).round() as usize;
    if clamped > 0.5 && filled == 0 {
        filled = 1;
    }
    filled = filled.min(width);
    let empty = width.saturating_sub(filled);

    let (fill_glyph, empty_glyph, animate) = if theme.style.uses_segmented_gauge() {
        ("▪", "▫", true)
    } else if theme.style == UiStyle::Heavy {
        ("▅", "▅", false)
    } else {
        ("▄", "▄", true)
    };

    let mut spans = Vec::with_capacity(filled + usize::from(empty > 0));
    for i in 0..filled {
        let fg_col = fx::bar_cell_color(
            i,
            width,
            clamped,
            theme,
            elapsed,
            !animate || reduced,
            animate && shimmer_col == Some(i),
        );
        spans.push(Span::styled(fill_glyph, Style::default().fg(fg_col)));
    }
    if empty > 0 {
        spans.push(Span::styled(empty_glyph.repeat(empty), empty_style));
    }
    spans
}

/// Static wrapper around [`gauge_spans_fx`] for unit tests and non-animated contexts.
#[cfg(test)]
pub(in crate::tui) fn gauge_spans(pct: f64, width: usize, theme: &Theme) -> Vec<Span<'static>> {
    gauge_spans_fx(pct, width, theme, Duration::ZERO, true, None)
}

/// Sparkline over recent CPU percentages.
pub(in crate::tui) fn sparkline(samples: &[f32], width: usize, theme: &Theme) -> Span<'static> {
    if width == 0 {
        return Span::raw("");
    }
    let levels: &[char] = if theme.flags.ascii {
        &['.', ':', '-', '=', '+', '*', '#', '@']
    } else {
        &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']
    };
    let tail = if samples.len() > width {
        &samples[samples.len() - width..]
    } else {
        samples
    };
    let pad_left = width.saturating_sub(tail.len());
    let mut out = String::with_capacity(width * 3);
    for _ in 0..pad_left {
        out.push(levels[0]);
    }
    for &sample in tail {
        let idx = ((sample.clamp(0.0, 100.0) / 100.0) * (levels.len() - 1) as f32).round() as usize;
        out.push(levels[idx.min(levels.len() - 1)]);
    }
    let latest = tail.last().copied().unwrap_or(0.0);
    Span::styled(
        out,
        Style::default().fg(fx::meter_color(f64::from(latest), theme)),
    )
}

/// Render a right-aligned badge/text at the end of a 1-row area without wrapping.
pub(in crate::tui) fn put_right(buf: &mut Buffer, row: Rect, text: &str, style: Style) {
    let w = text.width() as u16;
    if row.width == 0 || w == 0 || row.y >= buf.area.bottom() {
        return;
    }
    let x = row.right().saturating_sub(w).max(row.x);
    buf.set_stringn(
        x,
        row.y,
        text,
        row.right().saturating_sub(x) as usize,
        style,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Background, StyleSelection, builtin, resolve};

    #[test]
    fn every_style_glyph_and_gauge_symbol_is_single_cell_safe() {
        let glyphs = [
            Glyph::Brand,
            Glyph::Overview,
            Glyph::Optimize,
            Glyph::ActiveBar,
            Glyph::DotOn,
            Glyph::DotOff,
            Glyph::Success,
            Glyph::Failure,
            Glyph::Warning,
            Glyph::Cancelled,
            Glyph::Pending,
            Glyph::Skipped,
            Glyph::Chevron,
            Glyph::Separator,
            Glyph::External,
            Glyph::Execute,
            Glyph::Rule,
            Glyph::Menu,
        ];
        for style in UiStyle::ALL {
            let theme = resolve(
                &builtin::obsidian(),
                StyleSelection::Fixed(style),
                &Background::Disabled,
                false,
            );
            for kind in glyphs {
                let g = glyph(&theme, kind);
                assert_eq!(
                    g.width(),
                    1,
                    "style {style:?} glyph {kind:?} ({g:?}) width != 1"
                );
            }
            for pct in [0.0, 1.0, 12.0, 41.0, 63.0, 88.0, 100.0] {
                let spans = gauge_spans(pct, 16, &theme);
                let total_w: usize = spans.iter().map(Span::width).sum();
                assert_eq!(
                    total_w, 16,
                    "style {style:?} gauge width mismatch at {pct}%"
                );
            }
        }
    }
}
