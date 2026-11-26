//! High-resolution real-time traffic waveform & distribution atlas renderer for Mihomo.
//!
//! Supports:
//! - Unicode Braille (`2×4` sub-pixel dots per cell) with smooth RGB vertical gradient shading
//! - Pure 7-bit ASCII fallback (`plain` theme / serial consoles) with zero non-ASCII bytes
//! - Three visualization modes: `Mirror` (download up / upload down), `Overlay`, and `Stacked`
//! - Linear & Logarithmic Y-axis scaling with human-rounded ceiling ticks
//! - Rolling `60s` / `3m` / `5m` time windows with right-aligned live arrival
//! - Live traffic distribution breakdown by Exit Node, Routing Rule, Process, and Top Flows

use super::{draw_card_title, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    ops::clash::model::{
        ClashSnapshot, TrafficBucket, TrafficSample, format_bytes, format_bytes_compact,
        format_speed, format_speed_axis, format_speed_compact, nice_speed_ceiling,
    },
    theme::{Level, Role, Theme, derive},
    tui::{
        clash::{
            layout::{OverviewGeom, TrafficFullscreenGeom},
            state::{TrafficBreakdownKind, TrafficGraphMode, TrafficScale, TrafficWindow},
        },
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthStr;

/// Unicode Braille dot bitmasks indexed by `[dx in 0..2][dy in 0..4]` (top-to-bottom).
const BRAILLE_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Map `history` onto `cols` horizontal sub-pixel columns (right-aligned so `cols - 1` is `now`).
///
/// Upsamples smoothly via linear interpolation when `filled >= avail`, and downsamples using
/// peak-preserving bucket maximums when `filled < avail` so short bursts remain visible.
pub(in crate::tui) fn resample_window(
    history: &VecDeque<TrafficSample>,
    window_secs: usize,
    cols: usize,
) -> (Vec<Option<u64>>, Vec<Option<u64>>) {
    let mut down = vec![None; cols];
    let mut up = vec![None; cols];
    if cols == 0 || history.is_empty() {
        return (down, up);
    }
    let span = window_secs.max(1);
    let avail = history.len().min(span);
    let skip = history.len() - avail;
    let filled = ((avail as f64 / span as f64) * cols as f64)
        .round()
        .clamp(1.0, cols as f64) as usize;
    let start_col = cols - filled;

    if avail == 1 {
        let s = history[skip];
        for col in start_col..cols {
            down[col] = Some(s.down_bps);
            up[col] = Some(s.up_bps);
        }
        return (down, up);
    }

    if filled >= avail {
        let denom = (filled - 1).max(1) as f64;
        for i in 0..filled {
            let pos = (i as f64) * (avail - 1) as f64 / denom;
            let lo = (pos.floor() as usize).min(avail - 1);
            let hi = (lo + 1).min(avail - 1);
            let frac = pos - lo as f64;
            let s0 = history[skip + lo];
            let s1 = history[skip + hi];
            let d = (s0.down_bps as f64 + (s1.down_bps as f64 - s0.down_bps as f64) * frac)
                .round()
                .max(0.0) as u64;
            let u = (s0.up_bps as f64 + (s1.up_bps as f64 - s0.up_bps as f64) * frac)
                .round()
                .max(0.0) as u64;
            down[start_col + i] = Some(d);
            up[start_col + i] = Some(u);
        }
    } else {
        for i in 0..filled {
            let b0 = i * avail / filled;
            let b1 = ((i + 1) * avail / filled).max(b0 + 1).min(avail);
            let mut max_d = 0u64;
            let mut max_u = 0u64;
            for idx in b0..b1 {
                let s = history[skip + idx];
                max_d = max_d.max(s.down_bps);
                max_u = max_u.max(s.up_bps);
            }
            down[start_col + i] = Some(max_d);
            up[start_col + i] = Some(max_u);
        }
    }

    (down, up)
}

/// Normalize a byte/sec throughput reading onto `0.0..=1.0` against `ceiling` using `scale`.
pub(in crate::tui) fn normalize_bps(bps: u64, ceiling: u64, scale: TrafficScale) -> f64 {
    let ceil = ceiling.max(1);
    if bps == 0 {
        return 0.0;
    }
    match scale {
        TrafficScale::Linear => (bps as f64 / ceil as f64).clamp(0.0, 1.0),
        TrafficScale::Log => {
            const FLOOR: f64 = 512.0;
            let num = (1.0 + bps as f64 / FLOOR).ln();
            let den = (1.0 + ceil as f64 / FLOOR).ln().max(1e-6);
            (num / den).clamp(0.0, 1.0)
        }
    }
}

/// Convert normalized `0.0..=1.0` height into discrete sub-pixel height (`0..=max_px`),
/// keeping any positive non-zero rate visible as at least 1 sub-pixel.
fn scaled_height(bps: u64, ceiling: u64, scale: TrafficScale, max_px: usize) -> usize {
    if bps == 0 || max_px == 0 {
        return 0;
    }
    let ratio = normalize_bps(bps, ceiling, scale);
    let px = (ratio * max_px as f64).round() as usize;
    px.clamp(1, max_px)
}

fn shaded_role_color(theme: &Theme, role: Role, intensity: f64) -> Color {
    if theme.flags.ascii {
        return Color::Reset;
    }
    let base = theme.palette.get(role);
    let Some(base_rgb) = base.rgb() else {
        return tokens::color(theme, role);
    };
    let t = intensity.clamp(0.0, 1.0);
    let rgb = if t < 0.72 {
        let bg_rgb = theme
            .palette
            .surface_alt
            .rgb()
            .or_else(|| theme.palette.surface.rgb())
            .unwrap_or((18, 22, 30));
        let factor = 0.56 + (t / 0.72) * 0.44;
        derive::mix(bg_rgb, base_rgb, factor)
    } else {
        let hi_rgb = theme.palette.text1.rgb().unwrap_or((245, 248, 255));
        let factor = ((t - 0.72) / 0.28) * 0.22;
        derive::mix(base_rgb, hi_rgb, factor)
    };
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

fn overlap_color(theme: &Theme) -> Color {
    if theme.flags.ascii {
        return Color::Reset;
    }
    if let (Some(a), Some(b)) = (theme.palette.accent.rgb(), theme.palette.success.rgb()) {
        let (r, g, bl) = derive::mix(a, b, 0.5);
        return Color::Rgb(r, g, bl);
    }
    tokens::color(theme, Role::Warning)
}

fn grid_color(theme: &Theme) -> Color {
    if theme.flags.ascii {
        return tokens::color(theme, Role::Rule);
    }
    if let (Some(bg), Some(rule)) = (theme.palette.surface_alt.rgb(), theme.palette.rule.rgb()) {
        let (r, g, b) = derive::mix(bg, rule, 0.55);
        return Color::Rgb(r, g, b);
    }
    tokens::color(theme, Role::Rule)
}

struct WaveformSpec {
    mode: TrafficGraphMode,
    window: TrafficWindow,
    scale: TrafficScale,
    show_x_axis: bool,
}

fn draw_waveform_plot(
    buf: &mut Buffer,
    area: Rect,
    snap: &ClashSnapshot,
    spec: WaveformSpec,
    theme: &Theme,
    bg: Color,
) {
    let WaveformSpec {
        mode,
        window,
        scale,
        show_x_axis,
    } = spec;
    if area.width < 8 || area.height == 0 {
        return;
    }
    let ascii = theme.flags.ascii;
    let x_axis_h = u16::from(show_x_axis && area.height >= 4);
    let plot_h = area.height.saturating_sub(x_axis_h);
    if plot_h == 0 {
        return;
    }

    // Reserve a crisp left Y-axis gutter when horizontal space allows.
    let y_axis_w: u16 = if area.width >= 30 { 7 } else { 0 };
    let plot = Rect::new(
        area.x + y_axis_w,
        area.y,
        area.width.saturating_sub(y_axis_w),
        plot_h,
    );
    if plot.width < 4 {
        return;
    }

    let sub_cols = if ascii {
        plot.width as usize
    } else {
        (plot.width as usize) * 2
    };
    let (dn_series, up_series) = resample_window(&snap.traffic_history, window.secs(), sub_cols);
    let dn_max = dn_series.iter().flatten().copied().max().unwrap_or(0);
    let up_max = up_series.iter().flatten().copied().max().unwrap_or(0);
    let combined_max = dn_series
        .iter()
        .zip(up_series.iter())
        .map(|(d, u)| d.unwrap_or(0).saturating_add(u.unwrap_or(0)))
        .max()
        .unwrap_or(0);

    let dn_ceil = nice_speed_ceiling(dn_max);
    let up_ceil = nice_speed_ceiling(up_max);
    let shared_ceil = nice_speed_ceiling(dn_max.max(up_max));
    let stack_ceil = nice_speed_ceiling(combined_max);

    match mode {
        TrafficGraphMode::Mirror => {
            draw_mirror_plot(
                buf, plot, y_axis_w, area.x, &dn_series, &up_series, dn_ceil, up_ceil, scale,
                theme, bg,
            );
        }
        TrafficGraphMode::Overlay => {
            draw_overlay_plot(
                buf,
                plot,
                y_axis_w,
                area.x,
                &dn_series,
                &up_series,
                shared_ceil,
                scale,
                theme,
                bg,
            );
        }
        TrafficGraphMode::Stacked => {
            draw_stacked_plot(
                buf, plot, y_axis_w, area.x, &dn_series, &up_series, stack_ceil, scale, theme, bg,
            );
        }
    }

    // Bottom X-axis time scale + rolling average summary
    if x_axis_h > 0 {
        let axis_y = plot.bottom();
        let axis_style = tokens::text(theme, Level::Tertiary).bg(bg);
        let left_lbl = format!("-{}", window.label());
        let mid_lbl = window.mid_label();
        let right_lbl = "now";
        buf.set_stringn(plot.x, axis_y, &left_lbl, plot.width as usize, axis_style);
        let lw = left_lbl.width() as u16;
        let rw = right_lbl.width() as u16;
        let mw = mid_lbl.width() as u16;
        if plot.width >= lw + rw + mw + 6 {
            let mid_x = plot.x + (plot.width.saturating_sub(mw)) / 2;
            buf.set_stringn(mid_x, axis_y, mid_lbl, mw as usize, axis_style);
        }
        if plot.width >= lw + rw + 2 {
            tokens::put_right(
                buf,
                Rect::new(plot.x, axis_y, plot.width, 1),
                right_lbl,
                axis_style,
            );
        }
        if y_axis_w >= 5 {
            let tag = match scale {
                TrafficScale::Linear => "LIN",
                TrafficScale::Log => "LOG",
            };
            buf.set_stringn(
                area.x,
                axis_y,
                tag,
                y_axis_w.saturating_sub(1) as usize,
                axis_style,
            );
        }
    }
}

fn draw_y_tick(buf: &mut Buffer, axis_x: u16, axis_w: u16, y: u16, label: &str, style: Style) {
    if axis_w < 3 {
        return;
    }
    let w = axis_w.saturating_sub(1) as usize;
    let clipped = clip_ascii_safe(label, w, true);
    let pad = w.saturating_sub(clipped.width());
    buf.set_stringn(axis_x + pad as u16, y, &clipped, w, style);
}

#[allow(clippy::too_many_arguments)]
fn draw_mirror_plot(
    buf: &mut Buffer,
    plot: Rect,
    y_axis_w: u16,
    axis_x: u16,
    dn_series: &[Option<u64>],
    up_series: &[Option<u64>],
    dn_ceil: u64,
    up_ceil: u64,
    scale: TrafficScale,
    theme: &Theme,
    bg: Color,
) {
    let ascii = theme.flags.ascii;
    let h = plot.height;
    let (dn_rows, horizon_y, up_rows) = if h >= 5 {
        let dn = (h - 1).div_ceil(2);
        let up = (h - 1) - dn;
        (dn, Some(plot.y + dn), up)
    } else if h >= 2 {
        let dn = h.div_ceil(2);
        (dn, None, h - dn)
    } else {
        (1, None, 0)
    };

    let dn_rect = Rect::new(plot.x, plot.y, plot.width, dn_rows);
    let up_y = horizon_y.map_or(plot.y + dn_rows, |hy| hy + 1);
    let up_rect = Rect::new(plot.x, up_y, plot.width, up_rows);

    // Y-axis ceiling labels for Download (top) and Upload (bottom)
    if y_axis_w > 0 {
        let dn_style = tokens::fg(theme, Role::Accent).bg(bg);
        let up_style = tokens::fg(theme, Role::Success).bg(bg);
        let mut_style = tokens::text(theme, Level::Tertiary).bg(bg);
        draw_y_tick(
            buf,
            axis_x,
            y_axis_w,
            dn_rect.y,
            &format_speed_axis(dn_ceil),
            dn_style,
        );
        if let Some(hy) = horizon_y {
            draw_y_tick(buf, axis_x, y_axis_w, hy, "0", mut_style);
        }
        if up_rows > 0 {
            draw_y_tick(
                buf,
                axis_x,
                y_axis_w,
                up_rect.bottom().saturating_sub(1),
                &format_speed_axis(up_ceil),
                up_style,
            );
        }
    }

    // Center horizon line with subtle quarter markers
    if let Some(hy) = horizon_y {
        let rule_sym = if ascii { "-" } else { "─" };
        let tick_sym = if ascii { "+" } else { "┼" };
        let style = Style::default().fg(grid_color(theme)).bg(bg);
        let w = plot.width as usize;
        for col in 0..w {
            let is_tick = w >= 20 && (col == w / 4 || col == w / 2 || col == (w * 3) / 4);
            buf[(plot.x + col as u16, hy)]
                .set_symbol(if is_tick { tick_sym } else { rule_sym })
                .set_style(style);
        }
    }

    // Upper half: Download rising UP from bottom of `dn_rect`
    draw_filled_half(
        buf,
        dn_rect,
        dn_series,
        dn_ceil,
        scale,
        Role::Accent,
        true,
        theme,
        bg,
    );
    // Lower half: Upload dropping DOWN from top of `up_rect`
    if up_rows > 0 {
        draw_filled_half(
            buf,
            up_rect,
            up_series,
            up_ceil,
            scale,
            Role::Success,
            false,
            theme,
            bg,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_filled_half(
    buf: &mut Buffer,
    rect: Rect,
    series: &[Option<u64>],
    ceiling: u64,
    scale: TrafficScale,
    role: Role,
    grow_up: bool,
    theme: &Theme,
    bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let ascii = theme.flags.ascii;
    let w = rect.width as usize;
    let h = rect.height as usize;
    let grid_style = Style::default().fg(grid_color(theme)).bg(bg);
    let mid_row = (h >= 3).then_some(h / 2);

    if ascii {
        const SHADES: [char; 5] = [' ', '.', ':', '+', '#'];
        let max_px = h * 4;
        let style = tokens::fg(theme, role).bg(bg);
        for cx in 0..w {
            let px = series
                .get(cx)
                .and_then(|v| *v)
                .map(|bps| scaled_height(bps, ceiling, scale, max_px))
                .unwrap_or(0);
            for cy in 0..h {
                let band = if grow_up { h - 1 - cy } else { cy };
                let band_lo = band * 4;
                let fill = px.saturating_sub(band_lo).min(4);
                let cell = &mut buf[(rect.x + cx as u16, rect.y + cy as u16)];
                if fill > 0 {
                    cell.set_char(SHADES[fill]).set_style(style);
                } else if mid_row == Some(cy) && cx % 2 == 0 {
                    cell.set_char('.').set_style(grid_style);
                }
            }
        }
        return;
    }

    let max_px = h * 4;
    let mut masks = vec![0u8; w * h];
    for (sx, val) in series.iter().enumerate().take(w * 2) {
        let Some(bps) = *val else {
            continue;
        };
        let px = scaled_height(bps, ceiling, scale, max_px);
        if px == 0 {
            continue;
        }
        let cx = sx / 2;
        let dx = sx % 2;
        for step in 0..px {
            let dot_y = if grow_up { max_px - 1 - step } else { step };
            let cy = dot_y / 4;
            let dy = dot_y % 4;
            masks[cy * w + cx] |= BRAILLE_BITS[dx][dy];
        }
    }

    for cy in 0..h {
        let elev = if grow_up {
            (h - 1 - cy) as f64 / h.max(1) as f64
        } else {
            cy as f64 / h.max(1) as f64
        };
        let fg = shaded_role_color(theme, role, elev);
        let style = Style::default().fg(fg).bg(bg);
        for cx in 0..w {
            let mask = masks[cy * w + cx];
            let cell = &mut buf[(rect.x + cx as u16, rect.y + cy as u16)];
            if mask != 0 {
                if let Some(ch) = char::from_u32(0x2800 + u32::from(mask)) {
                    cell.set_char(ch).set_style(style);
                }
            } else if mid_row == Some(cy) && cx % 2 == 0 {
                cell.set_symbol("·").set_style(grid_style);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_overlay_plot(
    buf: &mut Buffer,
    plot: Rect,
    y_axis_w: u16,
    axis_x: u16,
    dn_series: &[Option<u64>],
    up_series: &[Option<u64>],
    ceiling: u64,
    scale: TrafficScale,
    theme: &Theme,
    bg: Color,
) {
    let ascii = theme.flags.ascii;
    let w = plot.width as usize;
    let h = plot.height as usize;
    let grid_style = Style::default().fg(grid_color(theme)).bg(bg);
    let mid_row = (h >= 3).then_some(h / 2);

    if y_axis_w > 0 {
        let axis_style = tokens::text(theme, Level::Tertiary).bg(bg);
        draw_y_tick(
            buf,
            axis_x,
            y_axis_w,
            plot.y,
            &format_speed_axis(ceiling),
            axis_style,
        );
        if let Some(mr) = mid_row {
            let mid_bps = match scale {
                TrafficScale::Linear => ceiling / 2,
                TrafficScale::Log => ((ceiling as f64).sqrt()).round() as u64,
            };
            draw_y_tick(
                buf,
                axis_x,
                y_axis_w,
                plot.y + mr as u16,
                &format_speed_axis(mid_bps),
                axis_style,
            );
        }
        if h >= 2 {
            draw_y_tick(
                buf,
                axis_x,
                y_axis_w,
                plot.bottom().saturating_sub(1),
                "0",
                axis_style,
            );
        }
    }

    if ascii {
        let max_y = h.saturating_sub(1);
        let dn_style = tokens::fg(theme, Role::Accent).bg(bg);
        let up_style = tokens::fg(theme, Role::Success).bg(bg);
        let both_style = tokens::fg(theme, Role::Warning).bg(bg);
        for cy in 0..h {
            if mid_row == Some(cy) {
                for cx in (0..w).step_by(2) {
                    buf[(plot.x + cx as u16, plot.y + cy as u16)]
                        .set_char('.')
                        .set_style(grid_style);
                }
            }
        }
        for cx in 0..w {
            let dn_row = dn_series.get(cx).and_then(|v| *v).map(|bps| {
                let k = (normalize_bps(bps, ceiling, scale) * max_y as f64).round() as usize;
                max_y.saturating_sub(k.min(max_y))
            });
            let up_row = up_series.get(cx).and_then(|v| *v).map(|bps| {
                let k = (normalize_bps(bps, ceiling, scale) * max_y as f64).round() as usize;
                max_y.saturating_sub(k.min(max_y))
            });
            match (dn_row, up_row) {
                (Some(dr), Some(ur)) if dr == ur => {
                    buf[(plot.x + cx as u16, plot.y + dr as u16)]
                        .set_char('X')
                        .set_style(both_style);
                }
                (dr, ur) => {
                    if let Some(r) = dr {
                        buf[(plot.x + cx as u16, plot.y + r as u16)]
                            .set_char('v')
                            .set_style(dn_style);
                    }
                    if let Some(r) = ur {
                        buf[(plot.x + cx as u16, plot.y + r as u16)]
                            .set_char('^')
                            .set_style(up_style);
                    }
                }
            }
        }
        return;
    }

    let max_px = h * 4;
    let mut dn_masks = vec![0u8; w * h];
    let mut up_masks = vec![0u8; w * h];

    let plot_curve = |series: &[Option<u64>], masks: &mut [u8], light_fill: bool| {
        let mut prev_y: Option<usize> = None;
        for (sx, val) in series.iter().enumerate().take(w * 2) {
            let Some(bps) = *val else {
                prev_y = None;
                continue;
            };
            let px = scaled_height(bps, ceiling, scale, max_px);
            let y = if px == 0 {
                max_px - 1
            } else {
                max_px.saturating_sub(px)
            };
            let cx = sx / 2;
            let dx = sx % 2;
            let (lo, hi) = match prev_y {
                Some(py) => (py.min(y), py.max(y)),
                None => (y, y),
            };
            for dot_y in lo..=hi {
                let cy = dot_y / 4;
                let dy = dot_y % 4;
                masks[cy * w + cx] |= BRAILLE_BITS[dx][dy];
            }
            // Light dotted skirt under primary curve on alternating sub-columns for visual depth
            if light_fill && px > 1 && sx % 2 == 0 {
                for dot_y in ((y + 2)..max_px).step_by(2) {
                    let cy = dot_y / 4;
                    let dy = dot_y % 4;
                    masks[cy * w + cx] |= BRAILLE_BITS[dx][dy];
                }
            }
            prev_y = Some(y);
        }
    };

    plot_curve(dn_series, &mut dn_masks, true);
    plot_curve(up_series, &mut up_masks, false);

    let both_col = overlap_color(theme);
    for cy in 0..h {
        let elev = (h - 1 - cy) as f64 / h.max(1) as f64;
        let dn_col = shaded_role_color(theme, Role::Accent, elev);
        let up_col = shaded_role_color(theme, Role::Success, elev);
        for cx in 0..w {
            let dm = dn_masks[cy * w + cx];
            let um = up_masks[cy * w + cx];
            let mask = dm | um;
            let cell = &mut buf[(plot.x + cx as u16, plot.y + cy as u16)];
            if mask != 0 {
                let fg = match (dm != 0, um != 0) {
                    (true, true) => both_col,
                    (true, false) => dn_col,
                    (false, true) => up_col,
                    (false, false) => continue,
                };
                if let Some(ch) = char::from_u32(0x2800 + u32::from(mask)) {
                    cell.set_char(ch).set_style(Style::default().fg(fg).bg(bg));
                }
            } else if mid_row == Some(cy) && cx % 2 == 0 {
                cell.set_symbol("·").set_style(grid_style);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_stacked_plot(
    buf: &mut Buffer,
    plot: Rect,
    y_axis_w: u16,
    axis_x: u16,
    dn_series: &[Option<u64>],
    up_series: &[Option<u64>],
    ceiling: u64,
    scale: TrafficScale,
    theme: &Theme,
    bg: Color,
) {
    let ascii = theme.flags.ascii;
    let w = plot.width as usize;
    let h = plot.height as usize;
    let grid_style = Style::default().fg(grid_color(theme)).bg(bg);
    let mid_row = (h >= 3).then_some(h / 2);

    if y_axis_w > 0 {
        let axis_style = tokens::text(theme, Level::Tertiary).bg(bg);
        draw_y_tick(
            buf,
            axis_x,
            y_axis_w,
            plot.y,
            &format_speed_axis(ceiling),
            axis_style,
        );
        if let Some(mr) = mid_row {
            let mid_bps = match scale {
                TrafficScale::Linear => ceiling / 2,
                TrafficScale::Log => ((ceiling as f64).sqrt()).round() as u64,
            };
            draw_y_tick(
                buf,
                axis_x,
                y_axis_w,
                plot.y + mr as u16,
                &format_speed_axis(mid_bps),
                axis_style,
            );
        }
        if h >= 2 {
            draw_y_tick(
                buf,
                axis_x,
                y_axis_w,
                plot.bottom().saturating_sub(1),
                "0",
                axis_style,
            );
        }
    }

    let max_px = h * 4;
    if ascii {
        let dn_style = tokens::fg(theme, Role::Accent).bg(bg);
        let up_style = tokens::fg(theme, Role::Success).bg(bg);
        for cx in 0..w {
            let dn = dn_series.get(cx).and_then(|v| *v).unwrap_or(0);
            let up = up_series.get(cx).and_then(|v| *v).unwrap_or(0);
            let tot = dn.saturating_add(up);
            let tot_px = scaled_height(tot, ceiling, scale, max_px);
            let dn_px = if tot == 0 {
                0
            } else {
                ((tot_px as f64) * (dn as f64 / tot as f64)).round() as usize
            };
            for cy in 0..h {
                let band_lo = (h - 1 - cy) * 4;
                let cell = &mut buf[(plot.x + cx as u16, plot.y + cy as u16)];
                if tot_px > band_lo {
                    if dn_px > band_lo + 1 {
                        cell.set_char('#').set_style(dn_style);
                    } else {
                        cell.set_char('+').set_style(up_style);
                    }
                } else if mid_row == Some(cy) && cx % 2 == 0 {
                    cell.set_char('.').set_style(grid_style);
                }
            }
        }
        return;
    }

    let mut dn_masks = vec![0u8; w * h];
    let mut up_masks = vec![0u8; w * h];
    for sx in 0..(w * 2) {
        let (Some(dn), Some(up)) = (
            dn_series.get(sx).and_then(|v| *v),
            up_series.get(sx).and_then(|v| *v),
        ) else {
            continue;
        };
        let tot = dn.saturating_add(up);
        let tot_px = scaled_height(tot, ceiling, scale, max_px);
        if tot_px == 0 {
            continue;
        }
        let dn_px = ((tot_px as f64) * (dn as f64 / tot.max(1) as f64)).round() as usize;
        let dn_px = dn_px.min(tot_px);
        let cx = sx / 2;
        let dx = sx % 2;
        for step in 0..tot_px {
            let dot_y = max_px - 1 - step;
            let cy = dot_y / 4;
            let dy = dot_y % 4;
            if step < dn_px {
                dn_masks[cy * w + cx] |= BRAILLE_BITS[dx][dy];
            } else {
                up_masks[cy * w + cx] |= BRAILLE_BITS[dx][dy];
            }
        }
    }

    for cy in 0..h {
        let elev = (h - 1 - cy) as f64 / h.max(1) as f64;
        let dn_col = shaded_role_color(theme, Role::Accent, elev);
        let up_col = shaded_role_color(theme, Role::Success, elev);
        for cx in 0..w {
            let dm = dn_masks[cy * w + cx];
            let um = up_masks[cy * w + cx];
            let mask = dm | um;
            let cell = &mut buf[(plot.x + cx as u16, plot.y + cy as u16)];
            if mask != 0 {
                let fg = if um.count_ones() >= dm.count_ones() && um != 0 {
                    up_col
                } else {
                    dn_col
                };
                if let Some(ch) = char::from_u32(0x2800 + u32::from(mask)) {
                    cell.set_char(ch).set_style(Style::default().fg(fg).bg(bg));
                }
            } else if mid_row == Some(cy) && cx % 2 == 0 {
                cell.set_symbol("·").set_style(grid_style);
            }
        }
    }
}

/// Draw a compact ranked traffic distribution bar list inside `rect`.
fn draw_bucket_bars(
    buf: &mut Buffer,
    rect: Rect,
    header: Option<&str>,
    buckets: &[TrafficBucket],
    empty_label: &str,
    theme: &Theme,
    bg: Color,
) {
    if rect.width < 12 || rect.height == 0 {
        return;
    }
    let ascii = theme.flags.ascii;
    let mut y = rect.y;
    let w = rect.width as usize;

    if let Some(title) = header {
        buf.set_stringn(
            rect.x,
            y,
            clip_ascii_safe(title, w, ascii),
            w,
            tokens::text(theme, Level::Secondary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        y += 1;
    }

    let avail_rows = rect.bottom().saturating_sub(y) as usize;
    if avail_rows == 0 {
        return;
    }
    if buckets.is_empty() {
        buf.set_stringn(
            rect.x,
            y,
            clip_ascii_safe(empty_label, w, ascii),
            w,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    let total_speed: u64 = buckets.iter().map(TrafficBucket::total_speed).sum();
    let total_bytes: u64 = buckets.iter().map(TrafficBucket::total_bytes).sum();
    let use_speed = total_speed > 0;
    let denom = if use_speed {
        total_speed
    } else {
        total_bytes.max(1)
    };

    let fill_ch = if ascii { '#' } else { '━' };
    let track_ch = if ascii { '-' } else { '─' };
    let track_style = Style::default().fg(grid_color(theme)).bg(bg);

    for (idx, bucket) in buckets.iter().take(avail_rows).enumerate() {
        let row_y = y + idx as u16;
        let metric = if use_speed {
            bucket.total_speed()
        } else {
            bucket.total_bytes()
        };
        let ratio = (metric as f64 / denom as f64).clamp(0.0, 1.0);
        let pct = (ratio * 100.0).round() as u64;
        let val_str = if use_speed {
            format_speed_compact(metric)
        } else {
            format_bytes_compact(metric)
        };
        let right_str = if w >= 28 {
            format!("{val_str} {pct:>2}%")
        } else {
            val_str
        };
        let right_w = right_str.width();

        let role = if bucket.name.eq_ignore_ascii_case("DIRECT") {
            Role::Success
        } else if bucket.name.eq_ignore_ascii_case("REJECT") {
            Role::Danger
        } else if idx == 0 {
            Role::Accent
        } else {
            Role::Info
        };

        // Layout: `<name> <bar> <right>`
        let bar_w = if w >= 34 {
            8usize
        } else if w >= 24 {
            5usize
        } else {
            0usize
        };
        let name_w = w.saturating_sub(right_w + if bar_w > 0 { bar_w + 2 } else { 1 });
        if name_w >= 2 {
            buf.set_stringn(
                rect.x,
                row_y,
                clip_ascii_safe(&bucket.name, name_w, ascii),
                name_w,
                tokens::text(
                    theme,
                    if idx == 0 {
                        Level::Primary
                    } else {
                        Level::Secondary
                    },
                )
                .bg(bg),
            );
        }
        if bar_w > 0 {
            let bx = rect.x + name_w as u16 + 1;
            let mut filled = (ratio * bar_w as f64).round() as usize;
            if metric > 0 && filled == 0 {
                filled = 1;
            }
            filled = filled.min(bar_w);
            let on_style = tokens::fg(theme, role).bg(bg);
            for c in 0..bar_w {
                let cell = &mut buf[(bx + c as u16, row_y)];
                if c < filled {
                    cell.set_char(fill_ch).set_style(on_style);
                } else {
                    cell.set_char(track_ch).set_style(track_style);
                }
            }
        }
        tokens::put_right(
            buf,
            Rect::new(rect.x, row_y, rect.width, 1),
            &right_str,
            tokens::fg(theme, role).bg(bg),
        );
    }
}

/// Render the Hero Real-Time Traffic Atlas card on the Mihomo Dashboard (`Overview`).
pub(super) fn draw_overview_card(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let card = geom.traffic_card;
    if card.width < 20 || card.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let snap = cl.traffic_snapshot();
    let ascii = theme.flags.ascii;

    tokens::draw_surface_card_fx(
        frame,
        card,
        theme,
        Role::SurfaceAlt,
        Role::Border,
        false,
        app.now.saturating_duration_since(app.started),
        app.reduced_motion(),
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();

    let sep = tokens::glyph(theme, Glyph::Separator);
    let dn_sym = if ascii { "v" } else { "↓" };
    let up_sym = if ascii { "^" } else { "↑" };
    let title = if cl.is_traffic_paused() {
        format!(
            "{} [{}]",
            i.text("clash.traffic.atlas_title"),
            i.text("clash.traffic.paused")
        )
    } else {
        i.text("clash.traffic.atlas_title").to_owned()
    };
    let title_meta = if card.width >= 56 {
        format!(
            "{dn_sym}{} {up_sym}{} {sep} TCP {} / UDP {}",
            format_bytes_compact(snap.download_total),
            format_bytes_compact(snap.upload_total),
            snap.tcp_count(),
            snap.udp_count()
        )
    } else {
        format!(
            "{dn_sym}{} {up_sym}{}",
            format_bytes_compact(snap.download_total),
            format_bytes_compact(snap.upload_total),
        )
    };
    draw_card_title(buf, card, &title, &title_meta, theme, Role::SurfaceAlt);

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4);
    if iw < 12 {
        return;
    }

    // Row 1: Live Download / Upload KPI readout + interactive control pills
    let kpi_y = card.y + 1;
    let chips_left = [
        geom.traffic_mode_chip,
        geom.traffic_window_chip,
        geom.traffic_scale_chip,
        geom.traffic_pause_chip,
        geom.traffic_expand_btn,
    ]
    .into_iter()
    .flatten()
    .map(|r| r.x)
    .min()
    .unwrap_or(ix + iw);
    let kpi_budget = chips_left.saturating_sub(ix + 1) as usize;

    let stats = snap.traffic_window_stats(cl.traffic_window.secs());
    let dn_text = if kpi_budget >= 44 {
        format!(
            "{dn_sym} {} ({} {})",
            format_speed(snap.down_speed),
            i.text("clash.overview.peak"),
            format_speed_compact(stats.peak_down.max(snap.down_speed))
        )
    } else {
        format!("{dn_sym} {}", format_speed_compact(snap.down_speed))
    };
    let up_text = if kpi_budget >= 44 {
        format!(
            "{up_sym} {} ({} {})",
            format_speed(snap.up_speed),
            i.text("clash.overview.peak"),
            format_speed_compact(stats.peak_up.max(snap.up_speed))
        )
    } else {
        format!("{up_sym} {}", format_speed_compact(snap.up_speed))
    };

    let dn_clip = clip_ascii_safe(&dn_text, kpi_budget, ascii);
    let dn_w = dn_clip.width();
    buf.set_stringn(
        ix,
        kpi_y,
        &dn_clip,
        kpi_budget,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let rem = kpi_budget.saturating_sub(dn_w + 2);
    if rem >= 6 {
        let up_clip = clip_ascii_safe(&up_text, rem, ascii);
        buf.set_stringn(
            ix + dn_w as u16 + 2,
            kpi_y,
            &up_clip,
            rem,
            tokens::fg(theme, Role::Success)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    }

    if let Some(chip) = geom.traffic_mode_chip {
        let lbl = format!(" [g] {} ", i.text(cl.traffic_mode.label_key()));
        let hov = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
    }
    if let Some(chip) = geom.traffic_window_chip {
        let lbl = format!(" [t] {} ", cl.traffic_window.label());
        let hov = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
    }
    if let Some(chip) = geom.traffic_scale_chip {
        let lbl = format!(" [y] {} ", i.text(cl.traffic_scale.label_key()));
        let hov = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(
            buf,
            chip,
            &lbl,
            cl.traffic_scale == TrafficScale::Log,
            hov,
            theme,
            bg,
        );
    }
    if let Some(chip) = geom.traffic_pause_chip {
        let key = if cl.is_traffic_paused() {
            "clash.traffic.resume"
        } else {
            "clash.traffic.pause"
        };
        let lbl = format!(" {} ", i.text(key));
        let hov = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(buf, chip, &lbl, cl.is_traffic_paused(), hov, theme, bg);
    }
    if let Some(btn) = geom.traffic_expand_btn {
        let lbl = format!(" [v] {} ", i.text("clash.key.traffic_atlas"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
    }

    // Main waveform plot
    let plot_area = geom.traffic_plot;
    if plot_area.width >= 8 && plot_area.height >= 1 {
        draw_waveform_plot(
            buf,
            plot_area,
            snap,
            WaveformSpec {
                mode: cl.traffic_mode,
                window: cl.traffic_window,
                scale: cl.traffic_scale,
                show_x_axis: plot_area.height >= 4,
            },
            theme,
            bg,
        );
    }

    // Right side mini-atlas: Live traffic share by Exit Node on wide screens
    let side = geom.traffic_side;
    if side.width >= 18 && side.height >= 3 {
        let div_x = side.x.saturating_sub(2);
        if div_x > plot_area.x {
            let div_sym = if ascii { "|" } else { "│" };
            let div_style = tokens::fg(theme, Role::Rule).bg(bg);
            for y in side.top()..side.bottom() {
                buf[(div_x, y)].set_symbol(div_sym).set_style(div_style);
            }
        }
        let buckets = snap.traffic_by_exit(side.height.saturating_sub(1) as usize);
        draw_bucket_bars(
            buf,
            side,
            Some(i.text("clash.traffic.by_exit")),
            &buckets,
            i.text("clash.conn.empty"),
            theme,
            bg,
        );
    }
}

/// Render the dedicated Fullscreen Real-Time Traffic Atlas (`[v]` or clicking Overview traffic graph).
pub(super) fn draw_fullscreen_atlas(app: &App, frame: &mut Frame, geom: &TrafficFullscreenGeom) {
    let area = geom.area;
    if area.width < 20 || area.height < 6 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let snap = cl.traffic_snapshot();
    let ascii = theme.flags.ascii;
    let bg = surface_bg(theme, Role::Surface);
    let sep = tokens::glyph(theme, Glyph::Separator);
    let dn_sym = if ascii { "v" } else { "↓" };
    let up_sym = if ascii { "^" } else { "↑" };

    // 1. Header toolbar with live badge + clickable control chips
    if geom.toolbar.height > 0 {
        let buf = frame.buffer_mut();
        fill_rect_bg(buf, geom.toolbar, bg);
        let (dot, state_lbl, state_role) = if cl.is_traffic_paused() {
            (
                tokens::glyph(theme, Glyph::Cancelled),
                i.text("clash.traffic.paused"),
                Role::Warning,
            )
        } else {
            (
                tokens::glyph(theme, Glyph::DotOn),
                i.text("clash.traffic.live"),
                Role::Success,
            )
        };
        let full_title = format!(
            "{dot} {} {sep} {state_lbl}",
            i.text("clash.traffic.atlas_title")
        );
        let short_title = format!("{dot} {}", i.text("clash.traffic.atlas_title"));
        let chips_left = [
            geom.mode_chip,
            geom.window_chip,
            geom.scale_chip,
            geom.pause_chip,
            geom.breakdown_chip,
            geom.close_btn,
        ]
        .into_iter()
        .flatten()
        .map(|r| r.x)
        .min()
        .unwrap_or(geom.toolbar.right());
        let title_w = chips_left.saturating_sub(geom.toolbar.x + 1) as usize;
        let title_str = if full_title.width() <= title_w {
            full_title
        } else {
            short_title
        };
        buf.set_stringn(
            geom.toolbar.x,
            geom.toolbar.y,
            clip_ascii_safe(&title_str, title_w, ascii),
            title_w,
            tokens::fg(theme, state_role)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );

        if let Some(chip) = geom.mode_chip {
            let lbl = format!(" [g] {} ", i.text(cl.traffic_mode.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
        }
        if let Some(chip) = geom.window_chip {
            let lbl = format!(" [t] {} ", cl.traffic_window.label());
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
        }
        if let Some(chip) = geom.scale_chip {
            let lbl = format!(" [y] {} ", i.text(cl.traffic_scale.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(
                buf,
                chip,
                &lbl,
                cl.traffic_scale == TrafficScale::Log,
                hov,
                theme,
                bg,
            );
        }
        if let Some(chip) = geom.pause_chip {
            let key = if cl.is_traffic_paused() {
                "clash.traffic.resume"
            } else {
                "clash.traffic.pause"
            };
            let lbl = format!(" [Space] {} ", i.text(key));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, cl.is_traffic_paused(), hov, theme, bg);
        }
        if let Some(chip) = geom.breakdown_chip {
            let lbl = format!(" [b] {} ", i.text(cl.traffic_breakdown.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
        }
        if let Some(btn) = geom.close_btn {
            let lbl = format!(" [Esc] {} ", i.text("key.close"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }

    // 2. Waveform hero card (KPI strip + giant Braille canvas)
    let wave = geom.waveform_card;
    if wave.width >= 16 && wave.height >= 4 {
        tokens::draw_surface_card_fx(
            frame,
            wave,
            theme,
            Role::SurfaceAlt,
            Role::BorderFocus,
            false,
            app.now.saturating_duration_since(app.started),
            app.reduced_motion(),
        );
        let wbg = surface_bg(theme, Role::SurfaceAlt);
        let buf = frame.buffer_mut();

        let stats = snap.traffic_window_stats(cl.traffic_window.secs());
        let mem_meta = snap
            .memory_bytes
            .map(|b| {
                format!(
                    " {sep} {} {}",
                    i.text("clash.overview.memory"),
                    format_bytes(b)
                )
            })
            .unwrap_or_default();
        let right_meta = format!(
            "TCP {} / UDP {}{mem_meta}",
            snap.tcp_count(),
            snap.udp_count()
        );
        draw_card_title(
            buf,
            wave,
            &format!(
                "{} ({})",
                i.text("clash.traffic.waveform"),
                cl.traffic_window.label()
            ),
            &right_meta,
            theme,
            Role::SurfaceAlt,
        );

        let ix = wave.x + 2;
        let iw = wave.width.saturating_sub(4) as usize;
        let kpi_dn = if iw >= 44 {
            format!(
                "{dn_sym} {}  {} {}  {} {}  {} {}",
                format_speed(snap.down_speed),
                i.text("clash.traffic.avg"),
                format_speed_compact(stats.avg_down),
                i.text("clash.overview.peak"),
                format_speed_compact(stats.peak_down.max(snap.down_speed)),
                i.text("clash.overview.total"),
                format_bytes_compact(snap.download_total),
            )
        } else {
            format!(
                "{dn_sym}{} {}{} {}{}",
                format_speed_compact(snap.down_speed),
                i.text("clash.traffic.avg"),
                format_speed_compact(stats.avg_down),
                i.text("clash.overview.peak"),
                format_speed_compact(stats.peak_down.max(snap.down_speed)),
            )
        };
        let kpi_up = if iw >= 44 {
            format!(
                "{up_sym} {}  {} {}  {} {}  {} {}",
                format_speed(snap.up_speed),
                i.text("clash.traffic.avg"),
                format_speed_compact(stats.avg_up),
                i.text("clash.overview.peak"),
                format_speed_compact(stats.peak_up.max(snap.up_speed)),
                i.text("clash.overview.total"),
                format_bytes_compact(snap.upload_total),
            )
        } else {
            format!(
                "{up_sym}{} {}{} {}{}",
                format_speed_compact(snap.up_speed),
                i.text("clash.traffic.avg"),
                format_speed_compact(stats.avg_up),
                i.text("clash.overview.peak"),
                format_speed_compact(stats.peak_up.max(snap.up_speed)),
            )
        };

        let mut plot_top = wave.y + 1;
        if iw >= 84 && wave.height >= 5 {
            // Single compact KPI row on wide viewports to maximize waveform height
            let half = iw / 2;
            buf.set_stringn(
                ix,
                plot_top,
                clip_ascii_safe(&kpi_dn, half.saturating_sub(1), ascii),
                half.saturating_sub(1),
                tokens::fg(theme, Role::Accent)
                    .bg(wbg)
                    .add_modifier(Modifier::BOLD),
            );
            buf.set_stringn(
                ix + half as u16,
                plot_top,
                clip_ascii_safe(&kpi_up, iw.saturating_sub(half), ascii),
                iw.saturating_sub(half),
                tokens::fg(theme, Role::Success)
                    .bg(wbg)
                    .add_modifier(Modifier::BOLD),
            );
            plot_top += 1;
        } else if wave.height >= 6 {
            buf.set_stringn(
                ix,
                plot_top,
                clip_ascii_safe(&kpi_dn, iw, ascii),
                iw,
                tokens::fg(theme, Role::Accent)
                    .bg(wbg)
                    .add_modifier(Modifier::BOLD),
            );
            buf.set_stringn(
                ix,
                plot_top + 1,
                clip_ascii_safe(&kpi_up, iw, ascii),
                iw,
                tokens::fg(theme, Role::Success)
                    .bg(wbg)
                    .add_modifier(Modifier::BOLD),
            );
            plot_top += 2;
        } else {
            let compact = format!(
                "{dn_sym}{}  {up_sym}{}",
                format_speed_compact(snap.down_speed),
                format_speed_compact(snap.up_speed)
            );
            buf.set_stringn(
                ix,
                plot_top,
                clip_ascii_safe(&compact, iw, ascii),
                iw,
                tokens::fg(theme, Role::Accent)
                    .bg(wbg)
                    .add_modifier(Modifier::BOLD),
            );
            plot_top += 1;
        }

        let plot_bottom = wave.bottom().saturating_sub(1);
        if plot_bottom > plot_top {
            let plot_rect = Rect::new(ix, plot_top, iw as u16, plot_bottom - plot_top);
            draw_waveform_plot(
                buf,
                plot_rect,
                snap,
                WaveformSpec {
                    mode: cl.traffic_mode,
                    window: cl.traffic_window,
                    scale: cl.traffic_scale,
                    show_x_axis: plot_rect.height >= 4,
                },
                theme,
                wbg,
            );
        }
    }

    // 3. Bottom Traffic Distribution / Topology Atlas cards
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    let multi_panel = geom.panel_two.width > 0;

    if geom.panel_one.width >= 16 && geom.panel_one.height >= 4 {
        let kind = if multi_panel {
            TrafficBreakdownKind::Exit
        } else {
            cl.traffic_breakdown
        };
        draw_atlas_panel(app, frame, geom.panel_one, snap, kind, elapsed, reduced);
    }
    if geom.panel_two.width >= 16 && geom.panel_two.height >= 4 {
        let kind = if geom.panel_three.width > 0 {
            TrafficBreakdownKind::Rule
        } else {
            match cl.traffic_breakdown {
                TrafficBreakdownKind::Exit => TrafficBreakdownKind::Rule,
                other => other,
            }
        };
        draw_atlas_panel(app, frame, geom.panel_two, snap, kind, elapsed, reduced);
    }
    if geom.panel_three.width >= 16 && geom.panel_three.height >= 4 {
        let kind = match cl.traffic_breakdown {
            TrafficBreakdownKind::Process => TrafficBreakdownKind::Process,
            _ => TrafficBreakdownKind::Flows,
        };
        draw_atlas_panel(app, frame, geom.panel_three, snap, kind, elapsed, reduced);
    }
}

fn draw_atlas_panel(
    app: &App,
    frame: &mut Frame,
    card: Rect,
    snap: &ClashSnapshot,
    kind: TrafficBreakdownKind,
    elapsed: std::time::Duration,
    reduced: bool,
) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    tokens::draw_surface_card_fx(
        frame,
        card,
        theme,
        Role::SurfaceAlt,
        Role::Border,
        false,
        elapsed,
        reduced,
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();

    let inner = Rect::new(
        card.x + 2,
        card.y + 1,
        card.width.saturating_sub(4),
        card.height.saturating_sub(2),
    );
    let limit = inner.height.max(1) as usize;

    match kind {
        TrafficBreakdownKind::Exit => {
            let buckets = snap.traffic_by_exit(limit);
            draw_card_title(
                buf,
                card,
                i.text("clash.traffic.by_exit"),
                &format!("{}", buckets.len()),
                theme,
                Role::SurfaceAlt,
            );
            draw_bucket_bars(
                buf,
                inner,
                None,
                &buckets,
                i.text("clash.conn.empty"),
                theme,
                bg,
            );
        }
        TrafficBreakdownKind::Rule => {
            let buckets = snap.traffic_by_rule(limit);
            draw_card_title(
                buf,
                card,
                i.text("clash.traffic.by_rule"),
                &format!("{}", buckets.len()),
                theme,
                Role::SurfaceAlt,
            );
            draw_bucket_bars(
                buf,
                inner,
                None,
                &buckets,
                i.text("clash.conn.empty"),
                theme,
                bg,
            );
        }
        TrafficBreakdownKind::Process => {
            let buckets = snap.traffic_by_process(limit);
            draw_card_title(
                buf,
                card,
                i.text("clash.traffic.by_process"),
                &format!("{}", buckets.len()),
                theme,
                Role::SurfaceAlt,
            );
            draw_bucket_bars(
                buf,
                inner,
                None,
                &buckets,
                i.text("clash.conn.empty"),
                theme,
                bg,
            );
        }
        TrafficBreakdownKind::Flows => {
            draw_card_title(
                buf,
                card,
                i.text("clash.traffic.top_flows"),
                &format!("{}", snap.connections.len()),
                theme,
                Role::SurfaceAlt,
            );
            let mut sorted: Vec<_> = snap.connections.iter().collect();
            sorted.sort_by(|a, b| {
                b.down_speed
                    .saturating_add(b.up_speed)
                    .cmp(&a.down_speed.saturating_add(a.up_speed))
                    .then_with(|| b.total_bytes().cmp(&a.total_bytes()))
            });
            let iw = inner.width as usize;
            if sorted.is_empty() {
                buf.set_stringn(
                    inner.x,
                    inner.y,
                    clip_ascii_safe(i.text("clash.conn.empty"), iw, ascii),
                    iw,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
                return;
            }
            let dn_sym = if ascii { "v" } else { "↓" };
            for (row_idx, conn) in sorted.into_iter().take(limit).enumerate() {
                let y = inner.y + row_idx as u16;
                let rate = conn.down_speed.saturating_add(conn.up_speed);
                let right = if rate > 0 {
                    format!("{dn_sym}{}", format_speed_compact(rate))
                } else {
                    format_bytes_compact(conn.total_bytes())
                };
                let right_w = right.width();
                let exit_w = if iw >= 32 { 10 } else { 0 };
                let host_w = iw.saturating_sub(right_w + if exit_w > 0 { exit_w + 2 } else { 1 });
                buf.set_stringn(
                    inner.x,
                    y,
                    clip_ascii_safe(conn.display_target(), host_w, ascii),
                    host_w,
                    tokens::text(theme, Level::Primary).bg(bg),
                );
                if exit_w > 0 {
                    buf.set_stringn(
                        inner.x + host_w as u16 + 1,
                        y,
                        clip_ascii_safe(&conn.exit_node, exit_w, ascii),
                        exit_w,
                        tokens::fg(theme, Role::Accent).bg(bg),
                    );
                }
                tokens::put_right(
                    buf,
                    Rect::new(inner.x, y, inner.width, 1),
                    &right,
                    tokens::fg(theme, if rate > 0 { Role::Success } else { Role::Text3 }).bg(bg),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_window_preserves_right_alignment_and_burst_peaks() {
        let mut hist = VecDeque::new();
        for i in 0..10u64 {
            hist.push_back(TrafficSample {
                up_bps: i * 100,
                down_bps: i * 1000,
                active_conns: 1,
            });
        }
        // 10 samples in a 60s window across 60 sub-cols -> rightmost 10 cols populated, leftmost 50 None
        let (dn, up) = resample_window(&hist, 60, 60);
        assert_eq!(dn.len(), 60);
        assert!(dn[..50].iter().all(Option::is_none));
        assert_eq!(dn[50], Some(0));
        assert_eq!(dn[59], Some(9000));
        assert_eq!(up[59], Some(900));

        // Downsampling 60 samples with a single burst spike into 10 cols must preserve the spike
        let mut burst = VecDeque::new();
        for i in 0..60u64 {
            burst.push_back(TrafficSample {
                up_bps: if i == 33 { 500_000 } else { 100 },
                down_bps: if i == 33 { 2_000_000 } else { 200 },
                active_conns: 1,
            });
        }
        let (dn_ds, up_ds) = resample_window(&burst, 60, 10);
        assert_eq!(dn_ds.iter().flatten().copied().max(), Some(2_000_000));
        assert_eq!(up_ds.iter().flatten().copied().max(), Some(500_000));
    }

    #[test]
    fn normalize_bps_handles_linear_and_log_bounds() {
        assert_eq!(normalize_bps(0, 1_000_000, TrafficScale::Linear), 0.0);
        assert!((normalize_bps(500_000, 1_000_000, TrafficScale::Linear) - 0.5).abs() < 1e-6);
        assert_eq!(normalize_bps(0, 1_000_000, TrafficScale::Log), 0.0);
        let log_small = normalize_bps(10_000, 100_000_000, TrafficScale::Log);
        let lin_small = normalize_bps(10_000, 100_000_000, TrafficScale::Linear);
        // Log scale keeps 10 KB/s clearly readable even next to a 100 MB/s ceiling.
        assert!(log_small > 0.20 && log_small > lin_small * 100.0);
    }
}
