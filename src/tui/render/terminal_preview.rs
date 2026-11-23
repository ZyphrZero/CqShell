//! Interactive Terminal Prompt Preview Studio (`terminal.preview`), live embedded form
//! preview (`terminal.apply`), and workbench inspector preview snippets.

use super::{
    clip_ascii_safe,
    tokens::{self, Glyph},
    wrap_words,
};
use crate::{
    actions,
    ops::terminal::{
        self, COLORS, Options, PreviewLine, PreviewScenario, PromptPalette, Rgb, STYLES,
    },
    theme::{Level, Role, Theme},
    tui::{
        fx,
        layout::PromptPreviewGeometry,
        preview as cmd_preview,
        state::{App, PromptPreview},
    },
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthChar;

/// Deep dark terminal backdrop for realistic prompt contrast across both dark and light UI themes.
const MOCK_TERM_BG: Color = Color::Rgb(0x0f, 0x13, 0x1c);
const MOCK_BAR_BG: Color = Color::Rgb(0x16, 0x1b, 0x28);
const MOCK_MUTED: Color = Color::Rgb(0x6e, 0x77, 0x8d);
const MOCK_DIM_TEXT: Color = Color::Rgb(0x88, 0x92, 0xa8);

fn rgb_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}

fn fill_rect(buf: &mut Buffer, rect: Rect, bg: Color) {
    let area = rect.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)]
                .set_symbol(" ")
                .set_style(Style::default().bg(bg));
        }
    }
}

/// Write one styled [`PreviewLine`] clipped strictly inside `[x, x + max_w)`.
fn draw_preview_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_w: u16,
    line: &PreviewLine,
    base_bg: Color,
    ascii: bool,
) {
    if max_w == 0 || y >= buf.area.bottom() {
        return;
    }
    let end_x = (x + max_w).min(buf.area.right());
    let mut cx = x;
    for span in &line.spans {
        let mut style = Style::default().bg(base_bg);
        if !ascii {
            if let Some(fg) = span.fg {
                style = style.fg(rgb_color(fg));
            } else {
                style = style.fg(Color::Rgb(0xd8, 0xde, 0xe9));
            }
            if let Some(bg) = span.bg {
                style = style.bg(rgb_color(bg));
            }
        }
        if span.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if span.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        for ch in span.text.chars() {
            let cw = ch.width().unwrap_or(1) as u16;
            if cw == 0 {
                continue;
            }
            if cx + cw > end_x {
                return;
            }
            let mut s = [0u8; 4];
            let sym = ch.encode_utf8(&mut s);
            buf[(cx, y)].set_symbol(sym).set_style(style);
            for pad in 1..cw {
                if cx + pad < end_x {
                    buf[(cx + pad, y)].set_symbol(" ").set_style(style);
                }
            }
            cx += cw;
        }
    }
}

/// Draw a miniature 5-color swatch strip for `palette` starting at `(x, y)`.
fn draw_swatches(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    palette: &PromptPalette,
    bg: Color,
    ascii: bool,
) {
    if y >= buf.area.bottom() {
        return;
    }
    let block = if ascii { "##" } else { "▬▬" };
    for (idx, rgb) in palette.swatches().iter().enumerate() {
        let sx = x + (idx as u16) * 2;
        if sx + 2 > buf.area.right() {
            break;
        }
        let style = if ascii {
            Style::default().bg(bg)
        } else {
            Style::default().fg(rgb_color(*rgb)).bg(bg)
        };
        buf.set_stringn(sx, y, block, 2, style);
    }
}

