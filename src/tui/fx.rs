//! Pure-Ratatui buffer-level motion and visual effect primitives.
//! Every time-dependent function accepts explicit instants/durations so rendering remains
//! 100% deterministic in tests and snapshots.

use crate::theme::{Color as ThemeColor, Palette, Role, Theme, UiStyle, derive};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use std::time::{Duration, Instant};

const BRAILLE_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_SPINNER: [&str; 4] = ["-", "\\", "|", "/"];
const SPINNER_STEP: Duration = Duration::from_millis(80);

/// Resource gauge bar glide duration.
pub(super) const GLIDE: Duration = Duration::from_millis(450);
/// Refresh sheen sweep across resource bars after telemetry updates.
pub(super) const GAUGE_SHEEN: Duration = Duration::from_millis(520);
/// Smooth palette RGB morph duration when switching/previewing themes.
pub(super) const PALETTE_MORPH: Duration = Duration::from_millis(180);
/// Right drawer / overlay sidebar reveal duration.
pub(super) const DRAWER_SLIDE: Duration = Duration::from_millis(140);
/// Modal reveal fade duration.
pub(super) const DIALOG_FADE: Duration = Duration::from_millis(120);
/// Workspace view cross-fade duration.
pub(super) const VIEW_FADE: Duration = Duration::from_millis(110);
/// Toast lifetime and fade-out window.
pub(super) const TOAST_LIFETIME: Duration = Duration::from_millis(2800);
const TOAST_FADE: Duration = Duration::from_millis(500);
/// Delay before showing a blocking loading box while fetching dynamic choices.
pub(super) const LOADING_DELAY: Duration = Duration::from_millis(150);

/// On-demand frame budgets to balance smoothness with low SSH bandwidth.
pub(super) const ANIMATION_FRAME: Duration = Duration::from_millis(33);
pub(super) const SPINNER_FRAME: Duration = SPINNER_STEP;
/// Gentle low-bandwidth cadence for restrained ambient focus/status breathing.
pub(super) const AMBIENT_FRAME: Duration = Duration::from_millis(110);
pub(super) const IDLE_FRAME: Duration = Duration::from_millis(250);

pub(super) fn spinner(elapsed: Duration, ascii: bool) -> &'static str {
    let step = (elapsed.as_millis() / SPINNER_STEP.as_millis()) as usize;
    if ascii {
        ASCII_SPINNER[step % ASCII_SPINNER.len()]
    } else {
        BRAILLE_SPINNER[step % BRAILLE_SPINNER.len()]
    }
}

pub(super) fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub(super) fn dialog_opacity(age: Duration, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    ease_out_cubic(age.as_secs_f64() / DIALOG_FADE.as_secs_f64())
}

pub(super) fn drawer_progress(age: Duration, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    ease_out_cubic(age.as_secs_f64() / DRAWER_SLIDE.as_secs_f64())
}

pub(super) fn view_opacity(age: Duration, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    ease_out_cubic(age.as_secs_f64() / VIEW_FADE.as_secs_f64())
}

pub(super) fn palette_morph_factor(age: Duration, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    ease_out_cubic(age.as_secs_f64() / PALETTE_MORPH.as_secs_f64())
}

pub(super) fn toast_opacity(age: Duration) -> f64 {
    if age >= TOAST_LIFETIME {
        return 0.0;
    }
    let remaining = TOAST_LIFETIME - age;
    if remaining < TOAST_FADE {
        remaining.as_secs_f64() / TOAST_FADE.as_secs_f64()
    } else {
        1.0
    }
}

pub(super) fn toast_fading(age: Duration) -> bool {
    age < TOAST_LIFETIME && TOAST_LIFETIME - age < TOAST_FADE
}

/// Whether a [`UiStyle`] enables gentle ambient breathing when reduced motion is off.
pub(super) const fn style_has_ambient_motion(style: UiStyle) -> bool {
    !matches!(style, UiStyle::Editorial | UiStyle::Heavy | UiStyle::Ascii)
}

/// Per-style ambient breathing period (in seconds) for distinct visual rhythm.
const fn style_pulse_period(style: UiStyle) -> f64 {
    match style {
        UiStyle::Hud => 1.8,
        UiStyle::Rounded | UiStyle::Atelier => 2.2,
        UiStyle::Bento | UiStyle::Studio => 2.6,
        UiStyle::Editorial | UiStyle::Heavy | UiStyle::Ascii => 2.4,
    }
}

