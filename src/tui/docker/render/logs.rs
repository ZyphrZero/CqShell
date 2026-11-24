//! Live streaming Docker log viewer (both embedded inspector sub-tab and fullscreen viewport).

use super::{fill_rect_bg, surface_bg};
use crate::{
    ops::docker::model::{DockerTab, LogSeverity, MAX_DOCKER_LOG_LINES},
    theme::{Level, Role},
    tui::{
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
            wrap_cells,
        },
        state::App,
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw_logs_panel(app: &App, frame: &mut Frame, area: Rect, fullscreen: bool) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let logs = &app.docker.logs;

    let bg = if fullscreen {
        tokens::draw_surface_card(frame, area, theme, Role::Elevated, Role::BorderFocus, true);
        surface_bg(theme, Role::Elevated)
    } else {
        surface_bg(theme, Role::SurfaceAlt)
    };

    let (header_rect, body_rect) = if fullscreen {
        let h = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), 1);
        let b = Rect::new(
            area.x + 2,
            area.y + 3,
            area.width.saturating_sub(4),
            area.height.saturating_sub(4),
        );
        (h, b)
    } else {
        let h = Rect::new(area.x, area.y, area.width, 1);
        let b = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        (h, b)
    };

    let buf = frame.buffer_mut();
    let bar_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        bg
    };
    fill_rect_bg(buf, header_rect, bar_bg);

    // Header: Target name + Follow status + Timestamps + Search + line counter
    let target_name = match app.docker.tab {
        DockerTab::Containers => app
            .docker
            .selected_container()
            .map(|c| c.name.as_str())
            .unwrap_or("-"),
        DockerTab::Compose => app
            .docker
            .selected_compose()
            .map(|p| p.name.as_str())
            .unwrap_or("-"),
        _ => "-",
    };
    let follow_lbl = if logs.follow {
        i.text("docker.badge.follow_on")
    } else {
        i.text("docker.badge.follow_off")
    };
    let search_part = if logs.searching {
        let (shown, _) = logs.search.visible(24);
        format!("  /{shown}_")
    } else if !logs.search.text.is_empty() {
        format!("  /{}", logs.search.text)
    } else {
        String::new()
    };
    let sep = tokens::glyph(theme, Glyph::Separator);
    let left_hdr = format!(
        " {target_name} {sep} [f] {follow_lbl} {sep} [t] {}:{}{search_part}",
        i.text("docker.key.timestamps"),
        if logs.timestamps { "on" } else { "off" }
    );
    let count_str = format!(" {}/{} ", logs.lines.len(), MAX_DOCKER_LOG_LINES);
    let left_budget = header_rect
        .width
        .saturating_sub(count_str.width() as u16 + 1) as usize;
    buf.set_stringn(
        header_rect.x,
        header_rect.y,
        clip_ascii_safe(&left_hdr, left_budget, ascii),
        left_budget,
        Style::default()
            .fg(tokens::color(
                theme,
                if logs.follow {
                    Role::Success
                } else {
                    Role::Warning
                },
            ))
            .bg(bar_bg)
            .add_modifier(Modifier::BOLD),
    );
    tokens::put_right(
        buf,
        header_rect,
        &count_str,
        tokens::text(theme, Level::Tertiary).bg(bar_bg),
    );

    if fullscreen && area.height >= 4 {
        tokens::draw_rule(
            buf,
            area.x + 2,
            area.y + 2,
            area.width.saturating_sub(4),
            theme,
            bg,
        );
    }

    if body_rect.height == 0 || body_rect.width < 8 {
        return;
    }
    app.docker
        .last_logs_viewport
        .set((body_rect.width as usize, body_rect.height as usize));

    let entries = logs.filtered_lines();
    if entries.is_empty() {
        buf.set_stringn(
            body_rect.x,
            body_rect.y,
            clip_ascii_safe(
                i.text("docker.empty.no_logs"),
                body_rect.width as usize,
                ascii,
            ),
            body_rect.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    // Flatten visual lines (respecting soft-wrap toggle `w`)
    let w = body_rect.width as usize;
    let mut rendered: Vec<(Option<&str>, String, LogSeverity)> = Vec::new();
    for entry in entries {
        let ts = if logs.timestamps {
            entry.timestamp.as_deref()
        } else {
            None
        };
        if logs.wrap {
            let prefix_w = ts.map_or(0, |t| t.width() + 2);
            let msg_w = w.saturating_sub(prefix_w).max(8);
            for (i_part, chunk) in wrap_cells(&entry.message, msg_w).into_iter().enumerate() {
                rendered.push((if i_part == 0 { ts } else { None }, chunk, entry.severity));
            }
        } else {
            rendered.push((ts, entry.message.clone(), entry.severity));
        }
    }

    let visible_h = body_rect.height as usize;
    let total = rendered.len();
    let max_scroll = total.saturating_sub(visible_h);
    let from_bottom = logs.scroll.min(max_scroll);
    let end = total.saturating_sub(from_bottom);
    let start = end.saturating_sub(visible_h);

    for (row_idx, (ts, msg, sev)) in rendered[start..end].iter().enumerate() {
        let y = body_rect.y + row_idx as u16;
        let mut x = body_rect.x;
        let mut remain = w;

        if let Some(ts_text) = ts {
            let ts_str = format!("{ts_text}  ");
            let tw = ts_str.width().min(remain);
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(&ts_str, tw, ascii),
                tw,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
            x = x.saturating_add(tw as u16);
            remain = remain.saturating_sub(tw);
        }

        let role = match sev {
            LogSeverity::Error => Role::Danger,
            LogSeverity::Warn => Role::Warning,
            LogSeverity::Info => Role::Info,
            LogSeverity::Debug => Role::Text3,
            LogSeverity::Plain => Role::Text1,
        };
        if remain > 0 {
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(msg, remain, ascii),
                remain,
                tokens::fg(theme, role).bg(bg),
            );
        }
    }
}
