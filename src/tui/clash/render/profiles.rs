//! FlClash-style Subscriptions (`Profiles`) tab renderer:
//! - Profile cards with active checkmark, host, relative age, `subscription-userinfo` traffic
//!   progress bar & expire date badge
//! - Right details inspector + external Proxy/Rule Providers overview

use super::{
    draw_card_title, draw_inline_filter, draw_pill_chip, draw_progress_bar, fill_rect_bg,
    surface_bg,
};
use crate::{
    ops::clash::model::{format_bytes, format_bytes_compact, format_relative_epoch},
    theme::{Level, Role},
    tui::{
        clash::layout::ProfilesGeom,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{Frame, style::Modifier};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &ProfilesGeom) {
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
            i.text("clash.filter.profiles"),
            theme,
            bg,
        );
        if let Some(btn) = geom.update_all_btn {
            let lbl = if cl.updating_all_profiles {
                format!(" {} {} ", app.spin(), i.text("clash.key.updating"))
            } else {
                format!(" [U] {} ", i.text("clash.key.update_all"))
            };
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, cl.updating_all_profiles, hov, theme, bg);
        }
        if let Some(btn) = geom.add_btn {
            let lbl = format!(" [a] {} ", i.text("clash.key.add"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
        if let Some(btn) = geom.import_btn {
            let lbl = format!(" [i] {} ", i.text("clash.key.import"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
        if let Some(btn) = geom.providers_btn {
            let lbl = format!(" [P] {} ", i.text("clash.key.providers"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }

    // 2. Left Profile List Card
    if geom.list_area.width >= 10 && geom.list_area.height >= 3 {
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
        let profiles = cl.filtered_profiles();
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        if profiles.is_empty() {
            buf.set_stringn(
                geom.list_inner.x,
                geom.list_inner.y,
                clip_ascii_safe(
                    i.text("clash.profile.empty"),
                    geom.list_inner.width as usize,
                    ascii,
                ),
                geom.list_inner.width as usize,
                tokens::text(theme, Level::Tertiary).bg(lbg),
            );
        } else {
            let check = tokens::glyph(theme, Glyph::Success);
            let active_bar = tokens::glyph(theme, Glyph::ActiveBar);
            let sep = tokens::glyph(theme, Glyph::Separator);

            for &(rect, idx) in &geom.item_hits {
                let Some(prof) = profiles.get(idx) else {
                    continue;
                };
                let is_cur = idx == cl.profile_cursor;
                let is_hov = app.hover.is_some_and(|p| rect.contains(p));
                let is_busy = cl.busy_profiles.contains(&prof.name);
                let row_bg = if is_cur {
                    tokens::color(theme, Role::Selection)
                } else if is_hov {
                    tokens::color(theme, Role::Surface)
                } else {
                    lbg
                };
                fill_rect_bg(buf, rect, row_bg);

                for dy in 0..rect.height {
                    buf.set_stringn(
                        rect.x,
                        rect.y + dy,
                        if is_cur {
                            active_bar
                        } else if prof.active {
                            check
                        } else {
                            " "
                        },
                        1,
                        tokens::fg(
                            theme,
                            if prof.active {
                                Role::Success
                            } else {
                                Role::Accent
                            },
                        )
                        .bg(row_bg),
                    );
                }

                let ix = rect.x + 2;
                let iw = rect.width.saturating_sub(3) as usize;

                // Row 0: Name + expire badge + active/age on right
                let age = if is_busy {
                    format!("{} {}", app.spin(), i.text("clash.key.updating"))
                } else {
                    prof.updated_epoch
                        .map(|ts| format_relative_epoch(ts, now_epoch))
                        .unwrap_or_else(|| "-".into())
                };
                let right_tag = if prof.active {
                    format!("{age}  {check}")
                } else {
                    age
                };
                let mut title_line = prof.name.clone();
                if let Some(exp) = prof.userinfo.and_then(|u| u.expire_date()) {
                    title_line.push_str(&format!("  [{exp}]"));
                }
                let name_w = iw.saturating_sub(right_tag.width() + 2);
                buf.set_stringn(
                    ix,
                    rect.y,
                    clip_ascii_safe(&title_line, name_w, ascii),
                    name_w,
                    tokens::text(theme, Level::Primary)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                );
                tokens::put_right(
                    buf,
                    ratatui::layout::Rect::new(ix, rect.y, iw as u16, 1),
                    &right_tag,
                    tokens::fg(
                        theme,
                        if prof.active {
                            Role::Success
                        } else {
                            Role::Text3
                        },
                    )
                    .bg(row_bg),
                );

                // Row 1: Traffic quota or host + file size
                if rect.height >= 2 {
                    let meta = if let Some(info) = prof.userinfo.filter(|u| u.total > 0) {
                        format!(
                            "{} / {}  ({}% ) {sep} {}",
                            format_bytes_compact(info.used()),
                            format_bytes_compact(info.total),
                            (info.ratio() * 100.0).round() as u64,
                            prof.host
                        )
                    } else if let Some(sz) = prof.file_bytes {
                        format!("{} {sep} {}", prof.host, format_bytes_compact(sz))
                    } else {
                        prof.host.clone()
                    };
                    buf.set_stringn(
                        ix,
                        rect.y + 1,
                        clip_ascii_safe(&meta, iw, ascii),
                        iw,
                        tokens::text(theme, Level::Secondary).bg(row_bg),
                    );
                }

                // Row 2: Quota progress bar if available
                if rect.height >= 3
                    && let Some(info) = prof.userinfo.filter(|u| u.total > 0)
                {
                    let ratio = info.ratio();
                    let role = if ratio >= 0.9 {
                        Role::Danger
                    } else if ratio >= 0.75 {
                        Role::Warning
                    } else {
                        Role::Accent
                    };
                    draw_progress_bar(
                        buf,
                        ratatui::layout::Rect::new(ix, rect.y + 2, iw as u16, 1),
                        ratio,
                        role,
                        theme,
                        row_bg,
                    );
                }
            }
        }
    }

    // 3. Right Profile Inspector & Providers summary
    if geom.inspector_area.width >= 18 && geom.inspector_area.height >= 6 {
        draw_profile_inspector(app, frame, geom.inspector_area);
    }
}

fn draw_profile_inspector(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
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
        i.text("clash.profile.detail_title"),
        "",
        theme,
        Role::SurfaceAlt,
    );

    let ix = area.x + 2;
    let iw = area.width.saturating_sub(4) as usize;
    let mut y = area.y + 1;
    let max_y = area.bottom().saturating_sub(1);

    let mut row = |label: &str, val: &str, val_role: Option<Role>| {
        if y >= max_y {
            return;
        }
        let lw = 11usize.min(iw / 2);
        buf.set_stringn(
            ix,
            y,
            clip_ascii_safe(label, lw, ascii),
            lw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        let vw = iw.saturating_sub(lw + 1);
        let st = match val_role {
            Some(r) => tokens::fg(theme, r).bg(bg),
            None => tokens::text(theme, Level::Primary).bg(bg),
        };
        buf.set_stringn(
            ix + lw as u16 + 1,
            y,
            clip_ascii_safe(val, vw, ascii),
            vw,
            st,
        );
        y += 1;
    };

    if let Some(prof) = cl.selected_profile() {
        row(i.text("clash.profile.name"), &prof.name, Some(Role::Accent));
        row(
            i.text("clash.profile.status"),
            i.text(if prof.active {
                "clash.profile.active"
            } else {
                "clash.profile.inactive"
            }),
            Some(if prof.active {
                Role::Success
            } else {
                Role::Text3
            }),
        );
        row(
            i.text("clash.profile.source"),
            if prof.is_local {
                i.text("clash.profile.local")
            } else {
                &prof.host
            },
            None,
        );
        if let Some(sz) = prof.file_bytes {
            row(i.text("clash.profile.size"), &format_bytes(sz), None);
        }
        if let Some(info) = prof.userinfo {
            if info.total > 0 {
                row(
                    i.text("clash.profile.quota"),
                    &format!(
                        "{} / {}",
                        format_bytes(info.used()),
                        format_bytes(info.total)
                    ),
                    None,
                );
            }
            if let Some(exp) = info.expire_date() {
                row(i.text("clash.profile.expire"), &exp, Some(Role::Warning));
            }
        }
        y = y.saturating_add(1);
        if y < max_y {
            let hint = format!(
                "[Enter] {}  [u] {}  [d] {}",
                i.text("clash.key.use_reload"),
                i.text("clash.key.update"),
                i.text("clash.key.delete")
            );
            buf.set_stringn(
                ix,
                y,
                clip_ascii_safe(&hint, iw, ascii),
                iw,
                tokens::text(theme, Level::Secondary).bg(bg),
            );
            y += 2;
        }
    } else {
        buf.set_stringn(
            ix,
            y,
            clip_ascii_safe(i.text("clash.profile.none"), iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        y += 2;
    }

    // External Providers summary
    let pp_len = cl.snapshot.proxy_providers.len();
    let rp_len = cl.snapshot.rule_providers.len();
    if y < max_y && (pp_len > 0 || rp_len > 0) {
        let heading = format!(
            "{} ({pp_len} / {rp_len})  [P]",
            i.text("clash.key.providers")
        );
        buf.set_stringn(
            ix,
            y,
            clip_ascii_safe(&heading, iw, ascii),
            iw,
            tokens::text(theme, Level::Secondary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        y += 1;
        for pp in cl.snapshot.proxy_providers.iter().take(3) {
            if y >= max_y {
                break;
            }
            let line = format!(
                "{} ({} {})",
                pp.name,
                pp.node_count,
                i.text("clash.unit.nodes")
            );
            buf.set_stringn(
                ix,
                y,
                clip_ascii_safe(&line, iw, ascii),
                iw,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
            y += 1;
        }
    }
}
