//! Top-level renderer and shared drawing primitives for the dedicated Mihomo workspace.

mod connections;
pub(in crate::tui) mod modals;
mod overview;
mod profiles;
mod proxies;
mod rules;
mod tools;
pub(in crate::tui) mod traffic;

use super::layout::{ClashBodyGeom, ClashGeometry};
use crate::{
    ops::clash::model::{ClashAvailability, ClashTab, format_speed_compact},
    theme::{Level, Role, Theme},
    tui::{
        input::InputBuffer,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
            wrap_words,
        },
        state::{App, Tone},
    },
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

pub(in crate::tui) fn draw(app: &App, frame: &mut Frame, geom: &ClashGeometry) {
    let theme = &app.theme.active;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();

    tokens::draw_surface_card_fx(
        frame,
        geom.area,
        theme,
        Role::Surface,
        Role::Border,
        false,
        elapsed,
        reduced,
    );
    let bg = surface_bg(theme, Role::Surface);

    draw_header_and_tabs(app, frame, geom, bg);

    match &geom.body {
        ClashBodyGeom::TrafficFullscreen(tf) => traffic::draw_fullscreen_atlas(app, frame, tf),
        ClashBodyGeom::Fullscreen(full) => {
            if app.clash.tools_log_fullscreen {
                tools::draw_logs_panel(app, frame, *full, true);
            } else {
                connections::draw_fullscreen_inspector(app, frame, *full);
            }
        }
        ClashBodyGeom::Unavailable { body, cta } => {
            draw_unavailable_guidance(app, frame, *body, *cta, bg);
        }
        ClashBodyGeom::Overview(ov) => overview::draw(app, frame, ov),
        ClashBodyGeom::Proxies(px) => proxies::draw(app, frame, px),
        ClashBodyGeom::Profiles(pf) => profiles::draw(app, frame, pf),
        ClashBodyGeom::Connections(cn) => connections::draw(app, frame, cn),
        ClashBodyGeom::Rules(rl) => rules::draw(app, frame, rl),
        ClashBodyGeom::Tools(tl) => tools::draw(app, frame, tl),
    }
}