/// Smooth 0.0..=1.0 sinusoidal breathing value for live indicators.
pub(super) fn pulse_factor(elapsed: Duration, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    let phase = elapsed.as_secs_f64() * std::f64::consts::TAU / 1.8;
    0.55 + 0.45 * ((phase.sin() + 1.0) * 0.5)
}

/// Restrained 0.0..=1.0 style-paced wave for focus border / accent rail glow.
pub(super) fn style_wave(style: UiStyle, elapsed: Duration, reduced: bool) -> f64 {
    if reduced || !style_has_ambient_motion(style) {
        return 0.0;
    }
    let period = style_pulse_period(style);
    let phase = elapsed.as_secs_f64() * std::f64::consts::TAU / period;
    (phase.sin() + 1.0) * 0.5
}

/// Moving highlight cell index for active progress bars.
pub(super) fn shimmer_index(elapsed: Duration, width: usize, reduced: bool) -> Option<usize> {
    if reduced || width < 4 {
        return None;
    }
    let step = (elapsed.as_millis() / 70) as usize;
    Some(step % width)
}

/// One-shot metallic sheen sweep across a gauge right after telemetry refreshes.
pub(super) fn refresh_sheen_index(age: Duration, width: usize, reduced: bool) -> Option<usize> {
    if reduced || width < 4 || age >= GAUGE_SHEEN {
        return None;
    }
    let progress = ease_out_cubic(age.as_secs_f64() / GAUGE_SHEEN.as_secs_f64());
    let idx = (progress * width as f64).floor() as usize;
    (idx < width).then_some(idx)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tween {
    from: f64,
    to: f64,
    started: Instant,
}

impl Tween {
    pub fn new(to: f64, now: Instant) -> Self {
        Self {
            from: 0.0,
            to,
            started: now,
        }
    }

    pub fn value(&self, now: Instant, reduced: bool) -> f64 {
        if reduced {
            return self.to;
        }
        let t = now.saturating_duration_since(self.started).as_secs_f64() / GLIDE.as_secs_f64();
        self.from + (self.to - self.from) * ease_out_cubic(t)
    }

    pub fn moving(&self, now: Instant, reduced: bool) -> bool {
        !reduced && now.saturating_duration_since(self.started) < GLIDE
    }

    pub fn retarget(&mut self, to: f64, now: Instant, reduced: bool) {
        if (to - self.to).abs() < 0.05 {
            return;
        }
        self.from = self.value(now, reduced);
        self.to = to;
        self.started = now;
    }
}

fn to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

pub(super) fn blend_ratatui(from: Color, to: Color, t: f64) -> Color {
    match (to_rgb(from), to_rgb(to)) {
        (Some(a), Some(b)) => {
            let (r, g, bl) = derive::mix(a, b, t);
            Color::Rgb(r, g, bl)
        }
        _ => to,
    }
}

fn lerp_theme_color(from: ThemeColor, to: ThemeColor, t: f64) -> ThemeColor {
    match (from.rgb(), to.rgb()) {
        (Some(a), Some(b)) => {
            let (r, g, bl) = derive::mix(a, b, t);
            ThemeColor::Rgb(r, g, bl)
        }
        _ => to,
    }
}

/// Smoothly interpolate every color role between two palettes (`t` in `0.0..=1.0`).
pub(super) fn lerp_palette(from: Palette, to: Palette, t: f64) -> Palette {
    if t >= 0.995 {
        return to;
    }
    if t <= 0.005 {
        return from;
    }
    let mut colors = [ThemeColor::Default; 20];
    for (idx, role) in Role::ALL.into_iter().enumerate() {
        colors[idx] = lerp_theme_color(from.get(role), to.get(role), t);
    }
    Palette::from_colors(colors)
}

/// Subtly animated accent color for focused borders and active accent rails.
pub(super) fn focus_glow(theme: &Theme, base: Color, elapsed: Duration, reduced: bool) -> Color {
    let wave = style_wave(theme.style, elapsed, reduced);
    if wave <= 0.001 {
        return base;
    }
    let target = match theme.palette.accent2.rgb() {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => return base,
    };
    let strength = match theme.style {
        UiStyle::Hud => 0.42,
        UiStyle::Rounded | UiStyle::Bento | UiStyle::Studio => 0.28,
        UiStyle::Atelier => 0.22,
        UiStyle::Editorial | UiStyle::Heavy | UiStyle::Ascii => 0.0,
    };
    blend_ratatui(base, target, wave * strength)
}

/// Smoothly compute the semantic color at normalized position `pos` (`0.0..=1.0`) along a meter:
/// healthy green (with a subtle theme-native accent tint at the left edge) -> amber warning ->
/// danger red.
fn meter_rgb_at(pos: f64, theme: &Theme) -> Option<(u8, u8, u8)> {
    let p = theme.palette;
    let healthy = p.success.rgb()?;
    let accent = p.accent.rgb().unwrap_or(healthy);
    let mid = p.warning.rgb()?;
    let high = p.danger.rgb()?;
    let start = derive::mix(healthy, accent, 0.22);
    let pos = pos.clamp(0.0, 1.0);
    let rgb = if pos <= 0.50 {
        derive::mix(start, healthy, pos / 0.50)
    } else if pos <= 0.76 {
        derive::mix(healthy, mid, (pos - 0.50) / 0.26)
    } else if pos <= 0.92 {
        derive::mix(mid, high, (pos - 0.76) / 0.16)
    } else {
        high
    };
    Some(rgb)
}

/// Semantic color for a percentage readout or compact telemetry pill (`0.0..=100.0`).
pub(super) fn meter_color(pct: f64, theme: &Theme) -> Color {
    if theme.flags.ascii {
        return Color::Reset;
    }
    let clamped = pct.clamp(0.0, 100.0);
    if let Some((r, g, b)) = meter_rgb_at(clamped / 100.0, theme) {
        return Color::Rgb(r, g, b);
    }
    if clamped >= 80.0 {
        Color::Red
    } else if clamped >= 55.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Per-cell horizontal gradient color along a progress bar (`cell_idx` in `0..width`), with
/// optional high-load (>=80%) gentle breathing pulse and refresh sheen highlight.
pub(super) fn bar_cell_color(
    cell_idx: usize,
    width: usize,
    pct: f64,
    theme: &Theme,
    elapsed: Duration,
    reduced: bool,
    shimmer: bool,
) -> Color {
    if theme.flags.ascii {
        return Color::Reset;
    }
    let pos = (cell_idx as f64 + 0.5) / width.max(1) as f64;
    let base = match meter_rgb_at(pos, theme) {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => meter_color(pos * 100.0, theme),
    };
    let mut col = if pct >= 80.0 && pos >= 0.45 && !reduced && style_has_ambient_motion(theme.style)
    {
        let wave = style_wave(theme.style, elapsed, false);
        let focus = ((pos - 0.45) / 0.55).clamp(0.0, 1.0);
        let danger = match theme.palette.danger.rgb() {
            Some((r, g, b)) => Color::Rgb(r, g, b),
            None => Color::Red,
        };
        blend_ratatui(base, danger, wave * focus * 0.45)
    } else {
        base
    };
    if shimmer && !reduced {
        let highlight = match theme.palette.text1.rgb() {
            Some((r, g, b)) => Color::Rgb(r, g, b),
            None => Color::White,
        };
        col = blend_ratatui(col, highlight, 0.42);
    }
    col
}

/// High-load (>=80%) gentle pulse for percentage readouts.
pub(super) fn meter_pct_color(pct: f64, theme: &Theme, elapsed: Duration, reduced: bool) -> Color {
    let base = meter_color(pct, theme);
    if pct >= 80.0 && !reduced && !theme.flags.ascii && style_has_ambient_motion(theme.style) {
        let wave = style_wave(theme.style, elapsed, false);
        let lift = match theme.palette.text1.rgb() {
            Some((r, g, b)) => Color::Rgb(r, g, b),
            None => return base,
        };
        blend_ratatui(base, lift, wave * 0.26)
    } else {
        base
    }
}

/// Softly darken/tint background buffer cells behind an active modal dialog.
pub(super) fn apply_scrim(buf: &mut Buffer, area: Rect, theme: &Theme) {
    if theme.flags.ascii {
        return;
    }
    let Some(shadow_rgb) = theme.palette.shadow.rgb() else {
        return;
    };
    let canvas_rgb = theme.palette.canvas.rgb().unwrap_or(shadow_rgb);
    let scrim_target = if theme.light {
        derive::mix(canvas_rgb, (0x0f, 0x17, 0x2a), 0.28)
    } else {
        derive::mix(shadow_rgb, (0, 0, 0), 0.35)
    };
    let dim_fg = theme.palette.text3.rgb().unwrap_or(scrim_target);
    let target_bg = Color::Rgb(scrim_target.0, scrim_target.1, scrim_target.2);
    let target_fg = Color::Rgb(dim_fg.0, dim_fg.1, dim_fg.2);

    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if to_rgb(cell.bg).is_some() {
                cell.set_bg(blend_ratatui(cell.bg, target_bg, 0.58));
            }
            if to_rgb(cell.fg).is_some() {
                cell.set_fg(blend_ratatui(cell.fg, target_fg, 0.65));
            } else {
                cell.set_fg(target_fg);
            }
        }
    }
}

