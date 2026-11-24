//! Top-level render compositor and CJK-safe typography helpers.

pub(super) mod logs;
mod modals;
mod overview;
mod sidebar;
mod terminal_preview;
mod theme_drawer;
pub(in crate::tui) mod tokens;
mod topbar;
mod workbench;

use super::{
    fx,
    layout::{self, LayoutRegions},
    state::{App, NavTarget},
};
use crate::theme::{Level, Role};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Styled text segment used by `related.rs` and header/tag builders.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct Segment {
    pub text: String,
    pub style: Style,
}

impl Segment {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

pub(super) fn clip_with(text: &str, width: usize, mark: &str) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let room = width.checked_sub(mark.width());
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > room.unwrap_or(width) {
            break;
        }
        out.push(ch);
        used += cw;
    }
    if room.is_some() {
        out.push_str(mark);
    }
    out
}

pub(in crate::tui) fn clip_ascii_safe(text: &str, width: usize, ascii: bool) -> String {
    clip_with(text, width, if ascii { "..." } else { "…" })
}

pub(in crate::tui) fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push(ch);
        used += cw;
    }
    rows.push(row);
    rows
}

pub(in crate::tui) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for paragraph in text.split('\n') {
        let mut row = String::new();
        for word in paragraph.split(' ') {
            let gap = usize::from(!row.is_empty());
            if row.width() + gap + word.width() <= width {
                if gap == 1 {
                    row.push(' ');
                }
                row.push_str(word);
                continue;
            }
            let breakable = word.chars().any(|c| c.width().unwrap_or(0) > 1);
            let first = word.chars().next().map_or(0, |c| c.width().unwrap_or(0));
            if breakable && row.width() + gap + first <= width {
                if gap == 1 {
                    row.push(' ');
                }
            } else if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            for ch in word.chars() {
                if row.width() + ch.width().unwrap_or(0) > width && !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                row.push(ch);
            }
        }
        rows.push(row);
    }
    rows
}

/// Split `text` into highlighted and base spans using sorted byte ranges on UTF-8 boundaries.
pub(super) fn highlight_spans(
    text: &str,
    ranges: &[Range<usize>],
    base: Style,
    highlight: Style,
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let mut spans = Vec::new();
    let mut pos = 0;
    for range in ranges {
        if range.start > pos && range.start <= text.len() {
            spans.push(Span::styled(text[pos..range.start].to_owned(), base));
        }
        let end = range.end.min(text.len());
        if range.start < end {
            spans.push(Span::styled(text[range.start..end].to_owned(), highlight));
        }
        pos = end;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_owned(), base));
    }
    spans
}