/// Draw a realistic dark mock terminal viewport containing prompt scenarios.
fn draw_mock_terminal(
    app: &App,
    buf: &mut Buffer,
    rect: Rect,
    options: &Options,
    show_scene_labels: bool,
) {
    if rect.width < 12 || rect.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;
    let host = app.preview_host();

    let term_bg = if ascii { Color::Reset } else { MOCK_TERM_BG };
    let bar_bg = if ascii { Color::Reset } else { MOCK_BAR_BG };
    let muted_fg = if ascii {
        tokens::color(theme, Role::Text3)
    } else {
        MOCK_MUTED
    };
    let dim_fg = if ascii {
        tokens::color(theme, Role::Text2)
    } else {
        MOCK_DIM_TEXT
    };

    fill_rect(buf, rect, term_bg);

    // Top mock terminal window bar
    let bar_rect = Rect::new(rect.x, rect.y, rect.width, 1);
    fill_rect(buf, bar_rect, bar_bg);
    let shell_label = if options.shell == "both" {
        "bash / zsh"
    } else {
        options.shell.as_str()
    };
    if ascii {
        let header = format!("[ {shell_label} - {host} ]");
        buf.set_stringn(
            rect.x + 1,
            rect.y,
            clip_ascii_safe(&header, rect.width.saturating_sub(2) as usize, true),
            rect.width.saturating_sub(2) as usize,
            Style::default().fg(dim_fg).bg(bar_bg),
        );
    } else {
        let dots = [
            (0x00u16, Color::Rgb(0xf7, 0x76, 0x8e)),
            (2, Color::Rgb(0xe0, 0xaf, 0x68)),
            (4, Color::Rgb(0x9e, 0xce, 0x6a)),
        ];
        for (dx, col) in dots {
            if rect.x + 1 + dx < rect.right() {
                buf.set_stringn(
                    rect.x + 1 + dx,
                    rect.y,
                    "●",
                    1,
                    Style::default().fg(col).bg(bar_bg),
                );
            }
        }
        let header = format!("{shell_label} — {host}");
        let start_x = rect.x + 8;
        if start_x < rect.right() {
            let max_w = rect.right().saturating_sub(start_x + 1) as usize;
            buf.set_stringn(
                start_x,
                rect.y,
                clip_ascii_safe(&header, max_w, false),
                max_w,
                Style::default().fg(dim_fg).bg(bar_bg),
            );
        }
    }

    let inner_x = rect.x + 1;
    let inner_w = rect.width.saturating_sub(2);
    let mut y = rect.y + 1;
    let bottom = rect.bottom();
    if y >= bottom || inner_w < 8 {
        return;
    }

    // Leave breathing room on taller viewports
    if rect.height >= 12 {
        y += 1;
    }

    // Scenario 1: Normal Git workspace
    if show_scene_labels && y < bottom {
        let lbl = format!("# {}", i.text("ui.prompt_scene_normal"));
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&lbl, inner_w as usize, ascii),
            inner_w as usize,
            Style::default().fg(muted_fg).bg(term_bg),
        );
        y += 1;
    }
    for line in terminal::preview_lines(
        options,
        host,
        PreviewScenario::Normal,
        inner_w as usize,
        ascii,
    ) {
        if y >= bottom {
            return;
        }
        draw_preview_line(buf, inner_x, y, inner_w, &line, term_bg, ascii);
        y += 1;
    }
    // Simulated git status output line when space allows
    if rect.height >= 9 && y < bottom {
        let output = if options.git == "off" {
            "README.md  Cargo.toml  src/  tests/"
        } else if ascii {
            "On branch main | 2 staged, 1 modified, 1 ahead"
        } else {
            "On branch main · 2 staged, 1 modified, 1 ahead"
        };
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(output, inner_w as usize, ascii),
            inner_w as usize,
            Style::default().fg(dim_fg).bg(term_bg),
        );
        y += 1;
    }

    // Scenario 2: Error exit code
    if y + 2 <= bottom {
        if rect.height >= 10 && y < bottom {
            y += 1;
        }
        if show_scene_labels && y + 2 <= bottom {
            let lbl = format!("# {}", i.text("ui.prompt_scene_error"));
            buf.set_stringn(
                inner_x,
                y,
                clip_ascii_safe(&lbl, inner_w as usize, ascii),
                inner_w as usize,
                Style::default().fg(muted_fg).bg(term_bg),
            );
            y += 1;
        }
        for line in terminal::preview_lines(
            options,
            host,
            PreviewScenario::Error,
            inner_w as usize,
            ascii,
        ) {
            if y >= bottom {
                return;
            }
            draw_preview_line(buf, inner_x, y, inner_w, &line, term_bg, ascii);
            y += 1;
        }
    }

    // Scenario 3: Root session
    if y + 3 <= bottom {
        y += 1;
        if show_scene_labels && y + 2 <= bottom {
            let lbl = format!("# {}", i.text("ui.prompt_scene_root"));
            buf.set_stringn(
                inner_x,
                y,
                clip_ascii_safe(&lbl, inner_w as usize, ascii),
                inner_w as usize,
                Style::default().fg(muted_fg).bg(term_bg),
            );
            y += 1;
        }
        for line in terminal::preview_lines(
            options,
            host,
            PreviewScenario::Root,
            inner_w as usize,
            ascii,
        ) {
            if y >= bottom {
                return;
            }
            draw_preview_line(buf, inner_x, y, inner_w, &line, term_bg, ascii);
            y += 1;
        }
    }
}