fn draw_header_and_tabs(app: &App, frame: &mut Frame, geom: &ClashGeometry, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let buf = frame.buffer_mut();

    if geom.area.width >= 20 {
        let title_bg = tokens::card_title_bg(theme, Role::Surface);
        let raw_title = if cl.fullscreen {
            format!("Mihomo [{}]", i.text("docker.badge.focus"))
        } else {
            "Mihomo".to_owned()
        };
        let title = tokens::format_card_title(theme, &raw_title, theme.flags.framed);
        let max_t = geom.area.width.saturating_sub(20) as usize;
        buf.set_stringn(
            geom.area.x + 2,
            geom.area.y,
            clip_ascii_safe(&title, max_t, ascii),
            max_t,
            Style::default()
                .fg(tokens::card_title_fg(theme, false))
                .bg(title_bg)
                .add_modifier(Modifier::BOLD),
        );

        // Right-aligned live badge: mode + version + speed when running
        if cl.snapshot.availability.is_ready() {
            let sep = tokens::glyph(theme, Glyph::Separator);
            let mode = cl.snapshot.summary.mode.to_ascii_uppercase();
            let ver = if cl.snapshot.summary.version.is_empty() {
                String::new()
            } else {
                cl.snapshot.summary.version.clone()
            };
            let up_sym = if ascii { "^" } else { "↑" };
            let dn_sym = if ascii { "v" } else { "↓" };
            let meta_core = if geom.area.width >= 66 {
                let speed = format!(
                    "{dn_sym}{} {up_sym}{}",
                    format_speed_compact(cl.snapshot.down_speed),
                    format_speed_compact(cl.snapshot.up_speed)
                );
                if ver.is_empty() {
                    format!("{mode} {sep} {speed}")
                } else {
                    format!("{mode} {sep} {ver} {sep} {speed}")
                }
            } else if !ver.is_empty() {
                format!("{mode} {sep} {ver}")
            } else {
                mode
            };
            let meta = if theme.flags.framed {
                format!(" {meta_core} ")
            } else {
                meta_core
            };
            let title_w = title.width();
            let meta_w = meta.width();
            if title_w + meta_w + 6 <= geom.area.width as usize {
                tokens::put_right(
                    buf,
                    Rect::new(
                        geom.area.x + 2,
                        geom.area.y,
                        geom.area.width.saturating_sub(4),
                        1,
                    ),
                    &meta,
                    tokens::text(theme, Level::Tertiary).bg(title_bg),
                );
            }
        }
    }

    if geom.tab_overflow_left {
        let sym = if ascii { "<" } else { "‹" };
        buf.set_stringn(
            geom.tab_bar.x,
            geom.tab_bar.y,
            sym,
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }
    if geom.tab_overflow_right {
        let sym = if ascii { ">" } else { "›" };
        buf.set_stringn(
            geom.tab_bar.right().saturating_sub(1),
            geom.tab_bar.y,
            sym,
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }

    for pill in &geom.tab_pills {
        let active = pill.tab == cl.tab;
        let hovered = app.hover.is_some_and(|p| pill.rect.contains(p));
        let body = if pill.count.is_empty() {
            format!(" {} ", pill.label)
        } else {
            format!(" {} {} ", pill.label, pill.count)
        };
        let style = if active {
            Style::default()
                .fg(tokens::color(theme, Role::OnAccent))
                .bg(tokens::color(theme, Role::Accent))
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            tokens::text(theme, Level::Primary)
                .bg(tokens::color(theme, Role::SurfaceAlt))
                .add_modifier(Modifier::BOLD)
        } else {
            tokens::text(theme, Level::Secondary).bg(bg)
        };
        fill_rect_bg(buf, pill.rect, style.bg.unwrap_or(bg));
        buf.set_stringn(
            pill.rect.x,
            pill.rect.y,
            clip_ascii_safe(&body, pill.rect.width as usize, ascii),
            pill.rect.width as usize,
            style,
        );
    }

    if let Some(btn) = geom.focus_btn {
        let lbl = if cl.fullscreen {
            format!(" [z] {} ", i.text("docker.key.unfocus"))
        } else {
            format!(" [z] {} ", i.text("docker.key.focus"))
        };
        let hovered = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, cl.fullscreen, hovered, theme, bg);
    }

    // Horizontal divider below tabs
    if geom.tab_rule_y < geom.area.bottom() {
        let rule_sym = tokens::glyph(theme, Glyph::Rule);
        let rx = geom.area.x + 1;
        let rw = geom.area.width.saturating_sub(2) as usize;
        if rw > 0 {
            buf.set_stringn(
                rx,
                geom.tab_rule_y,
                rule_sym.repeat(rw),
                rw,
                tokens::fg(theme, Role::Rule).bg(bg),
            );
        }
    }
}

fn draw_unavailable_guidance(
    app: &App,
    frame: &mut Frame,
    body: Rect,
    cta: Option<Rect>,
    bg: Color,
) {
    if body.width < 16 || body.height < 4 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let avail = &app.clash.snapshot.availability;

    let card_w = body
        .width
        .saturating_sub(if body.width < 48 { 2 } else { 6 })
        .min(68)
        .max(20.min(body.width));
    let card_h = 9u16
        .min(body.height.saturating_sub(2))
        .max(6.min(body.height));
    let card_x = body.x + (body.width.saturating_sub(card_w)) / 2;
    let card_y = body.y + (body.height.saturating_sub(card_h)) / 2;
    let card = Rect::new(card_x, card_y, card_w, card_h);

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
    let cbg = surface_bg(theme, Role::SurfaceAlt);
    let buf = frame.buffer_mut();

    let (title, detail, cta_label, tone) = match avail {
        ClashAvailability::Loading => (
            format!("{} {}", app.spin(), i.text("clash.avail.loading_title")),
            i.text("clash.avail.loading_desc").to_owned(),
            None,
            Tone::Info,
        ),
        ClashAvailability::NotInstalled => (
            i.text("clash.avail.not_installed_title").to_owned(),
            i.text("clash.avail.not_installed_desc").to_owned(),
            Some(i.text("clash.avail.cta_install")),
            Tone::Warning,
        ),
        ClashAvailability::Stopped => (
            i.text("clash.avail.stopped_title").to_owned(),
            i.text("clash.avail.stopped_desc").to_owned(),
            Some(i.text("clash.avail.cta_start")),
            Tone::Warning,
        ),
        ClashAvailability::ControllerUnconfigured => (
            i.text("clash.avail.no_ctrl_title").to_owned(),
            i.text("clash.avail.no_ctrl_desc").to_owned(),
            Some(i.text("clash.avail.cta_configure")),
            Tone::Warning,
        ),
        ClashAvailability::ControllerUnreachable(err) => (
            i.text("clash.avail.unreachable_title").to_owned(),
            err.clone(),
            None,
            Tone::Danger,
        ),
        ClashAvailability::UnknownProcess => (
            i.text("clash.avail.unknown_title").to_owned(),
            i.text("clash.avail.unknown_desc").to_owned(),
            None,
            Tone::Danger,
        ),
        ClashAvailability::Ready => return,
    };

    let ix = card.x + 2;
    let iw = card.width.saturating_sub(4) as usize;
    if iw == 0 {
        return;
    }
    buf.set_stringn(
        ix,
        card.y + 1,
        clip_ascii_safe(&title, iw, ascii),
        iw,
        tokens::tone_style(theme, tone)
            .bg(cbg)
            .add_modifier(Modifier::BOLD),
    );

    let max_lines = card.height.saturating_sub(4) as usize;
    for (idx, line) in wrap_words(&detail, iw)
        .into_iter()
        .take(max_lines)
        .enumerate()
    {
        buf.set_stringn(
            ix,
            card.y + 2 + idx as u16,
            clip_ascii_safe(&line, iw, ascii),
            iw,
            tokens::text(theme, Level::Secondary).bg(cbg),
        );
    }

    if let (Some(btn), Some(lbl)) = (cta, cta_label) {
        let text = format!(" [Enter] {lbl} ");
        let hovered = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &text, true, hovered, theme, cbg);
    } else if card.height >= 5 {
        let hint = i.text("clash.avail.offline_tabs_hint");
        buf.set_stringn(
            ix,
            card.bottom().saturating_sub(2),
            clip_ascii_safe(hint, iw, ascii),
            iw,
            tokens::text(theme, Level::Tertiary).bg(cbg),
        );
    }
    let _ = bg;
}

