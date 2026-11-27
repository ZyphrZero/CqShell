//! Mihomo Routing Rules (`/rules`) & Rule Providers (`/providers/rules`) tab renderer.

use super::{draw_card_title, draw_inline_filter, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    theme::{Level, Role},
    tui::{
        clash::layout::RulesGeom,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{Frame, layout::Rect, style::Modifier};

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &RulesGeom) {
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
            i.text("clash.filter.rules"),
            theme,
            bg,
        );
        if let Some(btn) = geom.providers_btn {
            let lbl = format!(" [P] {} ", i.text("clash.key.rule_providers"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }

    // 2. Rule-type facet chips
    if geom.facet_bar.height > 0 {
        let buf = frame.buffer_mut();
        for (rect, facet) in &geom.facet_chips {
            let active = cl.rule_facet == *facet;
            let hov = app.hover.is_some_and(|p| rect.contains(p));
            let lbl = match facet {
                None => format!(" {} ", i.text("clash.filter.all")),
                Some(f) => format!(" {f} "),
            };
            draw_pill_chip(buf, *rect, &lbl, active, hov, theme, bg);
        }
    }

    // 3. Rules table
    if geom.list_area.width >= 14 && geom.list_area.height >= 3 {
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
        let rules = cl.filtered_rules();

        let w = geom.table_header.width as usize;
        let idx_w = if w >= 52 { 5 } else { 0 };
        let kind_w = if w >= 48 { 14 } else { 10 };
        let policy_w = if w >= 48 { 16 } else { 10 };
        let payload_w = w.saturating_sub(idx_w + usize::from(idx_w > 0) + kind_w + policy_w + 4);

        if geom.table_header.height > 0 {
            let mut x = geom.table_header.x + 1;
            let y = geom.table_header.y;
            let hdr_style = tokens::text(theme, Level::Tertiary)
                .bg(lbg)
                .add_modifier(Modifier::BOLD);
            if idx_w > 0 {
                buf.set_stringn(x, y, "#", idx_w, hdr_style);
                x += idx_w as u16 + 1;
            }
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(i.text("clash.rules.col_type"), kind_w, ascii),
                kind_w,
                hdr_style,
            );
            x += kind_w as u16 + 1;
            buf.set_stringn(
                x,
                y,
                clip_ascii_safe(i.text("clash.rules.col_payload"), payload_w, ascii),
                payload_w,
                hdr_style,
            );
            tokens::put_right(
                buf,
                geom.table_header,
                i.text("clash.rules.col_policy"),
                hdr_style,
            );
        }

        if rules.is_empty() {
            if geom.list_inner.height > 0 {
                buf.set_stringn(
                    geom.list_inner.x + 1,
                    geom.list_inner.y,
                    clip_ascii_safe(
                        i.text("clash.rules.empty"),
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
                let Some(rule) = rules.get(idx) else {
                    break;
                };
                let y = geom.list_inner.y + (slot as u16) * rh;
                let row_rect = Rect::new(geom.list_inner.x, y, geom.list_inner.width, rh);
                let is_cur = idx == cl.rule_cursor;
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
                if idx_w > 0 {
                    buf.set_stringn(
                        x,
                        y,
                        format!("{:>4}", rule.index),
                        idx_w,
                        tokens::text(theme, Level::Tertiary).bg(rbg),
                    );
                    x += idx_w as u16 + 1;
                }
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(&rule.kind, kind_w, ascii),
                    kind_w,
                    tokens::text(theme, Level::Secondary).bg(rbg),
                );
                x += kind_w as u16 + 1;
                buf.set_stringn(
                    x,
                    y,
                    clip_ascii_safe(&rule.payload, payload_w, ascii),
                    payload_w,
                    tokens::text(theme, Level::Primary)
                        .bg(rbg)
                        .add_modifier(if is_cur {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
                let p_role = if rule.proxy.eq_ignore_ascii_case("DIRECT") {
                    Role::Success
                } else if rule.proxy.eq_ignore_ascii_case("REJECT") {
                    Role::Danger
                } else {
                    Role::Accent
                };
                tokens::put_right(
                    buf,
                    Rect::new(row_rect.x, y, row_rect.width, 1),
                    &clip_ascii_safe(&rule.proxy, policy_w, ascii),
                    tokens::fg(theme, p_role).bg(rbg),
                );
                if rh >= 2 && !is_cur && !is_hov && !ascii {
                    let sep_style = ratatui::style::Style::default()
                        .fg(tokens::color(theme, Role::Rule))
                        .bg(lbg);
                    for col in (row_rect.x + 1)..row_rect.right() {
                        buf[(col, y + 1)].set_symbol("┄").set_style(sep_style);
                    }
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

    // 4. Right Rule Providers side card (on wide screens when providers exist)
    if geom.providers_area.width >= 18 && geom.providers_area.height >= 5 {
        tokens::draw_surface_card_fx(
            frame,
            geom.providers_area,
            theme,
            Role::SurfaceAlt,
            Role::Border,
            false,
            app.now.saturating_duration_since(app.started),
            app.reduced_motion(),
        );
        let pbg = surface_bg(theme, Role::SurfaceAlt);
        let buf = frame.buffer_mut();
        draw_card_title(
            buf,
            geom.providers_area,
            i.text("clash.key.rule_providers"),
            "[P]",
            theme,
            Role::SurfaceAlt,
        );
        let ix = geom.providers_area.x + 2;
        let iw = geom.providers_area.width.saturating_sub(4) as usize;
        let max_rows = geom.providers_area.height.saturating_sub(2) as usize;
        for (idx, rp) in cl.snapshot.rule_providers.iter().take(max_rows).enumerate() {
            let y = geom.providers_area.y + 1 + idx as u16;
            let count = format!("{}", rp.rule_count);
            let name_w = iw.saturating_sub(count.len() + 2);
            buf.set_stringn(
                ix,
                y,
                clip_ascii_safe(&rp.name, name_w, ascii),
                name_w,
                tokens::text(theme, Level::Primary).bg(pbg),
            );
            tokens::put_right(
                buf,
                Rect::new(ix, y, iw as u16, 1),
                &count,
                tokens::text(theme, Level::Tertiary).bg(pbg),
            );
        }
    }
}