/// Draw scrollable generated `starship.toml` inside `rect`.
fn draw_toml_view(app: &App, buf: &mut Buffer, rect: Rect, options: &Options, scroll: u16) {
    if rect.width < 12 || rect.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let term_bg = if ascii { Color::Reset } else { MOCK_TERM_BG };
    let bar_bg = if ascii { Color::Reset } else { MOCK_BAR_BG };
    fill_rect(buf, rect, term_bg);

    let toml_text = terminal::render(options).unwrap_or_default();
    let all_lines: Vec<&str> = toml_text.lines().collect();
    let body_h = rect.height.saturating_sub(1) as usize;
    let max_scroll = all_lines.len().saturating_sub(body_h);
    let offset = (scroll as usize).min(max_scroll);

    // Header bar with line indicator
    fill_rect(buf, Rect::new(rect.x, rect.y, rect.width, 1), bar_bg);
    let title = format!(
        " starship.toml  ({}-{}/{})",
        (offset + 1).min(all_lines.len()),
        (offset + body_h).min(all_lines.len()),
        all_lines.len()
    );
    buf.set_stringn(
        rect.x + 1,
        rect.y,
        clip_ascii_safe(&title, rect.width.saturating_sub(2) as usize, ascii),
        rect.width.saturating_sub(2) as usize,
        Style::default()
            .fg(if ascii {
                tokens::color(theme, Role::Text2)
            } else {
                MOCK_DIM_TEXT
            })
            .bg(bar_bg),
    );

    let inner_x = rect.x + 1;
    let inner_w = rect.width.saturating_sub(2) as usize;
    for (idx, raw) in all_lines.iter().skip(offset).take(body_h).enumerate() {
        let y = rect.y + 1 + idx as u16;
        let trimmed = raw.trim_start();
        let fg = if ascii {
            tokens::color(theme, Role::Text1)
        } else if trimmed.starts_with('#') {
            MOCK_MUTED
        } else if trimmed.starts_with('[') {
            Color::Rgb(0x7a, 0xa2, 0xf7)
        } else if trimmed.contains('=') {
            Color::Rgb(0xc0, 0xca, 0xf5)
        } else {
            MOCK_DIM_TEXT
        };
        let mut style = Style::default().fg(fg).bg(term_bg);
        if trimmed.starts_with('[') {
            style = style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(raw, inner_w, ascii),
            inner_w,
            style,
        );
    }
}

