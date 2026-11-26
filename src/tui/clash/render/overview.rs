//! FlClash-inspired Mihomo Dashboard (`Overview`) renderer:
//! - Full-width Hero Real-Time Traffic Atlas (Braille waveform + live Exit Node distribution)
//! - Outbound Mode selector strip (Rule / Global / Direct)
//! - Status + Uptime + WebUI + Start/Stop/Reload
//! - Active Subscription Profile card with traffic quota bar & expiry
//! - Server Services (SOCKS5 `cqs-socks5`, TUN, LAN) + Manual Exit IP / Connectivity Probe
//! - Live Active Connections preview + Clickable Policy Group quick cards

use super::{
    draw_card_title, draw_pill_chip, draw_progress_bar, fill_rect_bg, surface_bg, traffic,
};
use crate::{
    ops::clash::model::{
        IpQualityLevel, IpQualityOutcome, format_bytes, format_bytes_compact,
        format_relative_epoch, format_speed_compact, format_uptime,
    },
    theme::{Level, Role},
    tui::{
        clash::layout::OverviewGeom,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{Frame, layout::Rect, style::Modifier};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    traffic::draw_overview_card(app, frame, geom);
    draw_mode_bar(app, frame, geom);
    draw_status_card(app, frame, geom);
    draw_profile_card(app, frame, geom);
    draw_services_card(app, frame, geom);
    draw_connections_preview(app, frame, geom.connections_card);
    draw_groups_preview(app, frame, geom);
}

fn draw_status_card(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let card = geom.status_card;
    if card.width < 16 || card.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let elapsed = app.now.saturating_duration_since(app.started);
    tokens::draw_surface_card_fx(
        frame,
        card,
        theme,
        Role::SurfaceAlt,
        Role::Border,
        false,
        elapsed,
        app.reduced_motion(),
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();

    let uptime = cl
        .snapshot
        .summary
        .uptime_secs
        .map(format_uptime)
        .unwrap_or_else(|| "-".to_owned());
    draw_card_title(
        buf,
        card,
        i.text("clash.overview.status"),
        &uptime,
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let dot = tokens::glyph(theme, Glyph::DotOn);
    let sep = tokens::glyph(theme, Glyph::Separator);
    let sum = &cl.snapshot.summary;
    let pid_str = sum.pid.map(|p| format!("PID {p}")).unwrap_or_default();
    let ver = if sum.version.is_empty() {
        "Mihomo".to_owned()
    } else {
        format!("Mihomo {}", sum.version)
    };
    let mem_part = cl
        .snapshot
        .memory_bytes
        .map(|b| {
            format!(
                " {sep} {} {}",
                i.text("clash.overview.memory"),
                format_bytes(b)
            )
        })
        .unwrap_or_default();
    let line1 = if pid_str.is_empty() {
        format!("{dot} {}  {ver}{mem_part}", i.text("clash.status.running"))
    } else {
        format!(
            "{dot} {}  {ver} ({pid_str}){mem_part}",
            i.text("clash.status.running")
        )
    };
    buf.set_stringn(
        ix,
        card.y + 1,
        clip_ascii_safe(&line1, iw, ascii),
        iw,
        tokens::fg(theme, Role::Success)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let btn_y = card.bottom().saturating_sub(2);
    let panel_label = match (&sum.panel_name, &sum.panel_version) {
        (Some(name), Some(ver)) if !ver.is_empty() => format!("{name} {ver}"),
        (Some(name), _) => name.clone(),
        _ => "WebUI".to_owned(),
    };

    // Line 2: FlClash-style Outbound IP Hero Row ([CC] IP + IP Type / Quality Level chips)
    if card.y + 2 < card.bottom().saturating_sub(1) {
        let ip_y = card.y + 2;
        if let Some(probe) = &cl.exit_probe {
            let badge_rect = geom
                .outbound_badge_hit
                .unwrap_or_else(|| Rect::new(ix, ip_y, 4u16.min(iw as u16), 1));
            let badge_hov = app.hover.is_some_and(|p| badge_rect.contains(p));
            let badge_txt = if cl.exit_probing {
                format!(" {}  ", app.spin())
            } else if probe.country.is_empty() {
                " -- ".to_owned()
            } else {
                format!(" {} ", probe.country)
            };
            draw_pill_chip(
                buf,
                badge_rect,
                &badge_txt,
                cl.exit_probing,
                badge_hov,
                theme,
                bg,
            );

            if let Some(ip_rect) = geom.outbound_ip_hit {
                let ip_hov = app.hover.is_some_and(|p| ip_rect.contains(p));
                let row_bg = if ip_hov {
                    tokens::color(theme, Role::Selection)
                } else {
                    bg
                };
                if ip_hov {
                    fill_rect_bg(buf, ip_rect, row_bg);
                }
                let shown_ip = cl.displayed_ip(&probe.ip);
                let level = probe.quality_ready().map(|q| q.level());
                let ip_style = match level {
                    Some(IpQualityLevel::Good) => tokens::fg(theme, Role::Success),
                    Some(IpQualityLevel::Risky) => tokens::fg(theme, Role::Danger),
                    _ => tokens::text(theme, Level::Primary),
                }
                .bg(row_bg)
                .add_modifier(Modifier::BOLD);

                let avail_w = ip_rect.width as usize;
                let ip_clip = clip_ascii_safe(shown_ip, avail_w, ascii);
                let ip_w = ip_clip.width();
                buf.set_stringn(ip_rect.x, ip_y, &ip_clip, avail_w, ip_style);

                let mut cx = ip_rect.x.saturating_add(ip_w as u16 + 1);
                let right_lim = ip_rect.right();
                match &probe.quality {
                    IpQualityOutcome::Ready(q) => {
                        let type_lbl = format!(" {} ", i.text(q.ip_type.label_key()));
                        let tw = type_lbl.width() as u16;
                        if cx + tw <= right_lim {
                            let chip_bg = tokens::color(theme, Role::Surface);
                            let chip_rect = Rect::new(cx, ip_y, tw, 1);
                            fill_rect_bg(buf, chip_rect, chip_bg);
                            buf.set_stringn(
                                cx,
                                ip_y,
                                clip_ascii_safe(&type_lbl, tw as usize, ascii),
                                tw as usize,
                                tokens::text(theme, Level::Secondary).bg(chip_bg),
                            );
                            cx = cx.saturating_add(tw + 1);
                        }
                        let qual_lbl = format!(
                            " {} {} ",
                            i.text("clash.outbound.quality_short"),
                            i.text(q.level().label_key())
                        );
                        let qw = qual_lbl.width() as u16;
                        if cx + qw <= right_lim {
                            let q_style = match q.level() {
                                IpQualityLevel::Good => tokens::fg(theme, Role::Success)
                                    .bg(tokens::color(theme, Role::Surface))
                                    .add_modifier(Modifier::BOLD),
                                IpQualityLevel::Risky => tokens::fg(theme, Role::Danger)
                                    .bg(tokens::color(theme, Role::Surface))
                                    .add_modifier(Modifier::BOLD),
                                IpQualityLevel::Normal => tokens::text(theme, Level::Secondary)
                                    .bg(tokens::color(theme, Role::Surface)),
                            };
                            let q_rect = Rect::new(cx, ip_y, qw, 1);
                            fill_rect_bg(buf, q_rect, tokens::color(theme, Role::Surface));
                            buf.set_stringn(
                                cx,
                                ip_y,
                                clip_ascii_safe(&qual_lbl, qw as usize, ascii),
                                qw as usize,
                                q_style,
                            );
                        }
                    }
                    IpQualityOutcome::Loading => {
                        let rem_w = right_lim.saturating_sub(cx) as usize;
                        if rem_w >= 4 {
                            let spin_txt = format!(
                                "{} {}",
                                app.spin(),
                                i.text("clash.outbound.quality_loading")
                            );
                            buf.set_stringn(
                                cx,
                                ip_y,
                                clip_ascii_safe(&spin_txt, rem_w, ascii),
                                rem_w,
                                tokens::text(theme, Level::Tertiary).bg(row_bg),
                            );
                        }
                    }
                    IpQualityOutcome::Failed(_) => {
                        let unk_lbl = format!(" {} ", i.text("clash.outbound.type_unknown"));
                        let uw = unk_lbl.width() as u16;
                        if cx + uw <= right_lim {
                            let u_bg = tokens::color(theme, Role::Surface);
                            fill_rect_bg(buf, Rect::new(cx, ip_y, uw, 1), u_bg);
                            buf.set_stringn(
                                cx,
                                ip_y,
                                clip_ascii_safe(&unk_lbl, uw as usize, ascii),
                                uw as usize,
                                tokens::fg(theme, Role::Warning).bg(u_bg),
                            );
                        }
                    }
                }
            }
        } else {
            let ip_rect = geom
                .outbound_ip_hit
                .unwrap_or_else(|| Rect::new(ix, ip_y, iw as u16, 1));
            let ip_hov = app.hover.is_some_and(|p| ip_rect.contains(p));
            let row_bg = if ip_hov {
                tokens::color(theme, Role::Selection)
            } else {
                bg
            };
            if ip_hov {
                fill_rect_bg(buf, ip_rect, row_bg);
            }
            let (text, style) = if cl.exit_probing {
                (
                    format!("{} {}", app.spin(), i.text("clash.overview.probing")),
                    tokens::fg(theme, Role::Accent).bg(row_bg),
                )
            } else if cl.exit_probe_error.is_some() {
                (
                    format!(
                        "! {} {sep} {}",
                        i.text("clash.outbound.check_failed"),
                        i.text("clash.outbound.retry_hint")
                    ),
                    tokens::fg(theme, Role::Danger)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    i.text("clash.overview.exit_hint").to_owned(),
                    tokens::text(theme, Level::Tertiary).bg(row_bg),
                )
            };
            buf.set_stringn(ix, ip_y, clip_ascii_safe(&text, iw, ascii), iw, style);
        }
    }

    // Line 3: Organization / ASN / Quality source (on tall cards height >= 6)
    let webui_y = if card.height >= 7 {
        Some(card.y + 4)
    } else if card.height == 6 {
        Some(card.y + 3)
    } else {
        None
    };
    if card.height >= 7 && card.y + 3 < btn_y {
        let (org_line, org_style) = if let Some(probe) = &cl.exit_probe {
            match &probe.quality {
                IpQualityOutcome::Ready(q) => {
                    let org = probe.display_org().unwrap_or_else(|| "-".to_owned());
                    let mut parts =
                        vec![format!("{} {org}", i.text("clash.outbound.organization"))];
                    if let Some(asn) = q.asn_label()
                        && org != asn
                    {
                        parts.push(asn);
                    }
                    parts.push(q.source.label().to_owned());
                    (
                        parts.join(&format!(" {sep} ")),
                        tokens::text(theme, Level::Secondary).bg(bg),
                    )
                }
                IpQualityOutcome::Loading => {
                    let text = if let Some(org) = probe.display_org() {
                        format!(
                            "{} {org} {sep} {}",
                            i.text("clash.outbound.organization"),
                            probe.ip_source
                        )
                    } else {
                        format!("{} {}", i.text("clash.outbound.source"), probe.ip_source)
                    };
                    (text, tokens::text(theme, Level::Tertiary).bg(bg))
                }
                IpQualityOutcome::Failed(_) => (
                    format!(
                        "{} {sep} {}",
                        i.text("clash.outbound.quality_failed"),
                        i.text("clash.outbound.inspect_hint")
                    ),
                    tokens::fg(theme, Role::Warning).bg(bg),
                ),
            }
        } else if cl.exit_probing {
            (
                i.text("clash.outbound.probing_sub").to_owned(),
                tokens::text(theme, Level::Tertiary).bg(bg),
            )
        } else {
            (
                i.text("clash.outbound.inspect_hint").to_owned(),
                tokens::text(theme, Level::Tertiary).bg(bg),
            )
        };
        buf.set_stringn(
            ix,
            card.y + 3,
            clip_ascii_safe(&org_line, iw, ascii),
            iw,
            org_style,
        );
    }

    // WebUI / Controller endpoint line (row 4 on Wide, row 3 on Medium)
    if let Some(wy) = webui_y
        && wy < btn_y
    {
        let (line_ui, style_ui) = if sum.panel_installed
            && let Some(url) = &sum.panel_url
        {
            let scope = i.text(if sum.panel_loopback {
                "clash.overview.webui_local"
            } else {
                "clash.overview.webui_lan"
            });
            let auth = i.text(if sum.controller_has_secret {
                "clash.overview.webui_auth"
            } else {
                "clash.overview.webui_open"
            });
            let full = format!("WebUI {panel_label} {sep} {url} ({scope}/{auth})");
            let compact = format!("{panel_label} {sep} {url}");
            let chosen = if full.width() <= iw { full } else { compact };
            (chosen, tokens::text(theme, Level::Secondary).bg(bg))
        } else if sum.panel_installed {
            (
                i.format("clash.overview.webui_need_ctrl", &[("panel", &panel_label)]),
                tokens::fg(theme, Role::Warning).bg(bg),
            )
        } else {
            let ctrl = sum.controller_url.as_deref().unwrap_or("-");
            (
                format!("API {ctrl} {sep} {}", i.text("clash.overview.webui_off")),
                tokens::text(theme, Level::Secondary).bg(bg),
            )
        };
        buf.set_stringn(ix, wy, clip_ascii_safe(&line_ui, iw, ascii), iw, style_ui);
    }

    // Bottom action row: [i] Outbound detail pill (left) + [s] Stop & [R] Reload (right)
    if card.height >= 5 && btn_y > card.y + 2 {
        if let Some(btn) = geom.outbound_detail_btn {
            let lbl = format!(" [i] {} ", i.text("clash.key.outbound_detail"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        } else {
            let left_limit = geom
                .start_stop_btn
                .map(|r| r.x.saturating_sub(ix + 1))
                .unwrap_or(iw as u16) as usize;
            if left_limit >= 8
                && let Some(org) = cl.exit_probe.as_ref().and_then(|p| p.display_org())
            {
                buf.set_stringn(
                    ix,
                    btn_y,
                    clip_ascii_safe(&org, left_limit, ascii),
                    left_limit,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            }
        }
        if let Some(btn) = geom.start_stop_btn {
            let lbl = format!(" [s] {} ", i.text("clash.key.stop"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
        if let Some(btn) = geom.reload_btn {
            let lbl = format!(" [R] {} ", i.text("clash.key.reload"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }
}

fn draw_mode_bar(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let bar = geom.mode_bar;
    if bar.width < 16 || bar.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    if bar.height >= 3 {
        tokens::draw_surface_card_fx(
            frame,
            bar,
            theme,
            Role::SurfaceAlt,
            Role::Border,
            false,
            app.now.saturating_duration_since(app.started),
            app.reduced_motion(),
        );
    }
    let bg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();
    let py = if bar.height >= 3 { bar.y + 1 } else { bar.y };
    if bar.height < 3 {
        fill_rect_bg(buf, bar, bg);
    }

    if bar.width >= 48 {
        buf.set_stringn(
            bar.x + 2,
            py,
            i.text("clash.overview.mode"),
            16,
            tokens::text(theme, Level::Secondary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    }

    let active_mode = cl.snapshot.summary.mode.to_ascii_lowercase();
    for &(rect, mode_id) in &geom.mode_pills {
        let key = match mode_id {
            "rule" => "clash.mode.rule",
            "global" => "clash.mode.global",
            _ => "clash.mode.direct",
        };
        let lbl = format!(" {} ", i.text(key));
        let active = active_mode == mode_id;
        let hov = app.hover.is_some_and(|p| rect.contains(p));
        draw_pill_chip(buf, rect, &lbl, active, hov, theme, bg);
    }

    // Right hint `[m] Cycle Mode`
    if bar.width >= 60 {
        let hint = format!("[m] {}", i.text("clash.key.mode"));
        let last_right = geom
            .mode_pills
            .last()
            .map(|(r, _)| r.right())
            .unwrap_or(bar.x + 2);
        if bar.right().saturating_sub(last_right) > hint.width() as u16 + 3 {
            tokens::put_right(
                buf,
                Rect::new(bar.x + 2, py, bar.width.saturating_sub(4), 1),
                &clip_ascii_safe(&hint, 28, ascii),
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
    }
}

fn draw_profile_card(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let card = geom.profile_card;
    if card.width < 16 || card.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
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

    let count_meta = format!(
        "{} {}",
        cl.snapshot.profiles.len(),
        i.text("clash.tab.profiles")
    );
    draw_card_title(
        buf,
        card,
        i.text("clash.overview.profile"),
        &count_meta,
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    if let Some(prof) = cl.snapshot.active_profile() {
        let name_budget = geom
            .profile_update_btn
            .map(|r| r.x.saturating_sub(ix + 1) as usize)
            .unwrap_or(iw);
        buf.set_stringn(
            ix,
            card.y + 1,
            clip_ascii_safe(&prof.name, name_budget, ascii),
            name_budget,
            tokens::text(theme, Level::Primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        if let Some(btn) = geom.profile_update_btn {
            let updating = cl.busy_profiles.contains(&prof.name);
            let lbl = if updating {
                format!(" {} {} ", app.spin(), i.text("clash.key.updating"))
            } else {
                format!(" [u] {} ", i.text("clash.key.update"))
            };
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, updating, hov, theme, bg);
        }
        if let Some(btn) = geom.profile_switch_btn {
            let lbl = format!(" [p] {} ", i.text("clash.key.switch"));
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }

        if card.height >= 4 {
            let age = prof
                .updated_epoch
                .map(|ts| format_relative_epoch(ts, now_epoch))
                .unwrap_or_else(|| "-".to_owned());
            let sep = tokens::glyph(theme, Glyph::Separator);
            let mut sub = format!("{} {sep} {age}", prof.host);
            if let Some(exp) = prof.userinfo.and_then(|u| u.expire_date()) {
                sub.push_str(&format!(" {sep} {} {exp}", i.text("clash.profile.expire")));
            }
            buf.set_stringn(
                ix,
                card.y + 2,
                clip_ascii_safe(&sub, iw, ascii),
                iw,
                tokens::text(theme, Level::Secondary).bg(bg),
            );
        }

        if card.height >= 5 {
            if let Some(info) = prof.userinfo.filter(|u| u.total > 0) {
                let pct = (info.ratio() * 100.0).round() as u64;
                let quota = format!(
                    "{pct}%  {} / {}",
                    format_bytes_compact(info.used()),
                    format_bytes_compact(info.total)
                );
                let fill_role = if pct >= 90 {
                    Role::Danger
                } else if pct >= 75 {
                    Role::Warning
                } else {
                    Role::Accent
                };
                buf.set_stringn(
                    ix,
                    card.y + 3,
                    clip_ascii_safe(&quota, iw, ascii),
                    iw,
                    tokens::fg(theme, fill_role).bg(bg),
                );
                if card.height >= 6 {
                    draw_progress_bar(
                        buf,
                        Rect::new(ix, card.y + 4, iw as u16, 1),
                        info.ratio(),
                        fill_role,
                        theme,
                        bg,
                    );
                }
            } else {
                buf.set_stringn(
                    ix,
                    card.y + 3,
                    clip_ascii_safe(i.text("clash.profile.no_quota"), iw, ascii),
                    iw,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            }
        }
    } else {
        buf.set_stringn(
            ix,
            card.y + 1,
            clip_ascii_safe(i.text("clash.profile.none"), iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }
}

fn draw_services_card(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let card = geom.services_card;
    if card.width < 16 || card.height < 3 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let sum = &cl.snapshot.summary;
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

    draw_card_title(
        buf,
        card,
        i.text("clash.overview.services"),
        "",
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let sep = tokens::glyph(theme, Glyph::Separator);

    // Line 1: Ports + allow-lan + TUN
    let port_str = sum
        .mixed_port
        .map(|p| format!("mixed:{p}"))
        .or_else(|| sum.http_port.map(|p| format!("http:{p}")))
        .unwrap_or_else(|| "port:-".to_owned());
    let lan_str = if sum.allow_lan { "LAN:ON" } else { "LAN:OFF" };
    let tun_str = if sum.tun_enabled {
        if sum.tun_stack.is_empty() {
            "TUN:ON".to_owned()
        } else {
            format!("TUN:{}", sum.tun_stack)
        }
    } else {
        "TUN:OFF".to_owned()
    };
    let line1 = format!("{port_str} {sep} {lan_str} {sep} {tun_str}");
    buf.set_stringn(
        ix,
        card.y + 1,
        clip_ascii_safe(&line1, iw, ascii),
        iw,
        tokens::text(theme, Level::Primary).bg(bg),
    );

    // Line 2: Managed cqs-socks5 inbound
    if card.height >= 4 {
        let socks_line = match &sum.socks5_listen {
            Some(listen) => format!(
                "SOCKS5 {listen} ({})",
                if sum.socks5_auth { "auth" } else { "open" }
            ),
            None => i.text("clash.overview.socks_off").to_owned(),
        };
        let socks_style = if sum.socks5_listen.is_some() {
            tokens::fg(theme, Role::Success).bg(bg)
        } else {
            tokens::text(theme, Level::Tertiary).bg(bg)
        };
        buf.set_stringn(
            ix,
            card.y + 2,
            clip_ascii_safe(&socks_line, iw, ascii),
            iw,
            socks_style,
        );
    }

    // Line 3: Exit Probe connectivity latency (Google / GitHub)
    let btn_y = card.bottom().saturating_sub(2);
    if card.height >= 5 && card.y + 3 < btn_y {
        let probe_line = if cl.exit_probing && cl.exit_probe.is_none() {
            format!("{} {}", app.spin(), i.text("clash.overview.probing"))
        } else if let Some(p) = &cl.exit_probe {
            let g = p
                .google_ms
                .map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".into());
            let gh = p
                .github_ms
                .map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".into());
            format!("Google {g} {sep} GitHub {gh}")
        } else {
            i.text("clash.overview.probe_idle").to_owned()
        };
        buf.set_stringn(
            ix,
            card.y + 3,
            clip_ascii_safe(&probe_line, iw, ascii),
            iw,
            tokens::text(theme, Level::Secondary).bg(bg),
        );
    }

    // Line 4 (when height >= 7): WebUI SSH tunnel / LAN access guidance
    if card.height >= 7 && card.y + 4 < btn_y && sum.panel_installed && sum.panel_url.is_some() {
        let bind = sum
            .panel_bind
            .as_deref()
            .or_else(|| {
                sum.controller_url
                    .as_deref()
                    .and_then(|u| u.strip_prefix("http://"))
            })
            .unwrap_or("127.0.0.1:9090");
        let port = bind.rsplit_once(':').map_or("9090", |(_, p)| p);
        let hint = if sum.panel_loopback {
            i.format("clash.overview.webui_hint_tunnel", &[("port", port)])
        } else if sum.controller_has_secret {
            i.format("clash.overview.webui_hint_lan_auth", &[("bind", bind)])
        } else {
            i.format("clash.overview.webui_hint_lan_open", &[("bind", bind)])
        };
        buf.set_stringn(
            ix,
            card.y + 4,
            clip_ascii_safe(&hint, iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }

    if let Some(btn) = geom.webui_btn {
        let key = if sum.panel_url.is_some() {
            "clash.key.webui_copy"
        } else {
            "clash.key.webui_install"
        };
        let lbl = format!(" [w] {} ", i.text(key));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
    }
    if let Some(btn) = geom.socks_secret_btn {
        let lbl = format!(" [S] {} ", i.text("clash.key.socks_card"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
    }
    if let Some(btn) = geom.exit_probe_btn {
        let lbl = format!(" [N] {} ", i.text("clash.key.probe_exit"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, cl.exit_probing, hov, theme, bg);
    }
}

fn draw_connections_preview(app: &App, frame: &mut Frame, card: Rect) {
    if card.width < 22 || card.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
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

    let meta = format!(
        "TCP {} / UDP {}",
        cl.snapshot.tcp_count(),
        cl.snapshot.udp_count()
    );
    draw_card_title(
        buf,
        card,
        &format!(
            "{} ({})",
            i.text("clash.tab.connections"),
            cl.snapshot.connections.len()
        ),
        &meta,
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let max_rows = card.height.saturating_sub(2) as usize;

    let mut sorted: Vec<_> = cl.snapshot.connections.iter().collect();
    sorted.sort_by(|a, b| {
        b.down_speed
            .saturating_add(b.up_speed)
            .cmp(&a.down_speed.saturating_add(a.up_speed))
            .then_with(|| b.total_bytes().cmp(&a.total_bytes()))
    });

    if sorted.is_empty() {
        buf.set_stringn(
            ix,
            card.y + 1,
            clip_ascii_safe(i.text("clash.conn.empty"), iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    for (row_idx, conn) in sorted.into_iter().take(max_rows).enumerate() {
        let y = card.y + 1 + row_idx as u16;
        let live_rate = conn.down_speed.saturating_add(conn.up_speed);
        let metric = if live_rate > 0 {
            format_speed_compact(live_rate)
        } else {
            format_bytes_compact(conn.total_bytes())
        };
        let exit_w = 12usize.min(iw / 3);
        let metric_w = metric.width().max(7);
        let host_w = iw.saturating_sub(exit_w + metric_w + 3);
        let host = clip_ascii_safe(conn.display_target(), host_w, ascii);
        let exit = clip_ascii_safe(&conn.exit_node, exit_w, ascii);
        buf.set_stringn(
            ix,
            y,
            &host,
            host_w,
            tokens::text(theme, Level::Primary).bg(bg),
        );
        if exit_w >= 4 {
            let ex = ix + host_w as u16 + 1;
            buf.set_stringn(ex, y, &exit, exit_w, tokens::fg(theme, Role::Accent).bg(bg));
        }
        let metric_style = if live_rate > 0 {
            tokens::fg(theme, Role::Success).bg(bg)
        } else {
            tokens::text(theme, Level::Tertiary).bg(bg)
        };
        tokens::put_right(buf, Rect::new(ix, y, iw as u16, 1), &metric, metric_style);
    }
}

fn draw_groups_preview(app: &App, frame: &mut Frame, geom: &OverviewGeom) {
    let card = geom.groups_card;
    if card.width < 18 || card.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let groups = cl.visible_groups();
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

    draw_card_title(
        buf,
        card,
        &format!("{} ({})", i.text("clash.overview.groups"), groups.len()),
        &format!("[Enter] {}", i.text("clash.tab.proxies")),
        theme,
        Role::SurfaceAlt,
    );

    if groups.is_empty() {
        buf.set_stringn(
            card.x + 2,
            card.y + 1,
            clip_ascii_safe(
                i.text("clash.proxies.empty"),
                card.width.saturating_sub(4) as usize,
                ascii,
            ),
            card.width.saturating_sub(4) as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    for &(rect, g_idx) in &geom.group_card_hits {
        let Some(group) = groups.get(g_idx) else {
            continue;
        };
        let focused = g_idx == cl.overview_group_cursor;
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let cell_bg = if focused {
            tokens::color(theme, Role::Selection)
        } else if hovered {
            tokens::color(theme, Role::Surface)
        } else {
            bg
        };
        fill_rect_bg(buf, rect, cell_bg);
        let cw = rect.width.saturating_sub(2) as usize;
        let bar = if focused {
            tokens::glyph(theme, Glyph::ActiveBar)
        } else {
            " "
        };
        buf.set_stringn(
            rect.x,
            rect.y,
            bar,
            1,
            tokens::fg(theme, Role::Accent).bg(cell_bg),
        );
        buf.set_stringn(
            rect.x + 1,
            rect.y,
            clip_ascii_safe(&group.name, cw, ascii),
            cw,
            tokens::text(theme, Level::Primary)
                .bg(cell_bg)
                .add_modifier(Modifier::BOLD),
        );
        let now_text = if group.now.is_empty() {
            "-".to_owned()
        } else {
            group.now.clone()
        };
        buf.set_stringn(
            rect.x + 1,
            rect.y + 1,
            clip_ascii_safe(&now_text, cw, ascii),
            cw,
            tokens::fg(theme, Role::Accent).bg(cell_bg),
        );
    }
}