pub(super) fn draw(app: &App, frame: &mut Frame) {
    app.selectable_panes.borrow_mut().clear();
    let area = frame.area();
    let theme = &app.theme.active;

    // Layer 0: Canvas background
    tokens::fill_surface(frame.buffer_mut(), area, theme, Role::Canvas);

    if !layout::fits(area) {
        draw_too_small(app, frame, area);
        return;
    }

    let regions = LayoutRegions::compute(area, app);

    // Top status strip
    topbar::draw(app, frame, &regions);

    // Docked sidebar (non-overlay)
    if let Some(side) = &regions.sidebar
        && !side.overlay
    {
        sidebar::draw(app, frame, side);
    }

    // Primary workspace
    if let Some(ov) = &regions.overview {
        overview::draw(app, frame, ov);
    } else if let Some(dk) = &regions.docker {
        super::docker::render::draw(app, frame, dk);
    } else if let Some(wb) = &regions.workbench {
        workbench::draw(app, frame, wb);
    }

    // Gentle view cross-fade transition on navigation change
    let view_age = app.now.saturating_duration_since(app.view_changed);
    let view_alpha = fx::view_opacity(view_age, app.reduced_motion());
    if view_alpha < 0.995 {
        fx::apply_fade(
            frame.buffer_mut(),
            regions.workspace,
            theme.palette.canvas,
            view_alpha,
        );
    }

    // Right-docked Appearance & Theme Drawer (with smooth reveal animation)
    if let (Some(drawer_geom), Some(drawer)) = (&regions.theme_drawer, &app.theme_drawer) {
        theme_drawer::draw(app, frame, drawer_geom);
        let age = app.now.saturating_duration_since(drawer.opened);
        let alpha = fx::drawer_progress(age, app.reduced_motion());
        if alpha < 0.995 {
            fx::apply_fade(
                frame.buffer_mut(),
                drawer_geom.area,
                theme.palette.canvas,
                alpha,
            );
        }
    }

    // Optional bottom execution log drawer
    if let Some(logs_rect) = regions.logs {
        logs::draw_drawer(
            app,
            frame,
            logs_rect,
            regions.log_body,
            regions.log_copy_btn,
            regions.log_plain_btn,
            regions.log_expand_btn,
        );
    }

    // Context-aware Footer + Toast pill
    draw_footer(app, frame, &regions);

    // Overlay layers (with Backdrop Scrim + Shadow + Reveal Fade)
    let has_overlay = app.overlay_kind() != 0;
    if has_overlay {
        // Loading dialog stays invisible during its first 150ms to avoid flicker
        let skip_scrim = app.loading.as_ref().is_some_and(|l| {
            app.confirmation.is_none()
                && app.now.saturating_duration_since(l.started) < fx::LOADING_DELAY
        });
        if !skip_scrim {
            fx::apply_scrim(frame.buffer_mut(), area, theme);
        }
    }

    let modal_alpha = app
        .dialog_opened
        .map(|opened| {
            fx::dialog_opacity(
                app.now.saturating_duration_since(opened),
                app.reduced_motion(),
            )
        })
        .unwrap_or(1.0);

    if app.sidebar_overlay
        && let Some(side) = &regions.sidebar
        && side.overlay
    {
        fx::apply_shadow(frame.buffer_mut(), side.area, area, theme);
        sidebar::draw(app, frame, side);
        fx::apply_fade(
            frame.buffer_mut(),
            side.area,
            theme.palette.canvas,
            modal_alpha,
        );
    }

    // Render topmost active modal matching the exact priority order in `tui/events.rs`:
    // Confirmation -> Loading -> Picker/Form -> PromptPreview -> DockerModal ->
    // Notice -> Help -> FullLogs -> CommandPalette.
    if app.confirmation.is_some() {
        modals::draw_confirmation(app, frame, area, modal_alpha);
    } else if let Some(loading) = &app.loading {
        if app.now.saturating_duration_since(loading.started) >= fx::LOADING_DELAY {
            modals::draw_loading(app, frame, area, loading, modal_alpha);
        }
    } else if let Some(form) = &app.form {
        if form.picker.is_some() {
            modals::draw_picker(app, frame, area, form, modal_alpha);
        } else {
            modals::draw_form(app, frame, area, form, modal_alpha);
        }
    } else if let Some(preview) = &app.prompt_preview {
        terminal_preview::draw_studio_modal(app, frame, area, preview, modal_alpha);
    } else if app.docker_has_modal() {
        super::docker::render::modals::draw(app, frame, area, modal_alpha);
    } else if app.notice.is_some() {
        modals::draw_notice(app, frame, area, modal_alpha);
    } else if app.help {
        modals::draw_help(app, frame, area, modal_alpha);
    } else if app.log_modal {
        logs::draw_modal(app, frame, area, modal_alpha);
    } else if app.command_palette.is_some() {
        modals::draw_palette(app, frame, area, modal_alpha);
    }
}