/// Render the full Interactive Terminal Prompt Preview Studio modal (`terminal.preview`).
pub(super) fn draw_studio_modal(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    preview: &PromptPreview,
    alpha: f64,
) {
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;
    let geom = PromptPreviewGeometry::compute_for(area, i, preview.style_idx, preview.color_idx);
    let options = preview.options();
    let pal = terminal::palette(&options.color);
    let meta = terminal::style_meta(&options.style);

    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    tokens::draw_surface_card(
        frame,
        geom.modal,
        theme,
        Role::Elevated,
        Role::BorderFocus,
        true,
    );
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    let buf = frame.buffer_mut();

    // Title
    let title = format!(" {} ", i.text("ui.prompt_preview_title"));
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        &title,
        geom.modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    if geom.modal.height >= 26 {
        buf.set_stringn(
            geom.modal.x + 2,
            geom.modal.y + 1,
            clip_ascii_safe(
                i.text("ui.prompt_preview_sub"),
                geom.modal.width.saturating_sub(4) as usize,
                ascii,
            ),
            geom.modal.width.saturating_sub(4) as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }

    // Style Selector Header
    let style_hdr_w = if geom.stacked {
        geom.color_header_x.saturating_sub(geom.left_pane.x + 1) as usize
    } else {
        geom.left_pane.width as usize
    };
    if geom.style_header_y < geom.left_pane.bottom() && style_hdr_w > 0 {
        buf.set_stringn(
            geom.left_pane.x,
            geom.style_header_y,
            clip_ascii_safe(i.text("ui.prompt_styles"), style_hdr_w, ascii),
            style_hdr_w,
            tokens::text(theme, Level::Tertiary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    }
    for (idx, rect) in geom.style_rects.iter().enumerate() {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let id = STYLES[idx];
        let active = idx == preview.style_idx;
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
        } else if hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        fill_rect(buf, *rect, row_bg);
        if active {
            buf[(rect.x, rect.y)]
                .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                .set_style(
                    Style::default()
                        .fg(tokens::selected_accent(theme))
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                );
        }
        let name = i.text(&format!("prompt.style.{id}.name"));
        let nf = terminal::style_meta(id).nerd_font;
        let show_nf = nf && rect.width >= 14;
        let badge = if show_nf { "NF" } else { "" };
        let budget = rect.width.saturating_sub(3 + if show_nf { 3 } else { 0 }) as usize;
        let fg = if active {
            tokens::selected_fg(theme)
        } else {
            tokens::color(theme, Role::Text2)
        };
        let mut style = Style::default().fg(fg).bg(row_bg);
        if active {
            style = style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            rect.x + 2,
            rect.y,
            clip_ascii_safe(name, budget, ascii),
            budget,
            style,
        );
        if !badge.is_empty() {
            tokens::put_right(
                buf,
                *rect,
                badge,
                Style::default()
                    .fg(tokens::color(theme, Role::Warning))
                    .bg(row_bg),
            );
        }
    }

    // Color Palette Selector
    if geom.color_header_y < geom.left_pane.bottom() && geom.color_header_w > 0 {
        buf.set_stringn(
            geom.color_header_x,
            geom.color_header_y,
            clip_ascii_safe(
                i.text("ui.prompt_palettes"),
                geom.color_header_w as usize,
                ascii,
            ),
            geom.color_header_w as usize,
            tokens::text(theme, Level::Tertiary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    }
    for (idx, rect) in geom.color_rects.iter().enumerate() {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let id = COLORS[idx];
        let p = terminal::palette(id);
        let active = idx == preview.color_idx;
        let hovered = app.hover.is_some_and(|pt| rect.contains(pt));
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
        } else if hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        fill_rect(buf, *rect, row_bg);
        let name = i.text(&format!("prompt.color.{id}.name"));
        if geom.compact_colors {
            let dot = if ascii { "*" } else { "●" };
            let dot_fg = if ascii {
                tokens::color(theme, Role::Accent)
            } else {
                rgb_color(p.user)
            };
            buf.set_stringn(
                rect.x,
                rect.y,
                dot,
                1,
                Style::default().fg(dot_fg).bg(row_bg),
            );
            let fg = if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text2)
            };
            let mut st = Style::default().fg(fg).bg(row_bg);
            if active {
                st = st.add_modifier(Modifier::BOLD);
            }
            buf.set_stringn(
                rect.x + 2,
                rect.y,
                clip_ascii_safe(name, rect.width.saturating_sub(2) as usize, ascii),
                rect.width.saturating_sub(2) as usize,
                st,
            );
        } else {
            if active {
                buf[(rect.x, rect.y)]
                    .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                    .set_style(
                        Style::default()
                            .fg(tokens::selected_accent(theme))
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD),
                    );
            }
            let sw_w = 10u16;
            let budget = rect.width.saturating_sub(3 + sw_w) as usize;
            let fg = if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text2)
            };
            let mut st = Style::default().fg(fg).bg(row_bg);
            if active {
                st = st.add_modifier(Modifier::BOLD);
            }
            buf.set_stringn(
                rect.x + 2,
                rect.y,
                clip_ascii_safe(name, budget, ascii),
                budget,
                st,
            );
            draw_swatches(
                buf,
                rect.right().saturating_sub(sw_w),
                rect.y,
                p,
                row_bg,
                ascii,
            );
        }
    }

    // Vertical hairline divider between controls (split mode) or style/color columns (stacked mode)
    let div_sym = if ascii { "|" } else { "│" };
    if geom.stacked {
        let div_x = geom.color_header_x.saturating_sub(2);
        for y in geom.left_pane.top()..geom.left_pane.bottom() {
            if div_x > geom.left_pane.left() && div_x < geom.modal.right() {
                buf[(div_x, y)]
                    .set_symbol(div_sym)
                    .set_style(tokens::fg(theme, Role::Rule).bg(bg));
            }
        }
    } else {
        let div_x = geom.left_pane.right() + 1;
        for y in geom.left_pane.top()..geom.left_pane.bottom() {
            if div_x < geom.modal.right() {
                buf[(div_x, y)]
                    .set_symbol(div_sym)
                    .set_style(tokens::fg(theme, Role::Rule).bg(bg));
            }
        }
    }

    // Top Toggle Pills
    let draw_pill = |buf: &mut Buffer, rect: Rect, text: &str, active: bool| {
        if rect.width < 3 {
            return;
        }
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let (fg, pbg) = if active {
            (
                tokens::color(theme, Role::OnAccent),
                tokens::color(theme, Role::Accent),
            )
        } else if hovered {
            (
                tokens::color(theme, Role::Text1),
                tokens::color(theme, Role::Selection),
            )
        } else {
            (
                tokens::color(theme, Role::Text2),
                if theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                },
            )
        };
        fill_rect(buf, rect, pbg);
        let mut st = Style::default().fg(fg).bg(pbg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(text, rect.width as usize, ascii),
            rect.width as usize,
            st,
        );
    };

    let on_off = |enabled: bool| {
        if enabled {
            i.text("ui.style_on")
        } else {
            i.text("ui.style_off")
        }
    };
    let compact_toggles = geom.stacked && geom.modal.width < 52;
    let time_lbl = if compact_toggles {
        format!("[T]{}", on_off(preview.time))
    } else {
        format!(
            " [T] {}:{} ",
            i.text("ui.prompt_time"),
            on_off(preview.time)
        )
    };
    let git_lbl = if compact_toggles {
        format!("[G]{}", on_off(preview.git))
    } else {
        format!(" [G] Git:{} ", on_off(preview.git))
    };
    let shell_lbl = if compact_toggles {
        format!("[S]{}", preview.shell())
    } else {
        format!(" [S] {} ", preview.shell())
    };
    let view_lbl = if compact_toggles {
        if preview.show_toml {
            "[V]TOML".to_owned()
        } else {
            format!("[V]{}", i.text("ui.prompt_view_preview"))
        }
    } else if preview.show_toml {
        format!(" [V] {} ", i.text("ui.prompt_view_toml"))
    } else {
        format!(" [V] {} ", i.text("ui.prompt_view_preview"))
    };
    draw_pill(buf, geom.time_btn, &time_lbl, preview.time);
    draw_pill(buf, geom.git_btn, &git_lbl, preview.git);
    draw_pill(buf, geom.shell_btn, &shell_lbl, false);
    draw_pill(buf, geom.view_btn, &view_lbl, preview.show_toml);

    // Main Viewport: Live Terminal Scenarios OR Generated TOML
    if preview.show_toml {
        draw_toml_view(app, buf, geom.preview_box, &options, preview.toml_scroll);
    } else {
        draw_mock_terminal(
            app,
            buf,
            geom.preview_box,
            &options,
            geom.preview_box.height >= 12,
        );
    }

    // Metadata area under preview box (style description + font badge + swatch bar)
    if geom.meta_area.height >= 1 {
        let desc = i.text(&format!("prompt.style.{}.desc", options.style));
        let wrapped = wrap_words(desc, geom.meta_area.width as usize);
        if geom.meta_area.height == 1 {
            let sw_w = if geom.meta_area.width > 24 {
                12u16
            } else {
                0u16
            };
            let text_w = geom.meta_area.width.saturating_sub(sw_w) as usize;
            if let Some(first) = wrapped.first() {
                buf.set_stringn(
                    geom.meta_area.x,
                    geom.meta_area.y,
                    clip_ascii_safe(first, text_w, ascii),
                    text_w,
                    tokens::text(theme, Level::Secondary).bg(bg),
                );
            }
            if sw_w > 0 {
                draw_swatches(
                    buf,
                    geom.meta_area.right().saturating_sub(10),
                    geom.meta_area.y,
                    pal,
                    bg,
                    ascii,
                );
            }
        } else {
            if let Some(first) = wrapped.first() {
                buf.set_stringn(
                    geom.meta_area.x,
                    geom.meta_area.y,
                    clip_ascii_safe(first, geom.meta_area.width as usize, ascii),
                    geom.meta_area.width as usize,
                    tokens::text(theme, Level::Secondary).bg(bg),
                );
            }
            let font_y = geom.meta_area.y + 1;
            let (font_text, font_role) = if meta.nerd_font {
                (i.text("ui.prompt_nerd_font"), Role::Warning)
            } else {
                (i.text("ui.prompt_unicode_ok"), Role::Success)
            };
            let sw_w = 12u16;
            let text_w = geom.meta_area.width.saturating_sub(sw_w) as usize;
            buf.set_stringn(
                geom.meta_area.x,
                font_y,
                clip_ascii_safe(font_text, text_w, ascii),
                text_w,
                tokens::fg(theme, font_role).bg(bg),
            );
            if geom.meta_area.width > sw_w + 4 {
                draw_swatches(
                    buf,
                    geom.meta_area.right().saturating_sub(10),
                    font_y,
                    pal,
                    bg,
                    ascii,
                );
            }
        }
    }

    // Equivalent CLI command line
    if let Ok(apply_spec) = actions::find("terminal.apply") {
        let words = cmd_preview::single_words(&apply_spec, &preview.parameters());
        let cmd = words
            .iter()
            .map(|w| w.shown().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        buf.set_stringn(
            geom.modal.x + 2,
            geom.cmd_y,
            clip_ascii_safe(&cmd, geom.modal.width.saturating_sub(4) as usize, ascii),
            geom.modal.width.saturating_sub(4) as usize,
            tokens::fg(theme, Role::Accent).bg(bg),
        );
    }

    // Action Buttons: [ Apply Preset ]  [ Close ]
    let can_apply = actions::find("terminal.apply").is_ok_and(|spec| app.available(&spec).is_ok());
    draw_studio_button(
        app,
        buf,
        geom.apply_btn,
        i.text("ui.prompt_apply_btn"),
        can_apply,
        bg,
    );
    draw_studio_button(app, buf, geom.close_btn, i.text("key.close"), false, bg);

    // Bottom Keycap Hints
    let hint_y = geom.modal.bottom().saturating_sub(2);
    if hint_y > geom.apply_btn.y {
        let hint_w = geom.modal.width.saturating_sub(4);
        let hints = tokens::fit_keycaps(
            theme,
            &[
                ("↑↓", i.text("key.prompt_style")),
                ("←→", i.text("key.prompt_color")),
                ("T/G/S", i.text("key.prompt_toggles")),
                ("V", i.text("key.prompt_toml")),
                ("↵", i.text("key.prompt_apply")),
                ("Esc", i.text("key.close")),
            ],
            hint_w as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(geom.modal.x + 2, hint_y, hint_w, 1),
        );
    }

    fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
}