// ── Shared rendering primitives ─────────────────────────────────────────────────────────────────

pub(super) fn surface_bg(theme: &Theme, role: Role) -> Color {
    if theme.flags.paint_bg {
        tokens::color(theme, role)
    } else {
        Color::Reset
    }
}

pub(super) fn fill_rect_bg(buf: &mut Buffer, rect: Rect, bg: Color) {
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            buf[(x, y)].set_bg(bg);
        }
    }
}

pub(super) fn draw_pill_chip(
    buf: &mut Buffer,
    rect: Rect,
    label: &str,
    active: bool,
    hovered: bool,
    theme: &Theme,
    parent_bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let style = if active {
        Style::default()
            .fg(tokens::color(theme, Role::OnAccent))
            .bg(tokens::color(theme, Role::Accent))
            .add_modifier(Modifier::BOLD)
    } else if hovered {
        tokens::text(theme, Level::Primary)
            .bg(tokens::color(theme, Role::Selection))
            .add_modifier(Modifier::BOLD)
    } else {
        tokens::text(theme, Level::Secondary).bg(tokens::color(theme, Role::SurfaceAlt))
    };
    let chip_bg = style.bg.unwrap_or(parent_bg);
    fill_rect_bg(buf, rect, chip_bg);
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(label, rect.width as usize, theme.flags.ascii),
        rect.width as usize,
        style,
    );
}

