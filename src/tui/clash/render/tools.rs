//! Mihomo `Tools` tab renderer:
//! - Deduplicated grouped management entries (Core, SOCKS5 + Shift+S secret card, TUN/DNS/Mixin,
//!   Controller/Secret, GeoData/Panel/systemd) with live status & command inspector
//! - Embedded redacted `mihomo.log` viewer (scrollable, searchable, fullscreen-capable)

use super::{draw_card_title, draw_inline_filter, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    actions,
    theme::{Level, Role},
    tui::{
        clash::{
            layout::ToolsGeom,
            state::{ToolEntry, tool_entry_title},
        },
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
            wrap_words,
        },
        state::App,
    },
};
use ratatui::{Frame, layout::Rect, style::Modifier};

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &ToolsGeom) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let bg = surface_bg(theme, Role::Surface);

    // 1. Toolbar (`/` filter + `[l]` toggle embedded redacted logs)
    if geom.toolbar.height > 0 {
        let buf = frame.buffer_mut();
        let placeholder = if cl.tools_log_mode {
            i.text("clash.filter.logs")
        } else {
            i.text("clash.filter.tools")
        };
        draw_inline_filter(
            buf,
            geom.filter_bar,
            &cl.filter,
            cl.filtering,
            placeholder,
            theme,
            bg,
        );
        if let Some(btn) = geom.logs_btn {
            let lbl = format!(
                " [l] {} ",
                i.text(if cl.tools_log_mode {
                    "clash.tools.back_list"
                } else {
                    "clash.tools.view_logs"
                })
            );
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, cl.tools_log_mode, hov, theme, bg);
        }
    }

    // 2. Embedded redacted logs mode
    if cl.tools_log_mode {
        draw_logs_panel(app, frame, geom.inspector_area, false);
        return;
    }

    // 3. Left grouped management list
    if geom.list_area.width >= 12 && geom.list_area.height >= 3 {
        tokens::draw_surface_card_fx(
            frame,
            geom.list_area,
            theme,
            Role::SurfaceAlt,
            Role::Border,
            true,
            app.now.saturating_duration_since(app.started),
            app.reduced_motion(),
        );
        let lbg = surface_bg(theme, Role::SurfaceAlt);
        let buf = frame.buffer_mut();
        let tools = cl.filtered_tools(i);

        if tools.is_empty() {
            buf.set_stringn(
                geom.list_inner.x,
                geom.list_inner.y,
                clip_ascii_safe(
                    i.text("clash.tools.empty"),
                    geom.list_inner.width as usize,
                    ascii,
                ),
                geom.list_inner.width as usize,
                tokens::text(theme, Level::Tertiary).bg(lbg),
            );
        } else {
            let active_bar = tokens::glyph(theme, Glyph::ActiveBar);
            let vis = geom.list_inner.height as usize;
            for slot in 0..vis {
                let idx = geom.list_scroll + slot;
                let Some(item) = tools.get(idx) else {
                    break;
                };
                let y = geom.list_inner.y + slot as u16;
                let row_rect = Rect::new(geom.list_inner.x, y, geom.list_inner.width, 1);
                let is_cur = idx == cl.tool_cursor;
                let is_hov = app.hover.is_some_and(|p| row_rect.contains(p));
                let rbg = if is_cur {
                    tokens::color(theme, Role::Selection)
                } else if is_hov {
                    tokens::color(theme, Role::Surface)
                } else {
                    lbg
                };
                fill_rect_bg(buf, row_rect, rbg);
                if is_cur {
                    buf.set_stringn(
                        row_rect.x,
                        y,
                        active_bar,
                        1,
                        tokens::fg(theme, Role::Accent).bg(rbg),
                    );
                }

                let title = tool_entry_title(item.entry, i);
                let sec_tag = i.text(&format!("section.clash.{}", item.section));
                let tag_w = 10usize.min(row_rect.width as usize / 3);
                let title_w = (row_rect.width as usize).saturating_sub(tag_w + 3);
                buf.set_stringn(
                    row_rect.x + 2,
                    y,
                    clip_ascii_safe(&title, title_w, ascii),
                    title_w,
                    tokens::text(theme, Level::Primary)
                        .bg(rbg)
                        .add_modifier(if is_cur {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
                if tag_w >= 4 {
                    tokens::put_right(
                        buf,
                        row_rect,
                        &clip_ascii_safe(sec_tag, tag_w, ascii),
                        tokens::text(theme, Level::Tertiary).bg(rbg),
                    );
                }
            }
        }
    }

    // 4. Right tool details & live configuration preview
    if geom.inspector_area.width >= 16 && geom.inspector_area.height >= 4 {
        draw_tool_inspector(app, frame, geom.inspector_area, geom.inspector_body);
    }
}

fn draw_tool_inspector(app: &App, frame: &mut Frame, area: Rect, body: Rect) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    tokens::draw_surface_card_fx(
        frame,
        area,
        theme,
        Role::SurfaceAlt,
        Role::Border,
        false,
        app.now.saturating_duration_since(app.started),
        app.reduced_motion(),
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();
    draw_card_title(
        buf,
        area,
        i.text("clash.tools.detail_title"),
        &format!("[Enter] {}", i.text("key.open")),
        theme,
        Role::SurfaceAlt,
    );

    let tools = cl.filtered_tools(i);
    let Some(item) = tools.get(cl.tool_cursor) else {
        return;
    };

    let iw = body.width as usize;
    let mut y = body.y;
    let max_y = body.bottom();
    let title = tool_entry_title(item.entry, i);
    buf.set_stringn(
        body.x,
        y,
        clip_ascii_safe(&title, iw, ascii),
        iw,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;

    let action_desc = |id: &str| {
        actions::find_ref(id)
            .map(|s| i.text(&s.description()).to_owned())
            .unwrap_or_default()
    };
    let (cmd_preview, desc) = match item.entry {
        ToolEntry::ServiceStart => ("cqs run clash.start".to_owned(), action_desc("clash.start")),
        ToolEntry::ServiceStop => ("cqs run clash.stop".to_owned(), action_desc("clash.stop")),
        ToolEntry::ServiceReload => (
            "cqs run clash.reload".to_owned(),
            action_desc("clash.reload"),
        ),
        ToolEntry::OutboundIp => (
            "[i] / [N]".to_owned(),
            i.text("clash.tools.outbound_ip_desc").to_owned(),
        ),
        ToolEntry::ServiceStatus => (
            "[c]".to_owned(),
            i.text("clash.tools.service_status_desc").to_owned(),
        ),
        ToolEntry::SocksSecretCard => (
            "[Shift+S]".to_owned(),
            i.text("clash.tools.socks_secret_desc").to_owned(),
        ),
        ToolEntry::ViewLogs => (
            "cqs run clash.logs".to_owned(),
            i.text("clash.tools.logs_desc").to_owned(),
        ),
        ToolEntry::CqsAction(id) => (format!("cqs run {id}"), action_desc(id)),
    };

    if y < max_y {
        buf.set_stringn(
            body.x,
            y,
            clip_ascii_safe(&cmd_preview, iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        y += 2;
    }

    for line in wrap_words(&desc, iw.max(8)) {
        if y >= max_y {
            break;
        }
        buf.set_stringn(
            body.x,
            y,
            clip_ascii_safe(&line, iw, ascii),
            iw,
            tokens::text(theme, Level::Secondary).bg(bg),
        );
        y += 1;
    }

    // Live environment snapshot summary at bottom of inspector
    let sum = &cl.snapshot.summary;
    y = y.saturating_add(1);
    if y + 3 <= max_y {
        let mut live_rows = vec![
            (
                i.text("clash.overview.status"),
                if cl.snapshot.availability.is_ready() {
                    i.text("clash.status.running").to_owned()
                } else {
                    i.text("clash.status.stopped").to_owned()
                },
            ),
            (
                "SOCKS5",
                sum.socks5_listen
                    .clone()
                    .unwrap_or_else(|| i.text("clash.overview.socks_off").to_owned()),
            ),
            (
                "TUN",
                if sum.tun_enabled {
                    format!("ON ({})", sum.tun_stack)
                } else {
                    "OFF".to_owned()
                },
            ),
        ];
        if let Some(ctrl) = &sum.controller_url {
            live_rows.push(("API", ctrl.clone()));
        }
        let webui_val = if sum.panel_installed {
            let title = match (&sum.panel_name, &sum.panel_version) {
                (Some(n), Some(v)) if !v.is_empty() => format!("{n} {v}"),
                (Some(n), _) => n.clone(),
                _ => "ON".to_owned(),
            };
            match &sum.panel_url {
                Some(url) => format!("{title} ({url})"),
                None => title,
            }
        } else {
            "OFF".to_owned()
        };
        live_rows.push(("WebUI", webui_val));
        for (k, v) in live_rows {
            if y >= max_y {
                break;
            }
            let line = format!("{k}: {v}");
            buf.set_stringn(
                body.x,
                y,
                clip_ascii_safe(&line, iw, ascii),
                iw,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
            y += 1;
        }
    }
}

pub(super) fn draw_logs_panel(app: &App, frame: &mut Frame, area: Rect, fullscreen: bool) {
    if area.width < 12 || area.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    tokens::draw_surface_card_fx(
        frame,
        area,
        theme,
        Role::SurfaceAlt,
        if fullscreen {
            Role::BorderFocus
        } else {
            Role::Border
        },
        false,
        app.now.saturating_duration_since(app.started),
        app.reduced_motion(),
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();
    let hint = if fullscreen {
        format!(
            "[/] {}  [y] {}  [r] {}  [Esc] {}",
            i.text("key.search"),
            i.text("clash.key.copy"),
            i.text("clash.key.refresh"),
            i.text("key.close")
        )
    } else {
        format!(
            "[y] {}  [r] {}  [f] {}  [l] {}",
            i.text("clash.key.copy"),
            i.text("clash.key.refresh"),
            i.text("clash.key.fullscreen"),
            i.text("clash.tools.back_list")
        )
    };
    draw_card_title(
        buf,
        area,
        &format!("{} ({})", i.text("clash.tools.logs_title"), cl.logs.len()),
        &hint,
        theme,
        Role::SurfaceAlt,
    );

    let mut body = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    if body.width == 0 || body.height == 0 {
        return;
    }

    if fullscreen && (cl.filtering || !cl.filter.text.is_empty()) && body.height >= 2 {
        draw_inline_filter(
            buf,
            Rect::new(body.x, body.y, body.width, 1),
            &cl.filter,
            cl.filtering,
            i.text("clash.filter.logs"),
            theme,
            bg,
        );
        body = Rect::new(body.x, body.y + 1, body.width, body.height - 1);
    }

    if cl.log_loading {
        let msg = format!("{} {}", app.spin(), i.text("clash.avail.loading_title"));
        buf.set_stringn(
            body.x,
            body.y,
            clip_ascii_safe(&msg, body.width as usize, ascii),
            body.width as usize,
            tokens::fg(theme, Role::Accent).bg(bg),
        );
        return;
    }
    if let Some(err) = &cl.log_error {
        buf.set_stringn(
            body.x,
            body.y,
            clip_ascii_safe(err, body.width as usize, ascii),
            body.width as usize,
            tokens::fg(theme, Role::Danger).bg(bg),
        );
        return;
    }
    let lines = cl.filtered_logs();
    if lines.is_empty() {
        buf.set_stringn(
            body.x,
            body.y,
            clip_ascii_safe(i.text("clash.tools.logs_empty"), body.width as usize, ascii),
            body.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    let vis = body.height as usize;
    let max_s = lines.len().saturating_sub(vis) as u16;
    cl.max_log_scroll.set(max_s);
    let scroll = (cl.log_scroll.min(max_s)) as usize;

    for (slot, line) in lines.iter().skip(scroll).take(vis).enumerate() {
        let y = body.y + slot as u16;
        let lower = line.to_ascii_lowercase();
        let style = if lower.contains("level=error") || lower.contains(" err ") {
            tokens::fg(theme, Role::Danger).bg(bg)
        } else if lower.contains("level=warning") || lower.contains(" warn ") {
            tokens::fg(theme, Role::Warning).bg(bg)
        } else {
            tokens::text(theme, Level::Secondary).bg(bg)
        };
        buf.set_stringn(
            body.x,
            y,
            clip_ascii_safe(line, body.width as usize, ascii),
            body.width as usize,
            style,
        );
    }
}