fn draw_studio_button(
    app: &App,
    buf: &mut Buffer,
    rect: Rect,
    label: &str,
    primary: bool,
    base_bg: Color,
) {
    if rect.width < 3 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    let (fg, bg) = if primary {
        (
            tokens::color(theme, Role::OnAccent),
            tokens::color(theme, Role::Accent),
        )
    } else if hovered {
        (
            tokens::color(theme, Role::Text1),
            tokens::color(theme, Role::Selection),
        )
    } else {
        (
            tokens::color(theme, Role::Text2),
            if theme.flags.paint_bg {
                tokens::color(theme, Role::SurfaceAlt)
            } else {
                base_bg
            },
        )
    };
    fill_rect(buf, rect, bg);
    let mut style = Style::default().fg(fg).bg(bg);
    if primary || hovered {
        style = style.add_modifier(Modifier::BOLD);
    }
    buf.set_stringn(
        rect.x + 1,
        rect.y,
        format!(" {label} "),
        rect.width.saturating_sub(1) as usize,
        style,
    );
}

/// Render the embedded Live Terminal Prompt Preview inside the `terminal.apply` form modal.
pub(super) fn draw_form_live_preview(
    app: &App,
    buf: &mut Buffer,
    rect: Rect,
    options: &Options,
    base_bg: Color,
) {
    if rect.width < 16 || rect.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;
    let pal = terminal::palette(&options.color);
    let meta = terminal::style_meta(&options.style);

    if rect.height < 7 {
        // Compact stacked preview strip (80x24): 1-line title + dark prompt preview box
        let header = format!(
            "{} · {} / {}",
            i.text("ui.prompt_live_preview"),
            i.text(&format!("prompt.style.{}.name", options.style)),
            i.text(&format!("prompt.color.{}.name", options.color)),
        );
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&header, rect.width.saturating_sub(11) as usize, ascii),
            rect.width.saturating_sub(11) as usize,
            tokens::text(theme, Level::Tertiary)
                .bg(base_bg)
                .add_modifier(Modifier::BOLD),
        );
        draw_swatches(
            buf,
            rect.right().saturating_sub(10),
            rect.y,
            pal,
            base_bg,
            ascii,
        );
        let box_rect = Rect::new(
            rect.x,
            rect.y + 1,
            rect.width,
            rect.height.saturating_sub(1),
        );
        draw_mock_terminal(app, buf, box_rect, options, false);
        return;
    }

    // Split-pane rich preview (Medium & Wide viewports)
    let header = format!(
        "{} · {}",
        i.text("ui.prompt_live_preview"),
        i.text(&format!("prompt.style.{}.name", options.style)),
    );
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&header, rect.width.saturating_sub(11) as usize, ascii),
        rect.width.saturating_sub(11) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(base_bg)
            .add_modifier(Modifier::BOLD),
    );
    draw_swatches(
        buf,
        rect.right().saturating_sub(10),
        rect.y,
        pal,
        base_bg,
        ascii,
    );

    let footer_h = if rect.height >= 14 { 3u16 } else { 2u16 };
    let term_h = rect.height.saturating_sub(1 + footer_h).max(4);
    let term_rect = Rect::new(rect.x, rect.y + 1, rect.width, term_h);
    draw_mock_terminal(app, buf, term_rect, options, term_h >= 12);

    let meta_y = term_rect.bottom();
    if meta_y < rect.bottom() {
        let desc = i.text(&format!("prompt.style.{}.desc", options.style));
        buf.set_stringn(
            rect.x,
            meta_y,
            clip_ascii_safe(desc, rect.width as usize, ascii),
            rect.width as usize,
            tokens::text(theme, Level::Secondary).bg(base_bg),
        );
    }
    if meta_y + 1 < rect.bottom() {
        let (font_text, font_role) = if meta.nerd_font {
            (i.text("ui.prompt_nerd_font"), Role::Warning)
        } else {
            (i.text("ui.prompt_unicode_ok"), Role::Success)
        };
        buf.set_stringn(
            rect.x,
            meta_y + 1,
            clip_ascii_safe(font_text, rect.width as usize, ascii),
            rect.width as usize,
            tokens::fg(theme, font_role).bg(base_bg),
        );
    }
}

/// Convert a [`PreviewLine`] into a Ratatui [`Line`] for inline display inside the Inspector.
pub(super) fn to_ratatui_line(line: &PreviewLine, theme: &Theme) -> Line<'static> {
    let ascii = theme.flags.ascii;
    let base_bg = if ascii { Color::Reset } else { MOCK_TERM_BG };
    let spans = line
        .spans
        .iter()
        .map(|s| {
            let mut style = Style::default().bg(base_bg);
            if !ascii {
                if let Some(fg) = s.fg {
                    style = style.fg(rgb_color(fg));
                } else {
                    style = style.fg(Color::Rgb(0xd8, 0xde, 0xe9));
                }
                if let Some(bg) = s.bg {
                    style = style.bg(rgb_color(bg));
                }
            }
            if s.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if s.dim {
                style = style.add_modifier(Modifier::DIM);
            }
            Span::styled(s.text.clone(), style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}
