//! Independent Overview Bento Dashboard:
//! - Top row: System & Resource telemetry card + Network & Proxy telemetry card
//! - Bottom row: 2x2 interactive Smart Quick Action cards

use super::{
    clip_ascii_safe,
    tokens::{self, Glyph},
};
use crate::{
    dashboard::{Reading, Usage},
    ops::clash::State as ClashState,
    theme::{Level, Role, UiStyle},
    tui::{
        fx,
        layout::OverviewGeometry,
        state::{App, Tone},
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

const GIB: f64 = (1u64 << 30) as f64;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &OverviewGeometry) {
    draw_system_card(app, frame, geom.system_card);
    draw_network_card(app, frame, geom.network_card);
    draw_quick_section(app, frame, geom);
}

fn draw_card_header(
    app: &App,
    frame: &mut Frame,
    card: Rect,
    title: &str,
    right_meta: &str,
    bg_role: Role,
) {
    if card.height < 2 || card.width < 8 {
        return;
    }
    let theme = &app.theme.active;
    let buf = frame.buffer_mut();
    let title_y = card.y;
    let title_x = card.x + 2;
    let budget = card.width.saturating_sub(4) as usize;
    let bg = tokens::card_title_bg(theme, bg_role);

    let label = tokens::format_card_title(theme, title, theme.flags.framed);
    let clipped_label = clip_ascii_safe(&label, budget, theme.flags.ascii);
    buf.set_stringn(
        title_x,
        title_y,
        &clipped_label,
        budget,
        Style::default()
            .fg(tokens::card_title_fg(theme, false))
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    if !right_meta.is_empty() {
        let meta = if theme.flags.framed {
            format!(" {right_meta} ")
        } else {
            right_meta.to_owned()
        };
        let min_needed = clipped_label.width() + meta.width() + usize::from(theme.flags.ascii);
        if min_needed <= budget {
            tokens::put_right(
                buf,
                Rect::new(title_x, title_y, card.width.saturating_sub(4), 1),
                &meta,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
    }
}

fn draw_system_card(app: &App, frame: &mut Frame, card: Rect) {
    if card.height < 3 || card.width < 16 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    tokens::draw_surface_card_fx(
        frame,
        card,
        theme,
        Role::Surface,
        Role::Border,
        false,
        elapsed,
        reduced,
    );

    let age = app
        .snapshot
        .local
        .updated
        .map(|at| {
            let rel = short_age(app.now.saturating_duration_since(at));
            i.format("dash.age", &[("age", &rel)])
        })
        .unwrap_or_else(|| format!("{} {}", app.spin(), i.text("dash.loading")));
    draw_card_header(
        app,
        frame,
        card,
        i.text("dash.panel_system"),
        &age,
        Role::Surface,
    );

    let top_inset = tokens::card_top_inset(theme, card.height);
    let bottom_inset = tokens::card_bottom_inset(theme, card.height);
    let start_y = card.y + top_inset;
    let inner = Rect::new(
        card.x + 2,
        start_y,
        card.width.saturating_sub(4),
        card.bottom().saturating_sub(start_y + bottom_inset),
    );
    if inner.height == 0 || inner.width < 12 {
        return;
    }

    let local = &app.snapshot.local;
    let sep = format!(" {} ", tokens::glyph(theme, Glyph::Separator));
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Ultra-compact 1-2 line summary for very short mobile screens (e.g. 40x16 with virtual keyboard)
    if inner.height <= 2 {
        let cpu_s = app.gauges[0]
            .map(|t| format!("{:.0}%", t.value(app.now, reduced)))
            .unwrap_or_else(|| "-".into());
        let mem_s = app.gauges[1]
            .map(|t| format!("{:.0}%", t.value(app.now, reduced)))
            .unwrap_or_else(|| "-".into());
        let dsk_s = app.gauges[2]
            .map(|t| format!("{:.0}%", t.value(app.now, reduced)))
            .unwrap_or_else(|| "-".into());
        let summary = format!(
            "{} {cpu_s}{sep}{} {mem_s}{sep}{} {dsk_s}",
            i.text("dash.cpu"),
            i.text("dash.memory"),
            i.text("dash.disk"),
        );
        lines.push(Line::from(Span::styled(
            clip_ascii_safe(&summary, inner.width as usize, theme.flags.ascii),
            tokens::text(theme, Level::Primary),
        )));
        if inner.height >= 2 {
            let host = match (&local.host, &local.os) {
                (Reading::Value(h), Reading::Value(os)) => format!("{h}{sep}{os}"),
                (Reading::Value(h), _) => h.clone(),
                (other, _) => placeholder(app, other).0,
            };
            lines.push(fact_line(
                app,
                i.text("dash.host"),
                &host,
                Tone::Plain,
                inner.width as usize,
            ));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    // Compute unified column widths across CPU / Memory / Disk / Swap so all 4 progress bars
    // start and end at the exact same horizontal coordinates and stretch cleanly with card width.
    let width = inner.width as usize;
    let label_w = [
        i.text("dash.cpu"),
        i.text("dash.memory"),
        i.text("dash.disk"),
        i.text("dash.swap"),
        i.text("dash.host"),
        i.text("dash.row_kernel"),
        i.text("dash.row_uptime"),
    ]
    .into_iter()
    .map(UnicodeWidthStr::width)
    .max()
    .unwrap_or(4)
    .clamp(5, 6)
        + 1;
    let pct_w = 5usize; // " 100%" or "  63%"
    let show_swap = inner.height >= 6;

    let mem_detail = usage_detail_text(&local.memory);
    let disk_detail = usage_detail_text(&local.disk);
    let swap_detail = if show_swap {
        usage_detail_text(&local.swap)
    } else {
        None
    };
    let cores_text = match local.cpu_count {
        Reading::Value(cores) => format!("{cores}C"),
        _ => String::new(),
    };

    let max_detail_w = [
        mem_detail
            .as_deref()
            .map(UnicodeWidthStr::width)
            .unwrap_or(0),
        disk_detail
            .as_deref()
            .map(UnicodeWidthStr::width)
            .unwrap_or(0),
        swap_detail
            .as_deref()
            .map(UnicodeWidthStr::width)
            .unwrap_or(0),
        cores_text.width(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);

    let (bar_w, detail_col_w) =
        if max_detail_w > 0 && width >= label_w + 8 + pct_w + 2 + max_detail_w {
            let detail_col = max_detail_w
                .max(10)
                .min(width.saturating_sub(label_w + 8 + pct_w + 2));
            let bar = width.saturating_sub(label_w + pct_w + 2 + detail_col);
            (bar.max(6), detail_col)
        } else {
            let bar = width.saturating_sub(label_w + pct_w).max(4);
            (bar, 0)
        };

    let sheen_age = local
        .updated
        .map(|at| app.now.saturating_duration_since(at))
        .unwrap_or(fx::GAUGE_SHEEN);
    let shimmer_col = fx::refresh_sheen_index(sheen_age, bar_w, reduced);

    // Row 1: CPU equal-width bar + bold semantic % + right-aligned cores (+ mini sparkline on wide)
    let cpu_val = app.gauges[0].map(|t| t.value(app.now, reduced));
    lines.push(cpu_row(
        app,
        i.text("dash.cpu"),
        label_w,
        &local.cpu_usage,
        cpu_val,
        &cores_text,
        bar_w,
        detail_col_w,
        elapsed,
        reduced,
        shimmer_col,
    ));

    // Row 2: Memory bar + bold % + right-aligned used/total
    lines.push(usage_row(
        app,
        i.text("dash.memory"),
        label_w,
        &local.memory,
        app.gauges[1].map(|t| t.value(app.now, reduced)),
        mem_detail.as_deref(),
        bar_w,
        detail_col_w,
        elapsed,
        reduced,
        shimmer_col,
    ));

    // Row 3: Disk bar + bold % + right-aligned used/total
    lines.push(usage_row(
        app,
        i.text("dash.disk"),
        label_w,
        &local.disk,
        app.gauges[2].map(|t| t.value(app.now, reduced)),
        disk_detail.as_deref(),
        bar_w,
        detail_col_w,
        elapsed,
        reduced,
        shimmer_col,
    ));

    // Row 4: Swap bar (if room >= 6 lines)
    if show_swap {
        lines.push(usage_row(
            app,
            i.text("dash.swap"),
            label_w,
            &local.swap,
            app.gauges[3].map(|t| t.value(app.now, reduced)),
            swap_detail.as_deref(),
            bar_w,
            detail_col_w,
            elapsed,
            reduced,
            shimmer_col,
        ));
    }

    // Visual breathing row separating resource progress bars from host metadata on tall cards
    if inner.height >= 8 {
        lines.push(Line::raw(""));
    }

    // Host / OS
    let host = match (&local.host, &local.os) {
        (Reading::Value(h), Reading::Value(os)) => format!("{h}{sep}{os}"),
        (Reading::Value(h), _) => h.clone(),
        (other, _) => placeholder(app, other).0,
    };
    lines.push(fact_line_with_width(
        app,
        i.text("dash.host"),
        label_w,
        &host,
        Tone::Plain,
        inner.width as usize,
    ));

    // Kernel + Arch (on 6-row mobile card, include uptime on this line so nothing is hidden)
    let kernel = match (&local.kernel, &local.uptime) {
        (Reading::Value(k), Reading::Value(secs)) if inner.height == 6 => {
            format!("{k}{sep}{}{sep}{}", local.arch, format_uptime(*secs))
        }
        (Reading::Value(k), _) => format!("{k}{sep}{}", local.arch),
        (other, _) => placeholder(app, other).0,
    };
    lines.push(fact_line_with_width(
        app,
        i.text("dash.row_kernel"),
        label_w,
        &kernel,
        Tone::Muted,
        inner.width as usize,
    ));

    // Uptime + Timezone
    let up = match (&local.uptime, &local.timezone) {
        (Reading::Value(secs), Reading::Value(tz)) => format!("{}{sep}{tz}", format_uptime(*secs)),
        (Reading::Value(secs), _) => format_uptime(*secs),
        (other, _) => placeholder(app, other).0,
    };
    lines.push(fact_line_with_width(
        app,
        i.text("dash.row_uptime"),
        label_w,
        &up,
        Tone::Muted,
        inner.width as usize,
    ));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_network_card(app: &App, frame: &mut Frame, card: Rect) {
    if card.height < 3 || card.width < 16 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    tokens::draw_surface_card_fx(
        frame,
        card,
        theme,
        Role::Surface,
        Role::Border,
        false,
        elapsed,
        reduced,
    );

    let meta = app
        .snapshot
        .public
        .updated
        .map(|at| {
            let rel = short_age(app.now.saturating_duration_since(at));
            i.format("dash.public_updated", &[("age", &rel)])
        })
        .unwrap_or_else(|| i.text("dash.not_queried").to_owned());
    draw_card_header(
        app,
        frame,
        card,
        i.text("dash.panel_network"),
        &meta,
        Role::Surface,
    );

    let top_inset = tokens::card_top_inset(theme, card.height);
    let bottom_inset = tokens::card_bottom_inset(theme, card.height);
    let start_y = card.y + top_inset;
    let inner = Rect::new(
        card.x + 2,
        start_y,
        card.width.saturating_sub(4),
        card.bottom().saturating_sub(start_y + bottom_inset),
    );
    if inner.height == 0 || inner.width < 12 {
        return;
    }

    let local = &app.snapshot.local;
    let public = &app.snapshot.public;
    let sep = format!(" {} ", tokens::glyph(theme, Glyph::Separator));
    let on = tokens::glyph(theme, Glyph::DotOn);
    let off = tokens::glyph(theme, Glyph::DotOff);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // BBR
    let (bbr_text, bbr_tone) = match &local.congestion {
        Reading::Value(algo) if algo == "bbr" => {
            let ver = match &local.bbr_version {
                Reading::Value(v) => format!(" {v}"),
                _ => String::new(),
            };
            (
                format!("{on} {}{ver}", i.text("status.enabled")),
                Tone::Success,
            )
        }
        Reading::Value(algo) => {
            let avail = match (&local.bbr_available, &local.bbr_version) {
                (Reading::Value(true), Reading::Value(v)) => {
                    format!("{sep}{} / {v}", i.text("dash.available"))
                }
                (Reading::Value(true), _) => format!("{sep}{}", i.text("dash.available")),
                _ => String::new(),
            };
            (
                format!("{off} {}{avail}{sep}{algo}", i.text("status.disabled")),
                Tone::Warning,
            )
        }
        other => placeholder(app, other),
    };

    // TCP & qdisc
    let (tcp_text, tcp_tone) = match &local.tcp_configured {
        Reading::Value(true) => {
            let q = match &local.qdisc {
                Reading::Value(q) => format!("{sep}{q}"),
                _ => String::new(),
            };
            (
                format!("{on} {}{q}", i.text("dash.configured")),
                Tone::Success,
            )
        }
        Reading::Value(false) => {
            let q = match &local.qdisc {
                Reading::Value(q) => format!("{sep}{q}"),
                _ => String::new(),
            };
            (
                format!("{off} {}{q}", i.text("dash.not_configured")),
                Tone::Muted,
            )
        }
        other => placeholder(app, other),
    };

    // IPv4 + ISP
    let (v4_text, v4_tone) = match (&public.ipv4, &public.isp) {
        (Reading::Value(ip), Reading::Value(isp)) if !isp.is_empty() => {
            (format!("{ip}{sep}{isp}"), Tone::Plain)
        }
        (Reading::Value(ip), _) => (ip.clone(), Tone::Plain),
        (other, _) => placeholder(app, other),
    };

    // IPv6
    let (v6_text, v6_tone) = match &public.ipv6 {
        Reading::Value(ip) => (ip.clone(), Tone::Muted),
        other => placeholder(app, other),
    };

    // Mihomo + Terminal Proxy
    let (proxy_text, proxy_tone) = match &local.mihomo_state {
        Reading::Value(ClashState::Running(_)) => {
            let ver = match &local.mihomo_version {
                Reading::Value(v) => format!("{sep}{v}"),
                _ => String::new(),
            };
            let term = match local.terminal_proxy {
                Reading::Value(true) => format!("{sep}{} {on}", i.text("dash.terminal_proxy")),
                Reading::Value(false) => format!("{sep}{} {off}", i.text("dash.terminal_proxy")),
                _ => String::new(),
            };
            (
                format!("Mihomo {on} {}{ver}{term}", i.text("dash.running")),
                Tone::Success,
            )
        }
        Reading::Value(ClashState::Stopped) => (
            format!("Mihomo {off} {}", i.text("dash.stopped")),
            Tone::Warning,
        ),
        Reading::Value(ClashState::NotInstalled) => (
            format!("Mihomo {off} {}", i.text("dash.not_installed")),
            Tone::Muted,
        ),
        Reading::Value(ClashState::Unknown) => {
            (format!("Mihomo {}", i.text("dash.unknown")), Tone::Warning)
        }
        other => placeholder(app, other),
    };

    // GitHub Acceleration Proxy
    let (gh_text, gh_tone) = match &app.ctx.github_proxy {
        Some(url) => (format!("{on} {url}"), Tone::Success),
        None => (format!("{off} {}", i.text("dash.direct")), Tone::Muted),
    };

    if inner.height <= 2 {
        // Ultra-compact mobile summary (e.g. 40x16)
        let bbr_dot = if matches!(&local.congestion, Reading::Value(a) if a == "bbr") {
            on
        } else {
            off
        };
        let summary = format!("{} {bbr_dot}{sep}{proxy_text}", i.text("dash.row_bbr"));
        lines.push(Line::from(Span::styled(
            clip_ascii_safe(&summary, inner.width as usize, theme.flags.ascii),
            tokens::tone_style(theme, proxy_tone),
        )));
        if inner.height >= 2 {
            lines.push(fact_line(
                app,
                i.text("dash.ipv4"),
                &v4_text,
                v4_tone,
                inner.width as usize,
            ));
        }
    } else if inner.height <= 4 {
        // Compact mobile priority: BBR, IPv4, Mihomo Proxy, GitHub Proxy
        lines.push(fact_line(
            app,
            i.text("dash.row_bbr"),
            &bbr_text,
            bbr_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.ipv4"),
            &v4_text,
            v4_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_proxy"),
            &proxy_text,
            proxy_tone,
            inner.width as usize,
        ));
        if inner.height >= 4 {
            lines.push(fact_line(
                app,
                i.text("dash.row_github"),
                &gh_text,
                gh_tone,
                inner.width as usize,
            ));
        }
    } else if inner.height == 5 {
        // Medium-short mobile: BBR, TCP, IPv4, Proxy, GitHub
        lines.push(fact_line(
            app,
            i.text("dash.row_bbr"),
            &bbr_text,
            bbr_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_tcp"),
            &tcp_text,
            tcp_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.ipv4"),
            &v4_text,
            v4_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_proxy"),
            &proxy_text,
            proxy_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_github"),
            &gh_text,
            gh_tone,
            inner.width as usize,
        ));
    } else {
        lines.push(fact_line(
            app,
            i.text("dash.row_bbr"),
            &bbr_text,
            bbr_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_tcp"),
            &tcp_text,
            tcp_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.ipv4"),
            &v4_text,
            v4_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.ipv6"),
            &v6_text,
            v6_tone,
            inner.width as usize,
        ));
        if inner.height >= 8 {
            lines.push(Line::raw(""));
        }
        lines.push(fact_line(
            app,
            i.text("dash.row_proxy"),
            &proxy_text,
            proxy_tone,
            inner.width as usize,
        ));
        lines.push(fact_line(
            app,
            i.text("dash.row_github"),
            &gh_text,
            gh_tone,
            inner.width as usize,
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_quick_section(app: &App, frame: &mut Frame, geom: &OverviewGeometry) {
    if let Some(list_box) = geom.quick_list {
        draw_quick_list_card(app, frame, list_box, &geom.quick_cards);
        return;
    }

    let theme = &app.theme.active;
    let i = &app.ctx.i18n;

    if geom.quick_header.height > 0 {
        let buf = frame.buffer_mut();
        let canvas_bg = if theme.flags.paint_bg {
            tokens::color(theme, Role::Canvas)
        } else {
            Color::Reset
        };
        let quick_title = i.text("overview.quick_title");
        buf.set_stringn(
            geom.quick_header.x + 1,
            geom.quick_header.y,
            quick_title,
            geom.quick_header.width.saturating_sub(2) as usize,
            tokens::text(theme, Level::Tertiary)
                .bg(canvas_bg)
                .add_modifier(Modifier::BOLD),
        );
        let search_hint = format!("/ {}", i.text("key.search"));
        if quick_title.width() + search_hint.width() + 3 <= geom.quick_header.width as usize {
            tokens::put_right(
                buf,
                Rect::new(
                    geom.quick_header.x,
                    geom.quick_header.y,
                    geom.quick_header.width.saturating_sub(1),
                    1,
                ),
                &search_hint,
                tokens::fg(theme, Role::Accent).bg(canvas_bg),
            );
        }
    }

    let cards = app.quick_cards();
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    for (idx, (rect, card)) in geom.quick_cards.iter().zip(cards.iter()).enumerate() {
        if rect.width < 12 || rect.height < 2 {
            continue;
        }
        let selected = idx == app.quick_cursor;
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let bg_role = if selected {
            Role::Selection
        } else if hovered {
            Role::SurfaceAlt
        } else {
            Role::Surface
        };
        let border_role = if selected {
            Role::BorderFocus
        } else {
            Role::Border
        };
        tokens::draw_surface_card_fx(
            frame,
            *rect,
            theme,
            bg_role,
            border_role,
            false,
            elapsed,
            reduced,
        );

        let bg = if theme.flags.paint_bg || selected {
            tokens::color(theme, bg_role)
        } else {
            Color::Reset
        };
        let title_bg = if theme.style == UiStyle::Studio && !theme.flags.framed {
            tokens::card_title_bg(theme, bg_role)
        } else {
            bg
        };
        let buf = frame.buffer_mut();

        let top_y = if theme.style == UiStyle::Studio && !theme.flags.framed {
            rect.y
        } else if theme.style == UiStyle::Editorial && !theme.flags.framed {
            rect.y + 1
        } else {
            rect.y + u16::from(theme.flags.framed || rect.height >= 4)
        };
        let text_x = rect.x + 2;
        let inner_w = rect.width.saturating_sub(4) as usize;

        let badge_str = if card.badge.is_empty() {
            String::new()
        } else if theme.flags.ascii {
            if selected {
                format!("[{} {}]", card.badge, tokens::glyph(theme, Glyph::Enter))
            } else {
                format!("[{}]", card.badge)
            }
        } else if selected {
            format!(" {} {} ", card.badge, tokens::glyph(theme, Glyph::Enter))
        } else {
            format!(" {} ", card.badge)
        };
        let badge_w = if badge_str.is_empty() {
            0
        } else {
            badge_str.width() + 2
        };

        let title_budget = inner_w.saturating_sub(badge_w);
        let title_fg = if selected {
            tokens::selected_fg(theme)
        } else {
            tokens::color(theme, Role::Text1)
        };
        buf.set_stringn(
            text_x,
            top_y,
            clip_ascii_safe(&card.title, title_budget, theme.flags.ascii),
            title_budget,
            Style::default()
                .fg(title_fg)
                .bg(title_bg)
                .add_modifier(Modifier::BOLD),
        );

        if !badge_str.is_empty() {
            let (b_fg, b_bg) = if theme.flags.ascii {
                (
                    if selected {
                        tokens::selected_fg(theme)
                    } else {
                        tokens::color(theme, tokens::tone_role(card.badge_tone))
                    },
                    title_bg,
                )
            } else if selected && !theme.flags.solid_selection {
                (
                    tokens::color(theme, Role::OnAccent),
                    tokens::color(theme, Role::Accent),
                )
            } else if selected && theme.flags.solid_selection {
                (tokens::selected_fg(theme), title_bg)
            } else {
                (
                    tokens::color(theme, tokens::tone_role(card.badge_tone)),
                    if theme.flags.paint_bg && theme.style != UiStyle::Editorial {
                        tokens::color(theme, Role::SurfaceAlt)
                    } else {
                        title_bg
                    },
                )
            };
            tokens::put_right(
                buf,
                Rect::new(text_x, top_y, rect.width.saturating_sub(4), 1),
                &badge_str,
                Style::default()
                    .fg(b_fg)
                    .bg(b_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }

        if top_y + 1 < rect.bottom() {
            let sub_fg = if selected && theme.flags.solid_selection {
                tokens::selected_fg(theme)
            } else if selected {
                tokens::color(theme, Role::Text2)
            } else {
                tokens::color(theme, Role::Text3)
            };
            let sub_lines = super::wrap_words(&card.subtitle, inner_w);
            let max_sub = rect
                .bottom()
                .saturating_sub(top_y + 1 + u16::from(theme.flags.framed))
                as usize;
            for (li, line) in sub_lines.into_iter().take(max_sub.min(2)).enumerate() {
                buf.set_stringn(
                    text_x,
                    top_y + 1 + li as u16,
                    clip_ascii_safe(&line, inner_w, theme.flags.ascii),
                    inner_w,
                    Style::default().fg(sub_fg).bg(bg),
                );
            }
        }
    }
}

/// Unified single-column Quick Actions card used on mobile and short viewports.
fn draw_quick_list_card(app: &App, frame: &mut Frame, list_box: Rect, rows: &[Rect; 4]) {
    if list_box.width < 16 || list_box.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let framed_box = list_box.height >= 6;
    if framed_box {
        tokens::draw_surface_card(frame, list_box, theme, Role::Surface, Role::Border, false);
    } else {
        tokens::fill_surface(frame.buffer_mut(), list_box, theme, Role::Surface);
    }
    let base_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        Color::Reset
    };
    if framed_box {
        let search_hint = format!("/ {}", i.text("key.search"));
        draw_card_header(
            app,
            frame,
            list_box,
            i.text("overview.quick_title"),
            &search_hint,
            Role::Surface,
        );
    }

    let buf = frame.buffer_mut();
    let sep = format!(" {} ", tokens::glyph(theme, Glyph::Separator));
    let cards = app.quick_cards();
    for (idx, (rect, card)) in rows.iter().zip(cards.iter()).enumerate() {
        if rect.width < 10 || rect.height == 0 {
            continue;
        }
        let selected = idx == app.quick_cursor;
        let hovered = app.hover.is_some_and(|p| rect.contains(p));
        let row_bg = if selected {
            tokens::color(theme, Role::Selection)
        } else if hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            base_bg
        };
        for y in rect.top()..rect.bottom() {
            for col in rect.left()..rect.right() {
                buf[(col, y)].set_symbol(" ").set_bg(row_bg);
            }
            if selected {
                buf[(rect.x, y)]
                    .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                    .set_style(
                        Style::default()
                            .fg(tokens::selected_accent(theme))
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD),
                    );
            }
        }

        let badge_str = if card.badge.is_empty() {
            String::new()
        } else if theme.flags.ascii {
            if selected {
                format!("[{} {}]", card.badge, tokens::glyph(theme, Glyph::Enter))
            } else {
                format!("[{}]", card.badge)
            }
        } else if selected {
            format!(" {} {} ", card.badge, tokens::glyph(theme, Glyph::Enter))
        } else {
            format!(" {} ", card.badge)
        };
        let badge_w = if badge_str.is_empty() {
            0
        } else {
            badge_str.width() + 2
        };

        let text_x = rect.x + 2;
        let text_budget = rect.width.saturating_sub(3 + badge_w as u16) as usize;
        let title_fg = if selected {
            tokens::selected_fg(theme)
        } else {
            tokens::color(theme, Role::Text1)
        };
        let sub_fg = if selected && theme.flags.solid_selection {
            tokens::selected_fg(theme)
        } else if selected {
            tokens::color(theme, Role::Text2)
        } else {
            tokens::color(theme, Role::Text3)
        };

        let clipped_title = clip_ascii_safe(&card.title, text_budget, theme.flags.ascii);
        buf.set_stringn(
            text_x,
            rect.y,
            &clipped_title,
            text_budget,
            Style::default()
                .fg(title_fg)
                .bg(row_bg)
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );

        if rect.height >= 2 {
            let sub_budget = rect.width.saturating_sub(3) as usize;
            buf.set_stringn(
                text_x,
                rect.y + 1,
                clip_ascii_safe(&card.subtitle, sub_budget, theme.flags.ascii),
                sub_budget,
                Style::default().fg(sub_fg).bg(row_bg),
            );
        } else {
            let title_used = clipped_title.width();
            let remain = text_budget.saturating_sub(title_used);
            if remain >= sep.width() + 10 && !card.subtitle.is_empty() {
                let inline = format!("{sep}{}", card.subtitle);
                buf.set_stringn(
                    text_x + title_used as u16,
                    rect.y,
                    clip_ascii_safe(&inline, remain, theme.flags.ascii),
                    remain,
                    Style::default().fg(sub_fg).bg(row_bg),
                );
            }
        }

        if !badge_str.is_empty() {
            let (b_fg, b_bg) = if theme.flags.ascii {
                (
                    if selected {
                        tokens::selected_fg(theme)
                    } else {
                        tokens::color(theme, tokens::tone_role(card.badge_tone))
                    },
                    row_bg,
                )
            } else if selected && !theme.flags.solid_selection {
                (
                    tokens::color(theme, Role::OnAccent),
                    tokens::color(theme, Role::Accent),
                )
            } else if selected && theme.flags.solid_selection {
                (tokens::selected_fg(theme), row_bg)
            } else {
                (
                    tokens::color(theme, tokens::tone_role(card.badge_tone)),
                    if theme.flags.paint_bg {
                        tokens::color(theme, Role::SurfaceAlt)
                    } else {
                        row_bg
                    },
                )
            };
            tokens::put_right(
                buf,
                Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), 1),
                &badge_str,
                Style::default()
                    .fg(b_fg)
                    .bg(b_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

fn pad_cells(label: &str, cells: usize) -> String {
    let w = label.width();
    if w >= cells {
        format!("{label} ")
    } else {
        format!("{label}{}", " ".repeat(cells - w))
    }
}

fn short_age(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn usage_detail_text(reading: &Reading<Usage>) -> Option<String> {
    match reading {
        Reading::Value(u) if u.total > 0 => Some(format!(
            "{:.1}/{:.1}G",
            u.used as f64 / GIB,
            u.total as f64 / GIB
        )),
        _ => None,
    }
}

fn right_pad_detail(text: &str, col_w: usize, ascii: bool) -> String {
    let clipped = clip_ascii_safe(text, col_w, ascii);
    let pad = col_w.saturating_sub(clipped.width());
    format!("{}{clipped}", " ".repeat(pad))
}

#[allow(clippy::too_many_arguments)]
fn cpu_row(
    app: &App,
    label: &str,
    label_w: usize,
    reading: &Reading<f32>,
    tweened: Option<f64>,
    cores_text: &str,
    bar_w: usize,
    detail_col_w: usize,
    elapsed: std::time::Duration,
    reduced: bool,
    shimmer_col: Option<usize>,
) -> Line<'static> {
    let theme = &app.theme.active;
    let lbl = pad_cells(
        &clip_ascii_safe(label, label_w.saturating_sub(1), theme.flags.ascii),
        label_w,
    );
    let mut spans = vec![Span::styled(lbl, tokens::text(theme, Level::Secondary))];
    if let Some(pct) = tweened {
        spans.extend(tokens::gauge_spans_fx(
            pct,
            bar_w,
            theme,
            elapsed,
            reduced,
            shimmer_col,
        ));
        let pct_str = format!(" {pct:>3.0}%");
        spans.push(Span::styled(
            pct_str,
            Style::default()
                .fg(fx::meter_pct_color(pct, theme, elapsed, reduced))
                .add_modifier(Modifier::BOLD),
        ));
        if detail_col_w > 0 {
            spans.push(Span::raw("  "));
            let sep_str = format!(" {} ", tokens::glyph(theme, Glyph::Separator));
            let spark_room = detail_col_w.saturating_sub(
                cores_text.width()
                    + if cores_text.is_empty() {
                        0
                    } else {
                        sep_str.width()
                    },
            );
            if !app.snapshot.cpu_history.is_empty() && spark_room >= 4 {
                let spark_w = spark_room.min(8);
                let total_w = cores_text.width()
                    + if cores_text.is_empty() {
                        0
                    } else {
                        sep_str.width()
                    }
                    + spark_w;
                let left_pad = detail_col_w.saturating_sub(total_w);
                if left_pad > 0 {
                    spans.push(Span::raw(" ".repeat(left_pad)));
                }
                if !cores_text.is_empty() {
                    spans.push(Span::styled(
                        cores_text.to_owned(),
                        tokens::text(theme, Level::Tertiary),
                    ));
                    spans.push(Span::styled(sep_str, tokens::text(theme, Level::Tertiary)));
                }
                spans.push(tokens::sparkline(&app.snapshot.cpu_history, spark_w, theme));
            } else if !cores_text.is_empty() {
                spans.push(Span::styled(
                    right_pad_detail(cores_text, detail_col_w, theme.flags.ascii),
                    tokens::text(theme, Level::Tertiary),
                ));
            }
        }
    } else {
        let (ph, tone) = placeholder(app, reading);
        spans.push(Span::styled(ph, tokens::tone_style(theme, tone)));
    }
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
fn usage_row(
    app: &App,
    label: &str,
    label_w: usize,
    reading: &Reading<Usage>,
    tweened: Option<f64>,
    detail: Option<&str>,
    bar_w: usize,
    detail_col_w: usize,
    elapsed: std::time::Duration,
    reduced: bool,
    shimmer_col: Option<usize>,
) -> Line<'static> {
    let theme = &app.theme.active;
    let lbl = pad_cells(
        &clip_ascii_safe(label, label_w.saturating_sub(1), theme.flags.ascii),
        label_w,
    );
    let mut spans = vec![Span::styled(lbl, tokens::text(theme, Level::Secondary))];
    match (reading, tweened) {
        (Reading::Value(u), Some(pct)) if u.total > 0 => {
            spans.extend(tokens::gauge_spans_fx(
                pct,
                bar_w,
                theme,
                elapsed,
                reduced,
                shimmer_col,
            ));
            let pct_str = format!(" {pct:>3.0}%");
            spans.push(Span::styled(
                pct_str,
                Style::default()
                    .fg(fx::meter_pct_color(pct, theme, elapsed, reduced))
                    .add_modifier(Modifier::BOLD),
            ));
            if detail_col_w > 0
                && let Some(text) = detail
            {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    right_pad_detail(text, detail_col_w, theme.flags.ascii),
                    tokens::text(theme, Level::Tertiary),
                ));
            }
        }
        (Reading::Value(_), _) => {
            spans.push(Span::styled(
                app.ctx.i18n.text("status.disabled").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ));
        }
        (other, _) => {
            let (ph, tone) = placeholder(app, other);
            spans.push(Span::styled(ph, tokens::tone_style(theme, tone)));
        }
    }
    Line::from(spans)
}

fn fact_line(app: &App, label: &str, value: &str, tone: Tone, width: usize) -> Line<'static> {
    fact_line_with_width(app, label, 6, value, tone, width)
}

fn fact_line_with_width(
    app: &App,
    label: &str,
    label_w: usize,
    value: &str,
    tone: Tone,
    width: usize,
) -> Line<'static> {
    let theme = &app.theme.active;
    let lbl = pad_cells(
        &clip_ascii_safe(label, label_w.saturating_sub(1), theme.flags.ascii),
        label_w,
    );
    let val_budget = width.saturating_sub(lbl.width());
    Line::from(vec![
        Span::styled(lbl, tokens::text(theme, Level::Tertiary)),
        Span::styled(
            clip_ascii_safe(value, val_budget, theme.flags.ascii),
            tokens::tone_style(theme, tone),
        ),
    ])
}

fn placeholder<T>(app: &App, reading: &Reading<T>) -> (String, Tone) {
    let i = &app.ctx.i18n;
    match reading {
        Reading::Loading => (
            format!("{} {}", app.spin(), i.text("dash.loading")),
            Tone::Info,
        ),
        Reading::NotQueried => (i.text("dash.not_queried").into(), Tone::Muted),
        Reading::NotApplicable => (i.text("dash.na").into(), Tone::Muted),
        Reading::Unavailable(_) | Reading::Value(_) => {
            (i.text("dash.unknown").into(), Tone::Warning)
        }
    }
}

fn format_uptime(seconds: u64) -> String {
    format!(
        "{}d {:02}h {:02}m",
        seconds / 86400,
        seconds % 86400 / 3600,
        seconds % 3600 / 60
    )
}