fn draw_too_small(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme.active;
    let msg = app.ctx.i18n.text("ui.small");
    let size = format!("{} × {}", area.width, area.height);
    let inner_w = area.width.saturating_sub(2).max(1) as usize;
    let mut lines: Vec<Line<'static>> = wrap_words(msg, inner_w)
        .into_iter()
        .map(|row| {
            Line::from(Span::styled(
                row,
                tokens::fg(theme, Role::Warning).add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    lines.push(Line::from(Span::styled(
        size,
        tokens::text(theme, Level::Secondary),
    )));
    let h = (lines.len() as u16).min(area.height);
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(
        area.x + u16::from(area.width > 2),
        y,
        area.width
            .saturating_sub(if area.width > 2 { 2 } else { 0 }),
        h,
    );
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

fn draw_footer(app: &App, frame: &mut Frame, regions: &LayoutRegions) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let bar = regions.footer;
    if bar.height == 0 || bar.width == 0 {
        return;
    }
    tokens::fill_surface(frame.buffer_mut(), bar, theme, Role::Sidebar);
    let mobile = bar.width < 80;

    let pairs: Vec<(&str, &str)> = if app.worker.is_some() {
        vec![
            ("Esc", i.text("key.cancel_task")),
            ("G", i.text("key.logs")),
            ("F2", i.text("key.full_log")),
            ("y", i.text("key.copy_logs")),
            ("V", i.text("key.plain_view")),
            ("Q", i.text("key.quit")),
        ]
    } else if app.theme_drawer.is_some() {
        vec![
            ("←→/Tab", i.text("key.switch_tab")),
            ("↑↓", i.text("key.preview")),
            ("B/M", i.text("key.toggle_opt")),
            ("↵", i.text("key.keep_theme")),
            ("Esc", i.text("key.restore_theme")),
        ]
    } else if app.nav == NavTarget::Overview {
        if mobile {
            let move_key = if regions.overview.as_ref().is_some_and(|o| o.is_list_mode()) {
                "↑↓"
            } else {
                "↑↓←→"
            };
            vec![
                (move_key, i.text("key.move")),
                ("↵", i.text("key.open")),
                ("[", i.text("nav.toggle")),
                ("/", i.text("key.search")),
                ("O", i.text("group.optimization")),
                ("N", i.text("key.public")),
                ("T", i.text("nav.theme")),
                ("?", i.text("key.help")),
            ]
        } else {
            vec![
                ("↑↓←→", i.text("key.move")),
                ("↵", i.text("key.open")),
                ("/", i.text("key.search")),
                ("O", i.text("group.optimization")),
                ("N", i.text("key.public")),
                ("T", i.text("nav.theme")),
                ("?", i.text("key.help")),
            ]
        }
    } else if app.is_docker_view() {
        super::docker::render::footer_pairs(app)
    } else if app.optimization() {
        if mobile {
            vec![
                ("Space", i.text("key.select")),
                ("↵", i.text("key.configure")),
                ("X", i.text("key.run_checklist")),
                ("[", i.text("nav.toggle")),
                ("/", i.text("key.search")),
                ("?", i.text("key.help")),
            ]
        } else {
            vec![
                ("Space", i.text("key.select")),
                ("↵", i.text("key.configure")),
                ("X", i.text("key.run_checklist")),
                ("c", i.text("key.copy")),
                ("A", i.text("key.recommended")),
                ("/", i.text("key.search")),
                ("?", i.text("key.help")),
            ]
        }
    } else if app
        .active_group()
        .is_some_and(|g| app.group_sections(g).is_some())
    {
        if mobile {
            vec![
                ("↑↓", i.text("key.move")),
                ("←→", i.text("key.section")),
                ("↵", i.text("key.open")),
                ("[", i.text("nav.toggle")),
                ("/", i.text("key.search")),
                ("?", i.text("key.help")),
            ]
        } else {
            vec![
                ("↑↓", i.text("key.move")),
                ("←→", i.text("key.section")),
                ("↵", i.text("key.open")),
                ("c", i.text("key.copy")),
                ("V", i.text("key.plain_view")),
                ("/", i.text("key.search")),
                ("T", i.text("nav.theme")),
                ("?", i.text("key.help")),
            ]
        }
    } else if mobile {
        vec![
            ("↑↓", i.text("key.move")),
            ("↵", i.text("key.open")),
            ("[", i.text("nav.toggle")),
            ("/", i.text("key.search")),
            ("?", i.text("key.help")),
        ]
    } else {
        vec![
            ("↑↓", i.text("key.move")),
            ("↵", i.text("key.open")),
            ("c", i.text("key.copy")),
            ("V", i.text("key.plain_view")),
            ("/", i.text("key.search")),
            ("T", i.text("nav.theme")),
            ("?", i.text("key.help")),
        ]
    };

    // Keep whole (key, label) pairs so narrow terminals never clip mid-shortcut.
    let mut kept: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used = 1usize;
    let max_w = bar.width.saturating_sub(1) as usize;
    for (k, lbl) in pairs {
        let group = tokens::keycap(theme, k, lbl);
        let gw: usize = group.iter().map(Span::width).sum();
        if used + gw > max_w {
            break;
        }
        used += gw;
        kept.extend(group);
    }
    frame.render_widget(Paragraph::new(Line::from(kept)), bar);

    // Floating Toast Pill aligned cleanly on the right of the footer
    if let Some(toast) = &app.toast {
        let age = app.now.saturating_duration_since(toast.shown);
        let opacity = fx::toast_opacity(age);
        if opacity > 0.05 {
            let max_toast_w = if bar.width < 60 {
                bar.width.saturating_sub(2).min(42) as usize
            } else {
                (bar.width / 2).clamp(24, 56) as usize
            };
            let icon = match toast.tone {
                super::state::Tone::Success => tokens::glyph(theme, tokens::Glyph::Success),
                super::state::Tone::Warning => tokens::glyph(theme, tokens::Glyph::Warning),
                super::state::Tone::Danger => tokens::glyph(theme, tokens::Glyph::Failure),
                _ => tokens::glyph(theme, tokens::Glyph::DotOn),
            };
            let content = clip_ascii_safe(
                &format!(" {icon} {} ", toast.text),
                max_toast_w,
                theme.flags.ascii,
            );
            let w = content.width() as u16;
            let x = bar.right().saturating_sub(w + 1);
            let pill_rect = Rect::new(x, bar.y, w, 1);
            let fg_col = tokens::color(theme, tokens::tone_role(toast.tone));
            let bg_col = if theme.flags.paint_bg {
                tokens::color(theme, Role::Elevated)
            } else {
                ratatui::style::Color::Reset
            };
            let blended_fg = fx::blend_ratatui(tokens::color(theme, Role::Canvas), fg_col, opacity);
            let style = Style::default()
                .fg(blended_fg)
                .bg(bg_col)
                .add_modifier(Modifier::BOLD);
            frame
                .buffer_mut()
                .set_stringn(pill_rect.x, pill_rect.y, &content, w as usize, style);
        }
    }
}
