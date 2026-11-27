//! FlClash-inspired Mihomo Dashboard (`Overview`) renderer:
//! - Full-width Hero Real-Time Traffic Atlas (Braille waveform + live Exit Node distribution)
//! - Outbound Mode selector strip (Rule / Global / Direct)
//! - Status + Uptime + WebUI + Start/Stop/Reload
//! - Active Subscription Profile card with traffic quota bar & expiry
//! - Server Services (SOCKS5 `cqs-socks5`, TUN, LAN) + Manual Exit IP / Connectivity Probe
//! - Live Active Connections preview + Clickable Policy Group quick cards

use super::{
    delay_role, draw_card_title, draw_pill_chip, draw_progress_bar, fill_rect_bg, surface_bg,
    traffic,
};
use crate::{
    ops::clash::model::{
        IpQualityLevel, IpQualityOutcome, SubscriptionUserinfo, UserinfoSource, format_bytes,
        format_bytes_compact, format_relative_age, format_speed_compact, format_uptime,
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

    let sep = tokens::glyph(theme, Glyph::Separator);
    let conns_total = cl.snapshot.connections.len();
    let header_meta = match cl.snapshot.summary.uptime_secs.map(format_uptime) {
        Some(uptime) => format!(
            "{} {conns_total} {sep} {uptime}",
            i.text("clash.overview.conns")
        ),
        None => format!("{} {conns_total}", i.text("clash.overview.conns")),
    };
    draw_card_title(
        buf,
        card,
        i.text("clash.overview.status"),
        &header_meta,
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let dot = tokens::glyph(theme, Glyph::DotOn);
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

    let effective_quota = cl.snapshot.effective_active_userinfo();
    let sep = tokens::glyph(theme, Glyph::Separator);

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
                .map(|ts| format_relative_age(ts, now_epoch, i))
                .unwrap_or_else(|| "-".to_owned());
            let host_lbl = if prof.is_local {
                i.text("clash.profile.local")
            } else {
                prof.host.as_str()
            };
            let extra: Option<(String, Option<Role>)> = if let Some((info, source)) =
                &effective_quota
            {
                if let Some(exp_ts) = info.expire
                    && let Some(exp) = info.expire_date()
                {
                    if exp_ts <= now_epoch {
                        Some((
                            format!("{exp} ({})", i.text("clash.profile.expired")),
                            Some(Role::Danger),
                        ))
                    } else if let Some(days) = info.days_until_expire(now_epoch) {
                        let role = (days <= 7).then_some(Role::Warning);
                        Some((
                            format!(
                                "{exp} ({})",
                                i.format("clash.profile.days_left", &[("days", &days.to_string())])
                            ),
                            role,
                        ))
                    } else {
                        Some((format!("{} {exp}", i.text("clash.profile.expire")), None))
                    }
                } else {
                    match source {
                        UserinfoSource::Provider(name) => Some((
                            i.format("clash.profile.from_provider", &[("name", name)]),
                            None,
                        )),
                        UserinfoSource::Providers(count) => Some((
                            i.format(
                                "clash.profile.from_providers",
                                &[("count", &count.to_string())],
                            ),
                            None,
                        )),
                        UserinfoSource::Profile if info.total > 0 || info.used() > 0 => {
                            Some((i.text("clash.profile.no_expiry").to_owned(), None))
                        }
                        UserinfoSource::Profile => None,
                    }
                }
            } else {
                None
            };
            let prefix_full = format!("{host_lbl} {sep} {age}");
            if let Some((extra_txt, extra_role)) = extra {
                let with_sep = format!(" {sep} {extra_txt}");
                let prefix = if prefix_full.width() + with_sep.width() <= iw {
                    prefix_full
                } else {
                    host_lbl.to_owned()
                };
                let pre_clip = clip_ascii_safe(&prefix, iw, ascii);
                let pre_w = pre_clip.width();
                buf.set_stringn(
                    ix,
                    card.y + 2,
                    &pre_clip,
                    iw,
                    tokens::text(theme, Level::Secondary).bg(bg),
                );
                let rem_w = iw.saturating_sub(pre_w);
                if rem_w >= 4 {
                    let sep_part = format!(" {sep} ");
                    let sep_w = sep_part.width();
                    buf.set_stringn(
                        ix + pre_w as u16,
                        card.y + 2,
                        clip_ascii_safe(&sep_part, rem_w, ascii),
                        rem_w,
                        tokens::text(theme, Level::Tertiary).bg(bg),
                    );
                    let ex_budget = rem_w.saturating_sub(sep_w);
                    if ex_budget > 0 {
                        let ex_style = match extra_role {
                            Some(role) => {
                                tokens::fg(theme, role).bg(bg).add_modifier(Modifier::BOLD)
                            }
                            None => tokens::text(theme, Level::Secondary).bg(bg),
                        };
                        buf.set_stringn(
                            ix + (pre_w + sep_w) as u16,
                            card.y + 2,
                            clip_ascii_safe(&extra_txt, ex_budget, ascii),
                            ex_budget,
                            ex_style,
                        );
                    }
                }
            } else {
                buf.set_stringn(
                    ix,
                    card.y + 2,
                    clip_ascii_safe(&prefix_full, iw, ascii),
                    iw,
                    tokens::text(theme, Level::Secondary).bg(bg),
                );
            }
        }

        if card.height >= 5 {
            if let Some((info, _)) = &effective_quota {
                if info.total > 0 {
                    let fill_role = draw_overview_quota_row(
                        buf,
                        Rect::new(ix, card.y + 3, iw as u16, 1),
                        *info,
                        i,
                        theme,
                        bg,
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
                } else if info.used() > 0 {
                    let used_line = format!(
                        "{} {}",
                        i.text("clash.profile.used"),
                        format_bytes(info.used())
                    );
                    buf.set_stringn(
                        ix,
                        card.y + 3,
                        clip_ascii_safe(&used_line, iw, ascii),
                        iw,
                        tokens::fg(theme, Role::Accent).bg(bg),
                    );
                } else if let Some(exp) = info.expire_date() {
                    let exp_line = format!("{} {exp}", i.text("clash.profile.expire"));
                    buf.set_stringn(
                        ix,
                        card.y + 3,
                        clip_ascii_safe(&exp_line, iw, ascii),
                        iw,
                        tokens::text(theme, Level::Secondary).bg(bg),
                    );
                }
            } else if cl.userinfo_probing_profile.as_deref() == Some(prof.name.as_str()) {
                let loading = format!("{} {}", app.spin(), i.text("clash.profile.quota_loading"));
                buf.set_stringn(
                    ix,
                    card.y + 3,
                    clip_ascii_safe(&loading, iw, ascii),
                    iw,
                    tokens::fg(theme, Role::Accent).bg(bg),
                );
            } else if !prof.is_local {
                buf.set_stringn(
                    ix,
                    card.y + 3,
                    clip_ascii_safe(i.text("clash.profile.no_quota_remote"), iw, ascii),
                    iw,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            } else {
                let local_stats = i.format(
                    "clash.profile.local_stats",
                    &[
                        ("groups", &cl.snapshot.visible_groups().len().to_string()),
                        ("rules", &cl.snapshot.rules.len().to_string()),
                    ],
                );
                buf.set_stringn(
                    ix,
                    card.y + 3,
                    clip_ascii_safe(&local_stats, iw, ascii),
                    iw,
                    tokens::text(theme, Level::Secondary).bg(bg),
                );
            }
        }
    } else if let Some((info, source)) = &effective_quota {
        let title_line = match source {
            UserinfoSource::Provider(name) => {
                i.format("clash.profile.from_provider", &[("name", name)])
            }
            UserinfoSource::Providers(count) => i.format(
                "clash.profile.from_providers",
                &[("count", &count.to_string())],
            ),
            UserinfoSource::Profile => i.text("clash.profile.none").to_owned(),
        };
        buf.set_stringn(
            ix,
            card.y + 1,
            clip_ascii_safe(&title_line, iw, ascii),
            iw,
            tokens::text(theme, Level::Primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        if card.height >= 4 && info.total > 0 {
            let fill_role = draw_overview_quota_row(
                buf,
                Rect::new(ix, card.y + 2, iw as u16, 1),
                *info,
                i,
                theme,
                bg,
            );
            if card.height >= 5 {
                draw_progress_bar(
                    buf,
                    Rect::new(ix, card.y + 3, iw as u16, 1),
                    info.ratio(),
                    fill_role,
                    theme,
                    bg,
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

fn draw_overview_quota_row(
    buf: &mut ratatui::buffer::Buffer,
    row: Rect,
    info: SubscriptionUserinfo,
    i: &crate::i18n::I18n,
    theme: &crate::theme::Theme,
    bg: ratatui::style::Color,
) -> Role {
    let ascii = theme.flags.ascii;
    let sep = tokens::glyph(theme, Glyph::Separator);
    let pct = info.usage_percent();
    let over = info.is_over_quota();
    let fill_role = if over || pct >= 90 {
        Role::Danger
    } else if pct >= 75 {
        Role::Warning
    } else {
        Role::Accent
    };

    let iw = row.width as usize;
    let pct_str = format!("{pct}%");
    let used_lbl = format!(
        " {}",
        i.text(if over {
            "clash.profile.over_quota"
        } else {
            "clash.profile.used"
        })
    );
    let left_w = pct_str.width() + used_lbl.width();
    buf.set_stringn(
        row.x,
        row.y,
        clip_ascii_safe(&pct_str, iw, ascii),
        iw,
        tokens::fg(theme, fill_role)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let lbl_budget = iw.saturating_sub(pct_str.width());
    if lbl_budget > 0 {
        buf.set_stringn(
            row.x + pct_str.width() as u16,
            row.y,
            clip_ascii_safe(&used_lbl, lbl_budget, ascii),
            lbl_budget,
            tokens::text(theme, Level::Secondary).bg(bg),
        );
    }

    let right_budget = iw.saturating_sub(left_w + 2);
    if right_budget < 8 {
        return fill_role;
    }

    let used_full = format_bytes(info.used());
    let total_full = format_bytes(info.total);
    let ratio_full = format!("{used_full} / {total_full}");
    let rem_part = if !over {
        format!(
            " {sep} {} {}",
            i.text("clash.profile.rem_short"),
            format_bytes(info.remaining())
        )
    } else {
        String::new()
    };

    if !rem_part.is_empty() && ratio_full.width() + rem_part.width() <= right_budget {
        let total_rw = ratio_full.width() + rem_part.width();
        let rx = row.right().saturating_sub(total_rw as u16);
        buf.set_stringn(
            rx,
            row.y,
            clip_ascii_safe(&ratio_full, ratio_full.width(), ascii),
            ratio_full.width(),
            tokens::text(theme, Level::Primary).bg(bg),
        );
        let rem_style = if pct >= 90 {
            tokens::fg(theme, Role::Danger).bg(bg)
        } else if pct >= 75 {
            tokens::fg(theme, Role::Warning).bg(bg)
        } else {
            tokens::text(theme, Level::Secondary).bg(bg)
        };
        buf.set_stringn(
            rx + ratio_full.width() as u16,
            row.y,
            clip_ascii_safe(&rem_part, rem_part.width(), ascii),
            rem_part.width(),
            rem_style,
        );
    } else if ratio_full.width() <= right_budget {
        tokens::put_right(
            buf,
            row,
            &clip_ascii_safe(&ratio_full, right_budget, ascii),
            tokens::text(theme, Level::Primary).bg(bg),
        );
    } else {
        let compact = format!(
            "{}/{}",
            format_bytes_compact(info.used()),
            format_bytes_compact(info.total)
        );
        if compact.width() <= right_budget {
            tokens::put_right(
                buf,
                row,
                &clip_ascii_safe(&compact, right_budget, ascii),
                tokens::text(theme, Level::Primary).bg(bg),
            );
        }
    }

    fill_role
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

    let enabled = cl.enabled_services();
    let checked = enabled
        .iter()
        .filter_map(|t| cl.service_entry(*t).and_then(|e| e.result.as_ref()))
        .count();
    let ok = enabled
        .iter()
        .filter_map(|t| cl.service_entry(*t).and_then(|e| e.result.as_ref()))
        .filter(|r| r.status.is_available())
        .count();
    let right_meta = if cl.any_service_probing() {
        format!("{} {}", app.spin(), i.text("clash.service.checking"))
    } else if checked > 0 && ok == checked {
        i.format(
            "clash.service.all_ok",
            &[("ok", &ok.to_string()), ("total", &checked.to_string())],
        )
    } else if checked > ok {
        i.format(
            "clash.service.abnormal_count",
            &[("count", &(checked - ok).to_string())],
        )
    } else {
        String::new()
    };

    draw_card_title(
        buf,
        card,
        i.text("clash.service.title"),
        &right_meta,
        theme,
        Role::SurfaceAlt,
    );

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    let sep = tokens::glyph(theme, Glyph::Separator);
    let dot_on = tokens::glyph(theme, Glyph::DotOn);
    let dot_off = tokens::glyph(theme, Glyph::DotOff);

    // Line 1: Compact server listeners summary (mixed/http port + LAN/TUN + SOCKS5)
    let port_str = sum
        .mixed_port
        .map(|p| format!("mixed:{p}"))
        .or_else(|| sum.http_port.map(|p| format!("http:{p}")))
        .unwrap_or_else(|| "port:-".to_owned());
    let tun_str = if sum.tun_enabled {
        if sum.tun_stack.is_empty() {
            "TUN:ON".to_owned()
        } else {
            format!("TUN:{}", sum.tun_stack)
        }
    } else {
        "TUN:OFF".to_owned()
    };
    let socks_str = match &sum.socks5_listen {
        Some(listen) => format!("SOCKS5 {listen}"),
        None => {
            if sum.allow_lan {
                format!("LAN:ON {sep} {}", i.text("clash.overview.socks_off"))
            } else {
                i.text("clash.overview.socks_off").to_owned()
            }
        }
    };
    let line1 = format!("{port_str} {sep} {tun_str} {sep} {socks_str}");
    buf.set_stringn(
        ix,
        card.y + 1,
        clip_ascii_safe(&line1, iw, ascii),
        iw,
        tokens::text(theme, Level::Secondary).bg(bg),
    );

    // Lines 2-3: Clickable pinned service status grid (FlClash-style)
    let btn_y = card.bottom().saturating_sub(2);
    if !geom.service_cell_hits.is_empty() {
        for &(cell, target) in &geom.service_cell_hits {
            let hov = app.hover.is_some_and(|p| cell.contains(p));
            let cbg = if hov {
                tokens::color(theme, Role::Selection)
            } else {
                surface_bg(theme, Role::Surface)
            };
            fill_rect_bg(buf, cell, cbg);

            let entry = cl.service_entry(target);
            let probing = entry.is_some_and(|e| e.probing);
            let result = entry.and_then(|e| e.result.as_ref());

            let (glyph_s, glyph_style) = if probing {
                (
                    app.spin().to_owned(),
                    tokens::fg(theme, Role::Accent).bg(cbg),
                )
            } else if let Some(res) = result {
                let role = if res.status.is_available() {
                    Role::Success
                } else if res.status.is_warning() {
                    Role::Warning
                } else {
                    Role::Danger
                };
                (dot_on.to_owned(), tokens::fg(theme, role).bg(cbg))
            } else {
                (
                    dot_off.to_owned(),
                    tokens::text(theme, Level::Tertiary).bg(cbg),
                )
            };
            buf.set_stringn(cell.x, cell.y, &glyph_s, 1, glyph_style);

            let (metric, metric_style) = if probing {
                ("...".to_owned(), tokens::fg(theme, Role::Accent).bg(cbg))
            } else if let Some(res) = result {
                if res.status.is_available()
                    && let Some(ms) = res.delay_ms
                {
                    let label = if cell.width >= 22
                        && let Some(reg) = res.region.as_deref()
                    {
                        format!("{reg} {ms}ms")
                    } else {
                        format!("{ms}ms")
                    };
                    let st = if ms < 400 {
                        tokens::fg(theme, Role::Success).bg(cbg)
                    } else {
                        tokens::text(theme, Level::Primary).bg(cbg)
                    };
                    (label, st)
                } else {
                    let role = if res.status.is_available() {
                        Role::Success
                    } else if res.status.is_warning() {
                        Role::Warning
                    } else {
                        Role::Danger
                    };
                    (
                        i.text(res.status.label_key()).to_owned(),
                        tokens::fg(theme, role).bg(cbg),
                    )
                }
            } else {
                ("-".to_owned(), tokens::text(theme, Level::Tertiary).bg(cbg))
            };

            let mw = metric.width().min(cell.width as usize / 2);
            let name_budget = (cell.width as usize).saturating_sub(mw + 3);
            if name_budget > 0 {
                buf.set_stringn(
                    cell.x + 2,
                    cell.y,
                    clip_ascii_safe(target.label(), name_budget, ascii),
                    name_budget,
                    tokens::text(theme, Level::Primary).bg(cbg),
                );
            }
            if mw > 0 {
                tokens::put_right(
                    buf,
                    cell,
                    &clip_ascii_safe(&metric, mw, ascii),
                    metric_style,
                );
            }
        }
    } else if card.height >= 4 && card.y + 2 < btn_y {
        let socks_line = match &sum.socks5_listen {
            Some(listen) => format!(
                "SOCKS5 {listen} ({})",
                if sum.socks5_auth { "auth" } else { "open" }
            ),
            None => i.text("clash.overview.socks_off").to_owned(),
        };
        buf.set_stringn(
            ix,
            card.y + 2,
            clip_ascii_safe(&socks_line, iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(bg),
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
    if let Some(btn) = geom.service_check_btn {
        let lbl = format!(" [c] {} ", i.text("clash.key.services"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, cl.any_service_probing(), hov, theme, bg);
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

    let arrow = if ascii { ">" } else { "›" };
    for &(rect, g_idx) in &geom.group_card_hits {
        let Some(group) = groups.get(g_idx) else {
            continue;
        };
        let focused = g_idx == cl.overview_group_cursor;
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let cell_bg = if focused {
            tokens::color(theme, Role::Selection)
        } else if hovered && !theme.flags.solid_selection {
            super::blend_role_bg(
                theme,
                Role::SurfaceAlt,
                Role::Selection,
                if theme.light { 0.55 } else { 0.45 },
                Role::SurfaceAlt,
            )
        } else {
            bg
        };
        let solid_focused = focused && theme.flags.solid_selection;
        fill_rect_bg(buf, rect, cell_bg);
        let cw = rect.width.saturating_sub(2) as usize;
        let bar = if focused {
            tokens::glyph(theme, Glyph::ActiveBar)
        } else {
            " "
        };
        let bar_style = ratatui::style::Style::default()
            .fg(if solid_focused {
                tokens::selected_accent(theme)
            } else {
                tokens::color(theme, Role::Accent)
            })
            .bg(cell_bg)
            .add_modifier(Modifier::BOLD);
        buf.set_stringn(rect.x, rect.y, bar, 1, bar_style);
        if rect.height >= 2 {
            buf.set_stringn(rect.x, rect.y + 1, bar, 1, bar_style);
        }

        let delay_badge = cl
            .group_resolved_delay(group)
            .map(|ms| (format!("{ms}ms"), delay_role(Some(ms), true)));
        let name_budget = match &delay_badge {
            Some((lbl, _)) if cw >= lbl.width() + 10 => cw.saturating_sub(lbl.width() + 1),
            _ => cw,
        };
        let name_style = if solid_focused {
            ratatui::style::Style::default()
                .fg(tokens::selected_fg(theme))
                .bg(cell_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            tokens::text(theme, Level::Primary)
                .bg(cell_bg)
                .add_modifier(Modifier::BOLD)
        };
        buf.set_stringn(
            rect.x + 1,
            rect.y,
            clip_ascii_safe(&group.name, name_budget, ascii),
            name_budget,
            name_style,
        );
        if let Some((lbl, role)) = &delay_badge
            && cw >= lbl.width() + 10
        {
            let d_style = if solid_focused {
                ratatui::style::Style::default()
                    .fg(tokens::selected_fg(theme))
                    .bg(cell_bg)
            } else {
                tokens::fg(theme, *role).bg(cell_bg)
            };
            tokens::put_right(
                buf,
                Rect::new(rect.x + 1, rect.y, cw as u16, 1),
                lbl,
                d_style,
            );
        }

        let now_text = if group.resolved_chain.len() >= 2 && cw >= 18 {
            let last = group
                .resolved_chain
                .last()
                .map(String::as_str)
                .unwrap_or(&group.now);
            let full = format!("{} {arrow} {last}", group.now);
            if full.width() <= cw {
                full
            } else {
                format!("{arrow} {last}")
            }
        } else if group.now.is_empty() {
            "-".to_owned()
        } else {
            group.now.clone()
        };
        let now_style = if solid_focused {
            ratatui::style::Style::default()
                .fg(tokens::selected_fg(theme))
                .bg(cell_bg)
        } else {
            tokens::fg(theme, Role::Accent).bg(cell_bg)
        };
        buf.set_stringn(
            rect.x + 1,
            rect.y + 1,
            clip_ascii_safe(&now_text, cw, ascii),
            cw,
            now_style,
        );
    }
}