pub(super) fn draw_inline_filter(
    buf: &mut Buffer,
    rect: Rect,
    filter: &InputBuffer,
    filtering: bool,
    placeholder: &str,
    theme: &Theme,
    bg: Color,
) {
    if rect.width < 6 || rect.height == 0 {
        return;
    }
    let w = rect.width as usize;
    let ascii = theme.flags.ascii;
    if filtering {
        buf.set_stringn(
            rect.x,
            rect.y,
            "/ ",
            w,
            tokens::fg(theme, Role::Accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        let vw = w.saturating_sub(2);
        if vw > 0 {
            let (shown, cur_col) = filter.visible(vw);
            let vx = rect.x + 2;
            buf.set_stringn(
                vx,
                rect.y,
                &shown,
                vw,
                tokens::fg(theme, Role::Accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let cx = vx + cur_col.min(vw.saturating_sub(1) as u16);
            if cx < rect.right() {
                let cell = &mut buf[(cx, rect.y)];
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
    } else if !filter.text.is_empty() {
        let text = format!("/ {} (Esc)", filter.text);
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&text, w, ascii),
            w,
            tokens::fg(theme, Role::Accent).bg(bg),
        );
    } else {
        let text = format!("/ {placeholder}");
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&text, w, ascii),
            w,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }
}

pub(super) fn draw_progress_bar(
    buf: &mut Buffer,
    bar: Rect,
    ratio: f64,
    fill_role: Role,
    theme: &Theme,
    bg: Color,
) {
    if bar.width == 0 || bar.height == 0 {
        return;
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = ((bar.width as f64) * ratio).round() as u16;
    let filled = filled.min(bar.width);
    let (on_ch, off_ch) = if theme.flags.ascii {
        ('#', '-')
    } else {
        ('━', '─')
    };
    let on_style = tokens::fg(theme, fill_role).bg(bg);
    let off_style = tokens::fg(theme, Role::Rule).bg(bg);
    for col in 0..bar.width {
        let cell = &mut buf[(bar.x + col, bar.y)];
        if col < filled {
            cell.set_char(on_ch).set_style(on_style);
        } else {
            cell.set_char(off_ch).set_style(off_style);
        }
    }
}

pub(super) fn delay_role(delay_ms: Option<u64>, tested: bool) -> Role {
    match delay_ms {
        Some(ms) if ms < 180 => Role::Success,
        Some(ms) if ms < 450 => Role::Warning,
        Some(_) => Role::Danger,
        None if tested => Role::Danger,
        None => Role::Text3,
    }
}

pub(super) fn draw_card_title(
    buf: &mut Buffer,
    card: Rect,
    title: &str,
    right_meta: &str,
    theme: &Theme,
    bg_role: Role,
) {
    if card.width < 8 || card.height < 2 {
        return;
    }
    let bg = tokens::card_title_bg(theme, bg_role);
    let budget = card.width.saturating_sub(4) as usize;
    let label = tokens::format_card_title(theme, title, theme.flags.framed);
    let clipped = clip_ascii_safe(&label, budget, theme.flags.ascii);
    buf.set_stringn(
        card.x + 2,
        card.y,
        &clipped,
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
        if clipped.width() + meta.width() + 2 <= budget {
            tokens::put_right(
                buf,
                Rect::new(card.x + 2, card.y, card.width.saturating_sub(4), 1),
                &meta,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
    }
}

pub(in crate::tui) fn footer_pairs(app: &App) -> Vec<(&'static str, &str)> {
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let mobile = app.viewport.width < 80;
    if cl.filtering {
        return vec![("↵", i.text("key.confirm")), ("Esc", i.text("ui.cancel"))];
    }
    if cl.traffic_fullscreen {
        let pause_lbl = if cl.is_traffic_paused() {
            i.text("clash.traffic.resume")
        } else {
            i.text("clash.traffic.pause")
        };
        return vec![
            ("g", i.text("clash.key.graph")),
            ("t", i.text("clash.key.window")),
            ("y", i.text("clash.key.scale")),
            ("Space", pause_lbl),
            ("b", i.text("clash.key.breakdown")),
            ("Esc", i.text("key.close")),
        ];
    }
    if cl.tools_log_fullscreen {
        return vec![
            ("↑↓", i.text("key.scroll")),
            ("/", i.text("key.search")),
            ("y", i.text("clash.key.copy")),
            ("r", i.text("clash.key.refresh")),
            ("Esc", i.text("key.close")),
        ];
    }
    if cl.conn_inspector_fullscreen {
        return vec![
            ("↑↓", i.text("key.scroll")),
            ("x", i.text("clash.key.close_conn")),
            ("Esc", i.text("key.close")),
        ];
    }
    let tab_key = if cl.fullscreen {
        "1-6/Tab"
    } else {
        "{}/Alt+1-6"
    };
    let zoom_lbl = if cl.fullscreen {
        i.text("docker.key.unfocus")
    } else {
        i.text("docker.key.focus")
    };
    match cl.tab {
        ClashTab::Overview => {
            if mobile {
                vec![
                    ("v", i.text("clash.key.traffic_atlas")),
                    ("g/t", i.text("clash.key.graph")),
                    ("m", i.text("clash.key.mode")),
                    ("i/N", i.text("clash.key.probe_exit")),
                    ("?", i.text("key.help")),
                ]
            } else {
                vec![
                    (tab_key, i.text("clash.key.tabs")),
                    ("v", i.text("clash.key.traffic_atlas")),
                    ("g/t/y", i.text("clash.key.graph")),
                    ("m", i.text("clash.key.mode")),
                    ("s/R", i.text("clash.key.stop_reload")),
                    ("i/N", i.text("clash.key.probe_exit")),
                    ("z", zoom_lbl),
                    ("?", i.text("key.help")),
                ]
            }
        }
        ClashTab::Proxies => {
            if mobile {
                vec![
                    ("↵", i.text("clash.key.select")),
                    ("t", i.text("clash.key.test_group")),
                    ("o", i.text("clash.key.sort")),
                    ("g", i.text("clash.key.groups")),
                    ("?", i.text("key.help")),
                ]
            } else {
                vec![
                    (",./g", i.text("clash.key.groups")),
                    ("↵", i.text("clash.key.select")),
                    ("t/p", i.text("clash.key.test_group")),
                    ("f", i.text("clash.key.fastest")),
                    ("o/O", i.text("clash.key.sort")),
                    ("h", i.text("clash.key.hide_timeout")),
                    ("/", i.text("key.search")),
                    ("?", i.text("key.help")),
                ]
            }
        }
        ClashTab::Profiles => vec![
            ("↵", i.text("clash.key.use_reload")),
            ("u/U", i.text("clash.key.update")),
            ("a/i", i.text("clash.key.add")),
            ("d", i.text("clash.key.delete")),
            ("P", i.text("clash.key.providers")),
            ("z", zoom_lbl),
            ("?", i.text("key.help")),
        ],
        ClashTab::Connections => vec![
            ("a", i.text("clash.key.scope")),
            (",./f", i.text("clash.key.facet")),
            ("o", i.text("clash.key.sort")),
            ("↵", i.text("clash.key.inspect")),
            ("x/X", i.text("clash.key.close_conn")),
            ("/", i.text("key.search")),
            ("?", i.text("key.help")),
        ],
        ClashTab::Rules => vec![
            (",./f", i.text("clash.key.facet")),
            ("P", i.text("clash.key.rule_providers")),
            ("/", i.text("key.search")),
            ("r", i.text("clash.key.refresh")),
            ("?", i.text("key.help")),
        ],
        ClashTab::Tools => {
            if cl.tools_log_mode {
                vec![
                    ("↑↓", i.text("key.scroll")),
                    ("/", i.text("key.search")),
                    ("y", i.text("clash.key.copy")),
                    ("r", i.text("clash.key.refresh")),
                    ("f", i.text("clash.key.fullscreen")),
                    ("l", i.text("clash.tools.back_list")),
                    ("?", i.text("key.help")),
                ]
            } else {
                vec![
                    ("↵", i.text("key.open")),
                    ("l", i.text("clash.tools.view_logs")),
                    ("Shift+S", i.text("clash.key.socks_card")),
                    ("/", i.text("key.search")),
                    ("z", zoom_lbl),
                    ("?", i.text("key.help")),
                ]
            }
        }
    }
}
