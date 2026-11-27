//! FlClash-style Requests / Connections tab renderer:
//! - Toolbar (`/` filter, `[a]` Running/Closed/All, `[o]` sort, `[D]` close all)
//! - Facet filter pill strip (`All`, `TCP`, `UDP`, `Domain`, `DomainSuffix`, `GeoIP`, ...)
//! - Live table with per-connection download/upload speed, total bytes, and exit pill
//! - Split or fullscreen inspector with full routing chain, rule payload, and `[x]` close action

use super::{draw_card_title, draw_inline_filter, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    ops::clash::model::{ConnectionItem, format_bytes, format_bytes_compact, format_speed_compact},
    theme::{Level, Role},
    tui::{
        clash::layout::ConnectionsGeom,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
            wrap_words,
        },
        state::App,
    },
};
use ratatui::{Frame, layout::Rect, style::Modifier};

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &ConnectionsGeom) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let bg = surface_bg(theme, Role::Surface);

    // 1. Toolbar
    if geom.toolbar.height > 0 {
        let buf = frame.buffer_mut();
        draw_inline_filter(
            buf,
            geom.filter_bar,
            &cl.filter,
            cl.filtering,
            i.text("clash.filter.connections"),
            theme,
            bg,
        );
        if let Some(chip) = geom.scope_chip {
            let lbl = format!(" [a] {} ", i.text(cl.conn_scope.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, true, hov, theme, bg);
        }
        if let Some(chip) = geom.sort_chip {
            let arrow = if cl.conn_sort_asc {
                if ascii { "^" } else { "▴" }
            } else if ascii {
                "v"
            } else {
                "▾"
            };
            let lbl = format!(" [o] {}{arrow} ", i.text(cl.conn_sort.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
        }
        if let Some(btn) = geom.close_all_btn {
            let lbl = format!(" [X] {} ", i.text("clash.key.close_all"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }

    // 2. Facet pill strip (TCP / UDP / Domain / DomainSuffix / GeoIP / ...)
    if geom.facet_bar.height > 0 {
        let buf = frame.buffer_mut();
        for (rect, facet) in &geom.facet_chips {
            let active = cl.conn_facet == *facet;
            let hov = app.hover.is_some_and(|p| rect.contains(p));
            let lbl = match facet {
                None => format!(" {} ", i.text("clash.filter.all")),
                Some(f) => format!(" {f} "),
            };
            draw_pill_chip(buf, *rect, &lbl, active, hov, theme, bg);
        }
    }

    // 3. Connections table
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

        let conns = cl.filtered_connections();
        let w = geom.table_header.width as usize;
        let show_proc = w >= 66;
        let show_speeds = w >= 54;
        let proc_w = if show_proc { 13 } else { 0 };
        let exit_w = if w >= 42 { 13 } else { 8 };
        let spd_w = if show_speeds { 9 } else { 0 };
        let tot_w = 8usize;
        let host_w = w.saturating_sub(
            proc_w
                + usize::from(show_proc)
                + exit_w
                + 1
                + if show_speeds { spd_w * 2 + 2 } else { 0 }
                + tot_w
                + 2,
        );

        // Header row
        if geom.table_header.height > 0 {
            let mut x = geom.table_header.x + 1;
            let y = geom.table_header.y;
            let hdr_style = tokens::text(theme, Level::Tertiary)
                .bg(lbg)
                .add_modifier(Modifier::BOLD);
            if show_proc {
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(i.text("clash.conn.col_process"), proc_w, ascii),
                    proc_w,
                    hdr_style,
                );
                x += proc_w as u16 + 1;
            }
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(i.text("clash.conn.col_target"), host_w, ascii),
                host_w,
                hdr_style,
            );
            x += host_w as u16 + 1;
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(i.text("clash.conn.col_exit"), exit_w, ascii),
                exit_w,
                hdr_style,
            );
            x += exit_w as u16 + 1;
            if show_speeds {
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(i.text("clash.conn.col_down"), spd_w, ascii),
                    spd_w,
                    hdr_style,
                );
                x += spd_w as u16 + 1;
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(i.text("clash.conn.col_up"), spd_w, ascii),
                    spd_w,
                    hdr_style,
                );
            }
            tokens::put_right(
                buf,
                geom.table_header,
                i.text("clash.conn.col_total"),
                hdr_style,
            );
        }

        if conns.is_empty() {
            if geom.list_inner.height > 0 {
                buf.set_stringn(
                    geom.list_inner.x + 1,
                    geom.list_inner.y,
                    clip_ascii_safe(
                        i.text("clash.conn.empty"),
                        geom.list_inner.width.saturating_sub(2) as usize,
                        ascii,
                    ),
                    geom.list_inner.width.saturating_sub(2) as usize,
                    tokens::text(theme, Level::Tertiary).bg(lbg),
                );
            }
        } else {
            let active_bar = tokens::glyph(theme, Glyph::ActiveBar);
            let rh = geom.row_height.max(1);
            let vis = (geom.list_inner.height / rh) as usize;
            for slot in 0..vis {
                let idx = geom.list_scroll + slot;
                let Some(conn) = conns.get(idx) else {
                    break;
                };
                let y = geom.list_inner.y + (slot as u16) * rh;
                let row_rect = Rect::new(geom.list_inner.x, y, geom.list_inner.width, rh);
                let is_cur = idx == cl.conn_cursor;
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
                    for ry in row_rect.top()..row_rect.bottom() {
                        buf.set_stringn(
                            row_rect.x,
                            ry,
                            active_bar,
                            1,
                            tokens::fg(theme, Role::Accent).bg(rbg),
                        );
                    }
                }

                let mut x = row_rect.x + 1;
                let base_lvl = if conn.closed {
                    Level::Tertiary
                } else {
                    Level::Primary
                };
                if show_proc {
                    buf.set_stringn(
                        x,
                        y,
                        clip_ascii_safe(conn.display_process(), proc_w, ascii),
                        proc_w,
                        tokens::text(theme, Level::Secondary).bg(rbg),
                    );
                    x += proc_w as u16 + 1;
                }
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(conn.display_target(), host_w, ascii),
                    host_w,
                    tokens::text(theme, base_lvl)
                        .bg(rbg)
                        .add_modifier(if is_cur {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
                x += host_w as u16 + 1;
                let exit_role = if conn.exit_node.eq_ignore_ascii_case("DIRECT") {
                    Role::Success
                } else if conn.exit_node.eq_ignore_ascii_case("REJECT") {
                    Role::Danger
                } else {
                    Role::Accent
                };
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(&conn.exit_node, exit_w, ascii),
                    exit_w,
                    tokens::fg(theme, exit_role).bg(rbg),
                );
                x += exit_w as u16 + 1;
                if show_speeds {
                    let dn = format_speed_compact(conn.down_speed);
                    let up = format_speed_compact(conn.up_speed);
                    buf.set_stringn(
                        x,
                        y,
                        clip_ascii_safe(&dn, spd_w, ascii),
                        spd_w,
                        tokens::text(
                            theme,
                            if conn.down_speed > 0 {
                                Level::Primary
                            } else {
                                Level::Tertiary
                            },
                        )
                        .bg(rbg),
                    );
                    x += spd_w as u16 + 1;
                    buf.set_stringn(
                        x,
                        y,
                        clip_ascii_safe(&up, spd_w, ascii),
                        spd_w,
                        tokens::text(
                            theme,
                            if conn.up_speed > 0 {
                                Level::Primary
                            } else {
                                Level::Tertiary
                            },
                        )
                        .bg(rbg),
                    );
                }
                let tot = format_bytes_compact(conn.total_bytes());
                tokens::put_right(
                    buf,
                    Rect::new(row_rect.x, y, row_rect.width, 1),
                    &tot,
                    tokens::text(theme, Level::Secondary).bg(rbg),
                );
                if rh >= 2 {
                    let sub = format!("{} · {}", conn.display_process(), conn.rule);
                    buf.set_stringn(
                        row_rect.x + 1,
                        y + 1,
                        clip_ascii_safe(&sub, row_rect.width.saturating_sub(2) as usize, ascii),
                        row_rect.width.saturating_sub(2) as usize,
                        tokens::text(theme, Level::Tertiary).bg(rbg),
                    );
                }
            }
        }
        if geom.prev_btn.is_some() || geom.next_btn.is_some() {
            tokens::draw_pager_buttons(
                buf,
                geom.prev_btn,
                geom.next_btn,
                geom.can_prev,
                geom.can_next,
                None,
                app.hover,
                theme,
                i,
                lbg,
            );
        }
    }

    // 4. Right Inspector card (on wide screens)
    if geom.inspector_area.width >= 16 && geom.inspector_area.height >= 4 {
        draw_inspector_card(app, frame, geom.inspector_area, geom.inspector_body, false);
    }
}