/// Paint a crisp 1-cell drop shadow along the right and bottom edges of an elevated dialog.
pub(super) fn apply_shadow(buf: &mut Buffer, rect: Rect, area: Rect, theme: &Theme) {
    if theme.flags.ascii {
        return;
    }
    let ThemeColor::Rgb(r, g, b) = theme.palette.shadow else {
        return;
    };
    let shadow = Color::Rgb(r, g, b);
    for y in (rect.top() + 1)..=rect.bottom().min(area.bottom().saturating_sub(1)) {
        if rect.right() < area.right() {
            buf[(rect.right(), y)].set_symbol(" ").set_bg(shadow);
        }
    }
    if rect.bottom() < area.bottom() {
        for x in (rect.left() + 1)..rect.right().min(area.right()) {
            buf[(x, rect.bottom())].set_symbol(" ").set_bg(shadow);
        }
    }
}

/// Blend a region from `base_bg` toward its rendered colors using `opacity` in `0.0..=1.0`.
pub(super) fn apply_fade(buf: &mut Buffer, rect: Rect, base: ThemeColor, opacity: f64) {
    if opacity >= 0.995 {
        return;
    }
    let ThemeColor::Rgb(br, bg, bb) = base else {
        return;
    };
    let base_color = Color::Rgb(br, bg, bb);
    let area = rect.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if to_rgb(cell.fg).is_some() {
                cell.set_fg(blend_ratatui(base_color, cell.fg, opacity));
            }
            if to_rgb(cell.bg).is_some() {
                cell.set_bg(blend_ratatui(base_color, cell.bg, opacity));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Background, StyleSelection, builtin, resolve};

    #[test]
    fn tween_glides_and_respects_reduced_motion() {
        let start = Instant::now();
        let mut tween = Tween::new(80.0, start);
        assert_eq!(tween.value(start, false), 0.0);
        assert_eq!(tween.value(start, true), 80.0);
        assert!(!tween.moving(start, true));
        assert!(tween.moving(start, false));
        let mid = tween.value(start + GLIDE / 2, false);
        assert!(mid > 40.0 && mid < 80.0);
        assert!((tween.value(start + GLIDE, false) - 80.0).abs() < 1e-9);
        tween.retarget(25.0, start + GLIDE, false);
        assert!((tween.value(start + GLIDE, false) - 80.0).abs() < 1e-9);
    }

    #[test]
    fn spinner_switches_between_braille_and_ascii() {
        assert_eq!(spinner(Duration::ZERO, false), "⠋");
        assert_eq!(spinner(Duration::ZERO, true), "-");
        assert_eq!(spinner(SPINNER_STEP, true), "\\");
    }

    #[test]
    fn palette_morph_interpolates_rgb_cleanly() {
        let mid = lerp_palette(builtin::OBSIDIAN, builtin::TOKYO_NIGHT, 0.5);
        assert_ne!(mid.canvas, builtin::OBSIDIAN.canvas);
        assert_ne!(mid.canvas, builtin::TOKYO_NIGHT.canvas);
        assert_eq!(
            lerp_palette(builtin::OBSIDIAN, builtin::TOKYO_NIGHT, 0.0),
            builtin::OBSIDIAN
        );
        assert_eq!(
            lerp_palette(builtin::OBSIDIAN, builtin::TOKYO_NIGHT, 1.0),
            builtin::TOKYO_NIGHT
        );
    }

    #[test]
    fn bar_gradient_progresses_from_healthy_to_danger() {
        let theme = resolve(
            &builtin::obsidian(),
            StyleSelection::Auto,
            &Background::Disabled,
            false,
        );
        let low = bar_cell_color(0, 20, 90.0, &theme, Duration::ZERO, true, false);
        let mid = bar_cell_color(14, 20, 90.0, &theme, Duration::ZERO, true, false);
        let high = bar_cell_color(19, 20, 90.0, &theme, Duration::ZERO, true, false);
        assert_ne!(low, mid);
        assert_ne!(mid, high);
    }
}
