//! Modal overlays local to the Mihomo visual workspace:
//! - DelaySettings (temporary test URL + timeout adjustment)
//! - GroupPicker (quick policy group jumper)
//! - Providers (Proxy Providers + Rule Providers update & healthcheck)
//! - OutboundIp (FlClash-style outbound IP, quality level/type/flags, network ownership & hide-IP)
//! - Confirm (delete subscription / close all connections)

use super::{draw_card_title, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    ops::clash::model::{IpQualityLevel, format_bytes_compact},
    theme::{Level, Role},
    tui::{
        clash::{
            layout::{ClashModalGeometry, OutboundModalRow, outbound_modal_rows},
            state::ClashModal,
        },
        fx,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
            wrap_words,
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

pub(in crate::tui) fn draw(app: &App, frame: &mut Frame, area: Rect, modal_alpha: f64) {
    let Some(modal) = &app.clash.modal else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let geom = ClashModalGeometry::compute(area, modal, cl, i);

    fx::apply_shadow(frame.buffer_mut(), geom.box_rect, area, theme);
    tokens::draw_surface_card_fx(
        frame,
        geom.box_rect,
        theme,
        Role::Surface,
        Role::BorderFocus,
        false,
        app.now.saturating_duration_since(app.started),
        app.reduced_motion(),
    );
    let bg = surface_bg(theme, Role::Surface);
    let buf = frame.buffer_mut();

    match modal {
        ClashModal::DelaySettings {
            url,
            timeout,
            active_field,
            error,
        } => {
            draw_card_title(
                buf,
                geom.box_rect,
                i.text("clash.modal.delay_title"),
                i.text("clash.modal.delay_hint"),
                theme,
                Role::Surface,
            );
            let iw = geom.inner.width as usize;
            let mut y = geom.inner.y;

            let draw_field = |buf: &mut ratatui::buffer::Buffer,
                              label: &str,
                              input: &crate::tui::input::InputBuffer,
                              focused: bool,
                              y: u16| {
                buf.set_stringn(
                    geom.inner.x,
                    y,
                    clip_ascii_safe(label, iw, ascii),
                    iw,
                    tokens::text(
                        theme,
                        if focused {
                            Level::Primary
                        } else {
                            Level::Secondary
                        },
                    )
                    .bg(bg)
                    .add_modifier(if focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                );
                let field_rect = Rect::new(geom.inner.x, y + 1, geom.inner.width, 1);
                let fbg = tokens::color(
                    theme,
                    if focused {
                        Role::Selection
                    } else {
                        Role::SurfaceAlt
                    },
                );
                fill_rect_bg(buf, field_rect, fbg);
                let (shown, cur_col) = input.visible(iw.saturating_sub(2));
                buf.set_stringn(
                    field_rect.x + 1,
                    field_rect.y,
                    &shown,
                    iw.saturating_sub(2),
                    tokens::text(theme, Level::Primary).bg(fbg),
                );
                if focused {
                    let cx = field_rect.x + 1 + cur_col.min(iw.saturating_sub(3) as u16);
                    if cx < field_rect.right() {
                        let cell = &mut buf[(cx, field_rect.y)];
                        if cell.symbol().is_empty() || cell.symbol() == " " {
                            cell.set_symbol("_");
                        }
                        cell.set_style(
                            Style::default()
                                .fg(tokens::color(theme, Role::OnAccent))
                                .bg(tokens::color(theme, Role::Accent))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
            };

            if y + 1 < geom.inner.bottom() {
                draw_field(
                    buf,
                    i.text("clash.modal.delay_url"),
                    url,
                    *active_field == 0,
                    y,
                );
                y += 3;
            }
            if y + 1 < geom.inner.bottom() {
                draw_field(
                    buf,
                    i.text("clash.modal.delay_timeout"),
                    timeout,
                    *active_field == 1,
                    y,
                );
                y += 3;
            }
            if let Some(err) = error
                && y < geom.box_rect.bottom().saturating_sub(1)
            {
                buf.set_stringn(
                    geom.inner.x,
                    y,
                    clip_ascii_safe(err, iw, ascii),
                    iw,
                    tokens::fg(theme, Role::Danger).bg(bg),
                );
            }
        }
        ClashModal::GroupPicker { cursor } => {
            draw_card_title(
                buf,
                geom.box_rect,
                i.text("clash.modal.groups_title"),
                i.text("clash.modal.groups_hint"),
                theme,
                Role::Surface,
            );
            let groups = cl.visible_groups();
            let bar = tokens::glyph(theme, Glyph::ActiveBar);
            for &(rect, idx) in &geom.item_rows {
                let Some(g) = groups.get(idx) else {
                    continue;
                };
                let is_cur = idx == *cursor;
                let rbg = if is_cur {
                    tokens::color(theme, Role::Selection)
                } else {
                    bg
                };
                fill_rect_bg(buf, rect, rbg);
                if is_cur {
                    buf.set_stringn(
                        rect.x,
                        rect.y,
                        bar,
                        1,
                        tokens::fg(theme, Role::Accent).bg(rbg),
                    );
                }
                let right = format!("[{}] {}", g.kind.label(), g.now);
                let rw = right.width().min(rect.width as usize / 2);
                let nw = (rect.width as usize).saturating_sub(rw + 3);
                buf.set_stringn(
                    rect.x + 2,
                    rect.y,
                    clip_ascii_safe(&g.name, nw, ascii),
                    nw,
                    tokens::text(theme, Level::Primary)
                        .bg(rbg)
                        .add_modifier(if is_cur {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
                tokens::put_right(
                    buf,
                    rect,
                    &clip_ascii_safe(&right, rw, ascii),
                    tokens::fg(theme, Role::Accent).bg(rbg),
                );
            }
        }
        ClashModal::Providers { show_rules, cursor } => {
            let hint = if *show_rules {
                i.text("clash.modal.providers_hint_rules")
            } else {
                i.text("clash.modal.providers_hint_proxy")
            };
            draw_card_title(
                buf,
                geom.box_rect,
                i.text("clash.modal.providers_title"),
                hint,
                theme,
                Role::Surface,
            );
            // Sub-tab bar inside modal (shares exact rects with mouse hit-tester)
            let p_lbl = format!(
                " {} ({}) ",
                i.text("clash.modal.proxy_providers"),
                cl.snapshot.proxy_providers.len()
            );
            let r_lbl = format!(
                " {} ({}) ",
                i.text("clash.modal.rule_providers"),
                cl.snapshot.rule_providers.len()
            );
            if let Some(rect) = geom.subtab_proxy {
                let hov = app.hover.is_some_and(|p| rect.contains(p));
                draw_pill_chip(buf, rect, &p_lbl, !*show_rules, hov, theme, bg);
            }
            if let Some(rect) = geom.subtab_rules {
                let hov = app.hover.is_some_and(|p| rect.contains(p));
                draw_pill_chip(buf, rect, &r_lbl, *show_rules, hov, theme, bg);
            }

            let bar = tokens::glyph(theme, Glyph::ActiveBar);
            if geom.item_rows.is_empty() {
                buf.set_stringn(
                    geom.inner.x,
                    geom.inner.y + 2,
                    clip_ascii_safe(
                        i.text("clash.modal.providers_empty"),
                        geom.inner.width as usize,
                        ascii,
                    ),
                    geom.inner.width as usize,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            } else {
                for &(rect, idx) in &geom.item_rows {
                    let is_cur = idx == *cursor;
                    let rbg = if is_cur {
                        tokens::color(theme, Role::Selection)
                    } else {
                        bg
                    };
                    fill_rect_bg(buf, rect, rbg);
                    if is_cur {
                        buf.set_stringn(
                            rect.x,
                            rect.y,
                            bar,
                            1,
                            tokens::fg(theme, Role::Accent).bg(rbg),
                        );
                    }
                    if *show_rules {
                        if let Some(rp) = cl.snapshot.rule_providers.get(idx) {
                            let meta = format!(
                                "{} | {} | {}",
                                rp.behavior, rp.vehicle_type, rp.rule_count
                            );
                            let mw = meta.width().min(rect.width as usize / 2);
                            let nw = (rect.width as usize).saturating_sub(mw + 3);
                            buf.set_stringn(
                                rect.x + 2,
                                rect.y,
                                clip_ascii_safe(&rp.name, nw, ascii),
                                nw,
                                tokens::text(theme, Level::Primary).bg(rbg),
                            );
                            tokens::put_right(
                                buf,
                                rect,
                                &clip_ascii_safe(&meta, mw, ascii),
                                tokens::text(theme, Level::Tertiary).bg(rbg),
                            );
                        }
                    } else if let Some(pp) = cl.snapshot.proxy_providers.get(idx) {
                        let quota = pp
                            .userinfo
                            .filter(|u| u.total > 0)
                            .map(|u| {
                                format!(
                                    " {}/{}",
                                    format_bytes_compact(u.used()),
                                    format_bytes_compact(u.total)
                                )
                            })
                            .unwrap_or_default();
                        let meta = format!(
                            "{} | {} {}{quota}",
                            pp.vehicle_type,
                            pp.node_count,
                            i.text("clash.unit.nodes")
                        );
                        let mw = meta.width().min(rect.width as usize * 3 / 5);
                        let nw = (rect.width as usize).saturating_sub(mw + 3);
                        buf.set_stringn(
                            rect.x + 2,
                            rect.y,
                            clip_ascii_safe(&pp.name, nw, ascii),
                            nw,
                            tokens::text(theme, Level::Primary).bg(rbg),
                        );
                        tokens::put_right(
                            buf,
                            rect,
                            &clip_ascii_safe(&meta, mw, ascii),
                            tokens::text(theme, Level::Tertiary).bg(rbg),
                        );
                    }
                }
            }
        }
        ClashModal::OutboundIp { scroll } => {
            let loading = cl.exit_probing
                || cl
                    .exit_probe
                    .as_ref()
                    .is_some_and(|p| p.is_quality_loading());
            let title_hint = if loading {
                format!("{} {}", app.spin(), i.text("clash.outbound.checking"))
            } else {
                i.text("clash.modal.outbound_hint").to_owned()
            };
            draw_card_title(
                buf,
                geom.box_rect,
                i.text("clash.outbound.title"),
                &title_hint,
                theme,
                Role::Surface,
            );

            let rows = outbound_modal_rows(cl, i);
            let vis = geom.outbound_body.height as usize;
            let max_s = rows.len().saturating_sub(vis).min(u16::MAX as usize) as u16;
            cl.max_outbound_modal_scroll.set(max_s);
            let start = (*scroll).min(max_s) as usize;
            let body = geom.outbound_body;
            let bw = body.width as usize;

            for (slot, row) in rows.iter().skip(start).take(vis).enumerate() {
                let y = body.y + slot as u16;
                let row_rect = Rect::new(body.x, y, body.width, 1);
                let interactive = row.action().is_some();
                let hov = interactive && app.hover.is_some_and(|p| row_rect.contains(p));
                let rbg = if hov {
                    tokens::color(theme, Role::Selection)
                } else {
                    bg
                };
                if hov {
                    fill_rect_bg(buf, row_rect, rbg);
                }

                match row {
                    OutboundModalRow::Tip(tip) => {
                        buf.set_stringn(
                            body.x,
                            y,
                            clip_ascii_safe(tip, bw, ascii),
                            bw,
                            tokens::text(theme, Level::Tertiary).bg(bg),
                        );
                    }
                    OutboundModalRow::Blank => {}
                    OutboundModalRow::Section(sec) => {
                        buf.set_stringn(
                            body.x,
                            y,
                            clip_ascii_safe(sec, bw, ascii),
                            bw,
                            tokens::fg(theme, Role::Accent)
                                .bg(bg)
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                    OutboundModalRow::Probing => {
                        let txt = format!("  {} {}", app.spin(), i.text("clash.overview.probing"));
                        buf.set_stringn(
                            body.x,
                            y,
                            clip_ascii_safe(&txt, bw, ascii),
                            bw,
                            tokens::fg(theme, Role::Accent).bg(bg),
                        );
                    }
                    OutboundModalRow::IpRow {
                        label,
                        value,
                        level,
                        failed,
                        copyable,
                    } => {
                        let val_style = if *failed {
                            tokens::fg(theme, Role::Danger)
                        } else {
                            match level {
                                Some(IpQualityLevel::Good) => tokens::fg(theme, Role::Success),
                                Some(IpQualityLevel::Risky) => tokens::fg(theme, Role::Danger),
                                _ => tokens::text(theme, Level::Primary),
                            }
                        }
                        .bg(rbg)
                        .add_modifier(Modifier::BOLD);
                        let right_text = if *copyable && bw >= 34 {
                            format!("{value}  [1]")
                        } else {
                            value.clone()
                        };
                        draw_kv_modal_row(
                            buf,
                            row_rect,
                            label,
                            &right_text,
                            val_style,
                            theme,
                            rbg,
                            ascii,
                        );
                    }
                    OutboundModalRow::KeyValue {
                        label,
                        value,
                        tone,
                        copy_hint,
                        ..
                    } => {
                        let val_style = match tone {
                            Some(role) => tokens::fg(theme, *role).bg(rbg),
                            None => tokens::text(theme, Level::Primary).bg(rbg),
                        };
                        let right_text = match copy_hint {
                            Some(hint) if bw >= 34 => format!("{value}  {hint}"),
                            _ => value.clone(),
                        };
                        draw_kv_modal_row(
                            buf,
                            row_rect,
                            label,
                            &right_text,
                            val_style,
                            theme,
                            rbg,
                            ascii,
                        );
                    }
                    OutboundModalRow::FlagsRow {
                        label,
                        flags,
                        none_label,
                    } => {
                        let lw = bw.min(18);
                        buf.set_stringn(
                            body.x + 2,
                            y,
                            clip_ascii_safe(label, lw.saturating_sub(2), ascii),
                            lw.saturating_sub(2),
                            tokens::text(theme, Level::Secondary).bg(rbg),
                        );
                        if flags.is_empty() {
                            tokens::put_right(
                                buf,
                                row_rect,
                                &clip_ascii_safe(none_label, bw.saturating_sub(lw), ascii),
                                tokens::text(theme, Level::Tertiary).bg(rbg),
                            );
                        } else {
                            let chips: Vec<String> =
                                flags.iter().map(|f| format!("[{f}]")).collect();
                            let joined = chips.join(" ");
                            let max_v = bw.saturating_sub(lw);
                            tokens::put_right(
                                buf,
                                row_rect,
                                &clip_ascii_safe(&joined, max_v, ascii),
                                tokens::fg(theme, Role::Warning)
                                    .bg(rbg)
                                    .add_modifier(Modifier::BOLD),
                            );
                        }
                    }
                    OutboundModalRow::HideIpRow { label, enabled } => {
                        let state_lbl = if *enabled {
                            format!(" [{}] ", i.text("clash.outbound.toggle_on"))
                        } else {
                            format!(" [{}] ", i.text("clash.outbound.toggle_off"))
                        };
                        let val_style = if *enabled {
                            tokens::fg(theme, Role::Accent)
                                .bg(rbg)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            tokens::text(theme, Level::Tertiary).bg(rbg)
                        };
                        draw_kv_modal_row(
                            buf, row_rect, label, &state_lbl, val_style, theme, rbg, ascii,
                        );
                    }
                }
            }

            if max_s > 0 && body.height >= 2 {
                let ind = format!(" {}/{} ", (*scroll).min(max_s) + 1, max_s + 1);
                tokens::put_right(
                    buf,
                    Rect::new(
                        geom.box_rect.x + 2,
                        geom.box_rect.bottom().saturating_sub(1),
                        geom.box_rect.width.saturating_sub(4),
                        1,
                    ),
                    &ind,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            }

            if let Some(btn) = geom.outbound_refresh_btn {
                let lbl = format!(" [r] {} ", i.text("clash.outbound.retry"));
                let hov = app.hover.is_some_and(|p| btn.contains(p));
                draw_pill_chip(buf, btn, &lbl, loading, hov, theme, bg);
            }
            if let Some(btn) = geom.outbound_copy_btn {
                let lbl = format!(" [1] {} ", i.text("clash.outbound.copy_ip"));
                let hov = app.hover.is_some_and(|p| btn.contains(p));
                draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
            }
            if let Some(btn) = geom.outbound_hide_btn {
                let lbl = format!(" [h] {} ", i.text("clash.outbound.hide_ip"));
                let hov = app.hover.is_some_and(|p| btn.contains(p));
                draw_pill_chip(buf, btn, &lbl, cl.hide_ip, hov, theme, bg);
            }
            if let Some(btn) = geom.outbound_close_btn {
                let lbl = format!(" [Esc] {} ", i.text("key.close"));
                let hov = app.hover.is_some_and(|p| btn.contains(p));
                draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
            }
        }
        ClashModal::Confirm {
            title,
            message,
            danger,
            accept,
            ..
        } => {
            draw_card_title(buf, geom.box_rect, title, "", theme, Role::Surface);
            let iw = geom.inner.width as usize;
            for (idx, line) in wrap_words(message, iw.max(8))
                .into_iter()
                .take(geom.inner.height.saturating_sub(2) as usize)
                .enumerate()
            {
                buf.set_stringn(
                    geom.inner.x,
                    geom.inner.y + idx as u16,
                    clip_ascii_safe(&line, iw, ascii),
                    iw,
                    tokens::text(theme, Level::Primary).bg(bg),
                );
            }
            if let Some(yes_rect) = geom.confirm_yes {
                let lbl = format!(" {} ", i.text("key.confirm"));
                let role = if *danger && *accept {
                    Role::Danger
                } else {
                    Role::Accent
                };
                if *accept {
                    fill_rect_bg(buf, yes_rect, tokens::color(theme, role));
                    buf.set_stringn(
                        yes_rect.x,
                        yes_rect.y,
                        clip_ascii_safe(&lbl, yes_rect.width as usize, ascii),
                        yes_rect.width as usize,
                        Style::default()
                            .fg(tokens::color(theme, Role::OnAccent))
                            .bg(tokens::color(theme, role))
                            .add_modifier(Modifier::BOLD),
                    );
                } else {
                    draw_pill_chip(buf, yes_rect, &lbl, false, false, theme, bg);
                }
            }
            if let Some(no_rect) = geom.confirm_no {
                let lbl = format!(" {} ", i.text("ui.cancel"));
                draw_pill_chip(buf, no_rect, &lbl, !*accept, false, theme, bg);
            }
        }
    }

    if modal_alpha < 0.995 {
        fx::apply_fade(
            frame.buffer_mut(),
            geom.box_rect,
            theme.palette.canvas,
            modal_alpha,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_kv_modal_row(
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    label: &str,
    value: &str,
    value_style: Style,
    theme: &crate::theme::Theme,
    bg: ratatui::style::Color,
    ascii: bool,
) {
    let w = rect.width as usize;
    if w < 6 {
        return;
    }
    let label_max = (w * 40 / 100).clamp(8, 22);
    let label_clip = clip_ascii_safe(label, label_max, ascii);
    let label_w = label_clip.width();
    buf.set_stringn(
        rect.x + 2,
        rect.y,
        &label_clip,
        label_max,
        tokens::text(theme, Level::Secondary).bg(bg),
    );
    let val_budget = w.saturating_sub(label_w + 4);
    if val_budget > 0 {
        let val_clip = clip_ascii_safe(value, val_budget, ascii);
        tokens::put_right(buf, rect, &val_clip, value_style);
    }
}