pub(super) fn draw_fullscreen_inspector(app: &App, frame: &mut Frame, area: Rect) {
    let body = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    );
    draw_inspector_card(app, frame, area, body, true);
}

fn draw_inspector_card(app: &App, frame: &mut Frame, area: Rect, body: Rect, fullscreen: bool) {
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
    let right_hint = if fullscreen {
        format!("[Esc] {}", i.text("key.close"))
    } else {
        format!("[x] {}", i.text("clash.key.close_conn"))
    };
    draw_card_title(
        buf,
        area,
        i.text("clash.conn.detail_title"),
        &right_hint,
        theme,
        Role::SurfaceAlt,
    );

    if body.width < 10 || body.height == 0 {
        return;
    }
    let Some(conn) = cl.selected_connection() else {
        buf.set_stringn(
            body.x,
            body.y,
            clip_ascii_safe(i.text("clash.conn.empty"), body.width as usize, ascii),
            body.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    };

    let lines = build_connection_lines(conn, i, body.width as usize, ascii);
    let vis = body.height as usize;
    let max_s = lines.len().saturating_sub(vis) as u16;
    cl.max_conn_inspector_scroll.set(max_s);
    let scroll = (cl.conn_inspector_scroll.min(max_s)) as usize;

    for (slot, (label, val, role)) in lines.iter().skip(scroll).take(vis).enumerate() {
        let y = body.y + slot as u16;
        if label.is_empty() {
            buf.set_stringn(
                body.x,
                y,
                clip_ascii_safe(val, body.width as usize, ascii),
                body.width as usize,
                role.map(|r| tokens::fg(theme, r).bg(bg))
                    .unwrap_or_else(|| tokens::text(theme, Level::Primary).bg(bg)),
            );
        } else {
            let lw = 10usize.min(body.width as usize / 2);
            buf.set_stringn(
                body.x,
                y,
                clip_ascii_safe(label, lw, ascii),
                lw,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
            let vw = (body.width as usize).saturating_sub(lw + 1);
            buf.set_stringn(
                body.x + lw as u16 + 1,
                y,
                clip_ascii_safe(val, vw, ascii),
                vw,
                role.map(|r| tokens::fg(theme, r).bg(bg))
                    .unwrap_or_else(|| tokens::text(theme, Level::Primary).bg(bg)),
            );
        }
    }
}

fn build_connection_lines(
    conn: &ConnectionItem,
    i: &crate::i18n::I18n,
    width: usize,
    ascii: bool,
) -> Vec<(String, String, Option<Role>)> {
    let mut rows = Vec::new();
    let status = if conn.closed {
        i.text("clash.conn.scope_closed")
    } else {
        i.text("clash.conn.scope_running")
    };
    rows.push((
        i.text("clash.profile.status").to_owned(),
        format!("{status} ({} {})", conn.network, conn.conn_type),
        Some(if conn.closed {
            Role::Text3
        } else {
            Role::Success
        }),
    ));
    rows.push((
        i.text("clash.conn.col_target").to_owned(),
        conn.display_target().to_owned(),
        Some(Role::Accent),
    ));
    if !conn.destination.is_empty() && conn.destination != conn.display_target() {
        rows.push(("IP".to_owned(), conn.destination.clone(), None));
    }
    if !conn.source.is_empty() {
        rows.push((
            i.text("clash.conn.source").to_owned(),
            conn.source.clone(),
            None,
        ));
    }
    if !conn.process.is_empty() {
        rows.push((
            i.text("clash.conn.col_process").to_owned(),
            conn.process.clone(),
            None,
        ));
    }
    let rule_text = if conn.rule_payload.is_empty() {
        conn.rule.clone()
    } else {
        format!("{} ({})", conn.rule, conn.rule_payload)
    };
    rows.push((i.text("clash.conn.rule").to_owned(), rule_text, None));
    rows.push((
        i.text("clash.conn.col_down").to_owned(),
        format!(
            "{}  ({} {})",
            format_bytes(conn.download),
            format_speed_compact(conn.down_speed),
            i.text("clash.conn.now")
        ),
        None,
    ));
    rows.push((
        i.text("clash.conn.col_up").to_owned(),
        format!(
            "{}  ({} {})",
            format_bytes(conn.upload),
            format_speed_compact(conn.up_speed),
            i.text("clash.conn.now")
        ),
        None,
    ));

    let arrow = if ascii { " -> " } else { " → " };
    let chain = if conn.chains.is_empty() {
        "-".to_owned()
    } else {
        conn.chains.join(arrow)
    };
    rows.push((i.text("clash.conn.chain").to_owned(), String::new(), None));
    for part in wrap_words(&chain, width.max(8)) {
        rows.push((String::new(), format!("  {part}"), Some(Role::Accent)));
    }
    if !conn.start.is_empty() {
        rows.push((
            i.text("clash.conn.started").to_owned(),
            conn.start.clone(),
            None,
        ));
    }
    rows
}
