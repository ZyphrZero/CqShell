//! Top-level renderer for the dedicated Docker management workspace.

mod inspector;
mod lists;
mod logs;
pub(in crate::tui) mod modals;

use super::{
    layout::DockerGeometry,
    state::{ComposeSubTab, ContainerSubTab},
};
use crate::{
    ops::docker::model::{ContainerState, DockerAvailability, DockerTab, format_bytes},
    theme::{Level, Role, Theme},
    tui::{
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

pub(in crate::tui) fn draw(app: &App, frame: &mut Frame, geom: &DockerGeometry) {
    app.docker.last_cmd_rect.set(None);
    let theme = &app.theme.active;
    let dk = &app.docker;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();

    // Outer Docker workspace surface card
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

    // Top title / engine summary / tab pill bar
    draw_header_and_tabs(app, frame, geom, bg);

    // Fullscreen log or fullscreen inspect overlay inside the workspace
    if let Some(full) = geom.fullscreen_view {
        if dk.logs.fullscreen {
            logs::draw_logs_panel(app, frame, full, true);
        } else {
            inspector::draw_fullscreen_inspector(app, frame, full);
        }
        return;
    }

    // Empty / unavailable guidance screen (except on Daemon tab, which stays usable for install/config)
    if dk.tab != DockerTab::Daemon && !dk.snapshot.availability.is_ready() {
        let body_y = (geom.tab_rule_y + 1).min(geom.area.bottom());
        let body_h = geom
            .area
            .bottom()
            .saturating_sub(body_y + u16::from(theme.flags.framed));
        let body = Rect::new(
            geom.area.x + 1,
            body_y,
            geom.area.width.saturating_sub(2),
            body_h,
        );
        draw_unavailable_guidance(app, frame, body, geom.empty_cta, bg);
        return;
    }

    // Split workspace: Left resource table + Right rich inspector
    lists::draw(app, frame, geom);

    if geom.inspector_area.width >= 16 && geom.inspector_area.height >= 4 {
        inspector::draw(app, frame, geom);
    }
}

fn draw_header_and_tabs(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let dk = &app.docker;
    let buf = frame.buffer_mut();

    // Card header along row 0 (works across all 8 UI styles: framed, Bento, Studio, Editorial)
    if geom.area.width >= 20 {
        let title_bg = tokens::card_title_bg(theme, Role::Surface);
        let raw_title = if dk.fullscreen {
            format!("Docker [{}]", i.text("docker.badge.focus"))
        } else {
            "Docker".to_owned()
        };
        let title = tokens::format_card_title(theme, &raw_title, theme.flags.framed);
        buf.set_stringn(
            geom.area.x + 2,
            geom.area.y,
            clip_ascii_safe(
                &title,
                geom.area.width.saturating_sub(20) as usize,
                theme.flags.ascii,
            ),
            geom.area.width.saturating_sub(20) as usize,
            Style::default()
                .fg(tokens::card_title_fg(theme, false))
                .bg(title_bg)
                .add_modifier(Modifier::BOLD),
        );

        // Right-aligned engine version & disk usage badge on header row
        if let Some(eng) = &dk.snapshot.engine
            && !eng.server_version.is_empty()
        {
            let sep = tokens::glyph(theme, Glyph::Separator);
            let total_df = dk.snapshot.total_disk_bytes();
            let meta_core = if total_df > 0 && geom.area.width >= 48 {
                format!("v{} {sep} {}", eng.server_version, format_bytes(total_df))
            } else {
                format!("v{}", eng.server_version)
            };
            let meta = if theme.flags.framed {
                format!(" {meta_core} ")
            } else {
                meta_core
            };
            let title_w = unicode_width::UnicodeWidthStr::width(title.as_str());
            let meta_w = unicode_width::UnicodeWidthStr::width(meta.as_str());
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

    // Tab pills row
    if geom.tab_overflow_left {
        let sym = if theme.flags.ascii { "<" } else { "‹" };
        buf.set_stringn(
            geom.tab_bar.x,
            geom.tab_bar.y,
            sym,
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }
    if geom.tab_overflow_right {
        let sym = if theme.flags.ascii { ">" } else { "›" };
        buf.set_stringn(
            geom.tab_bar.right().saturating_sub(1),
            geom.tab_bar.y,
            sym,
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }

    for pill in &geom.tab_pills {
        let active = pill.tab == dk.tab;
        let hovered = app.hover.is_some_and(|p| pill.rect.contains(p));
        let body = if pill.count.is_empty() {
            pill.label.clone()
        } else {
            format!("{} {}", pill.label, pill.count)
        };
        draw_pill_chip(buf, pill.rect, &body, active, hovered, theme, bg);
    }

    // Focus toggle button at top right
    if let Some(frect) = geom.focus_btn {
        let hovered = app.hover.is_some_and(|p| frect.contains(p));
        let txt = if dk.fullscreen {
            format!("[z] {}", i.text("docker.key.unfocus"))
        } else {
            format!("[z] {}", i.text("docker.key.focus"))
        };
        draw_pill_chip(buf, frect, &txt, dk.fullscreen, hovered, theme, bg);
    }

    // Divider rule below tabs
    tokens::draw_rule(
        buf,
        geom.area.x + 2,
        geom.tab_rule_y,
        geom.area.width.saturating_sub(4),
        theme,
        bg,
    );
}

fn draw_unavailable_guidance(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    cta_rect: Option<Rect>,
    bg: Color,
) {
    if area.width < 20 || area.height < 6 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let (title, desc, detail, tone, cta_label) = match &app.docker.snapshot.availability {
        DockerAvailability::Loading => (
            format!("{} {}", app.spin(), i.text("docker.empty.loading_title")),
            i.text("docker.empty.loading_desc").to_owned(),
            None,
            Tone::Info,
            None,
        ),
        DockerAvailability::NotInstalled => (
            i.text("docker.empty.not_installed_title").to_owned(),
            i.text("docker.empty.not_installed_desc").to_owned(),
            None,
            Tone::Warning,
            Some(i.text("docker.empty.install_cta")),
        ),
        DockerAvailability::DaemonStopped(diag) => (
            i.text("docker.empty.stopped_title").to_owned(),
            i.text("docker.empty.stopped_desc").to_owned(),
            Some(diag.clone()),
            Tone::Warning,
            Some(i.text("docker.empty.start_cta")),
        ),
        DockerAvailability::PermissionDenied(diag) => (
            i.text("docker.empty.permission_title").to_owned(),
            format!(
                "{} {}",
                i.text("docker.empty.permission_desc"),
                i.text("docker.empty.permission_hint")
            ),
            Some(diag.clone()),
            Tone::Danger,
            None,
        ),
        DockerAvailability::NotApplicable(os) => (
            i.text("docker.empty.na_title").to_owned(),
            i.text("docker.empty.na_desc").to_owned(),
            Some(os.clone()),
            Tone::Muted,
            None,
        ),
        DockerAvailability::Unavailable(diag) => (
            i.text("docker.empty.error_title").to_owned(),
            diag.clone(),
            None,
            Tone::Danger,
            None,
        ),
        DockerAvailability::Ready => return,
    };

    let card_w = area
        .width
        .saturating_sub(if area.width < 48 { 2 } else { 6 })
        .min(68)
        .max(20.min(area.width));
    let card_h = 9u16
        .min(area.height.saturating_sub(2))
        .max(6.min(area.height));
    let card_x = area.x + (area.width.saturating_sub(card_w)) / 2;
    let card_y = area.y + (area.height.saturating_sub(card_h)) / 2;
    let card = Rect::new(card_x, card_y, card_w, card_h);

    tokens::draw_surface_card(frame, card, theme, Role::Elevated, Role::Border, true);
    let cbg = surface_bg(theme, Role::Elevated);
    let buf = frame.buffer_mut();

    let inner_x = card.x + 2;
    let inner_w = card.width.saturating_sub(4) as usize;
    let mut y = card.y + 1;

    buf.set_stringn(
        inner_x,
        y,
        clip_ascii_safe(&title, inner_w, ascii),
        inner_w,
        tokens::tone_style(theme, tone)
            .bg(cbg)
            .add_modifier(Modifier::BOLD),
    );
    y += 2;

    for line in wrap_words(&desc, inner_w).into_iter().take(3) {
        if y >= card.bottom().saturating_sub(1) {
            break;
        }
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&line, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Secondary).bg(cbg),
        );
        y += 1;
    }

    if let Some(d) = detail
        && y < card.bottom().saturating_sub(2)
    {
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&d, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Tertiary).bg(cbg),
        );
    }

    if let (Some(btn), Some(lbl)) = (cta_rect, cta_label) {
        let hovered = app.hover.is_some_and(|p| btn.contains(p));
        let label = format!("{} {lbl}", tokens::glyph(theme, Glyph::Enter));
        let _ = bg;
        draw_pill_chip(buf, btn, &label, true, hovered, theme, cbg);
    }
}

pub(super) fn surface_bg(theme: &Theme, role: Role) -> Color {
    if theme.flags.paint_bg || matches!(role, Role::Elevated | Role::Selection) {
        tokens::color(theme, role)
    } else {
        Color::Reset
    }
}

pub(super) fn fill_rect_bg(buf: &mut Buffer, rect: Rect, bg: Color) {
    let area = rect.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_symbol(" ").set_bg(bg);
        }
    }
}

pub(super) fn draw_pill_chip(
    buf: &mut Buffer,
    rect: Rect,
    text: &str,
    active: bool,
    hovered: bool,
    theme: &Theme,
    parent_bg: Color,
) {
    if rect.width < 2 || rect.height == 0 {
        return;
    }
    if theme.flags.ascii {
        let wrapped = if active {
            format!("[{text}]")
        } else {
            format!(" {text} ")
        };
        let style = if active {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            tokens::text(theme, Level::Secondary)
        };
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&wrapped, rect.width as usize, true),
            rect.width as usize,
            style,
        );
        return;
    }

    let (fg, pbg) = if active {
        (
            tokens::color(theme, Role::OnAccent),
            tokens::color(theme, Role::Accent),
        )
    } else if hovered && theme.flags.paint_bg {
        (
            tokens::color(theme, Role::Text1),
            tokens::color(theme, Role::Elevated),
        )
    } else if theme.flags.paint_bg {
        (
            tokens::color(theme, Role::Text2),
            tokens::color(theme, Role::SurfaceAlt),
        )
    } else {
        (tokens::color(theme, Role::Text2), parent_bg)
    };
    fill_rect_bg(buf, rect, pbg);
    let mut style = Style::default().fg(fg).bg(pbg);
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    let body = format!(" {text} ");
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&body, rect.width as usize, false),
        rect.width as usize,
        style,
    );
}

pub(super) fn container_state_glyph(theme: &Theme, state: ContainerState) -> (&'static str, Tone) {
    match state {
        ContainerState::Running => (tokens::glyph(theme, Glyph::DotOn), Tone::Success),
        ContainerState::Restarting => (if theme.flags.ascii { "~" } else { "◐" }, Tone::Warning),
        ContainerState::Paused => (if theme.flags.ascii { "=" } else { "⏸" }, Tone::Info),
        ContainerState::Created | ContainerState::Removing => {
            (tokens::glyph(theme, Glyph::DotOff), Tone::Muted)
        }
        ContainerState::Exited => (tokens::glyph(theme, Glyph::DotOff), Tone::Muted),
        ContainerState::Dead => (tokens::glyph(theme, Glyph::Failure), Tone::Danger),
        ContainerState::Unknown => ("?", Tone::Muted),
    }
}

/// Context-aware Docker footer shortcut hints rendered by `tui/render/mod.rs`.
pub(in crate::tui) fn footer_pairs(app: &App) -> Vec<(&'static str, &str)> {
    let i = &app.ctx.i18n;
    let dk = &app.docker;

    if dk.logs.fullscreen {
        return vec![
            ("f", i.text("docker.key.follow")),
            ("t", i.text("docker.key.timestamps")),
            ("w", i.text("docker.key.wrap")),
            ("/", i.text("docker.key.filter")),
            ("c", i.text("docker.key.clear")),
            ("y", i.text("key.copy")),
            ("Shift+V", i.text("key.plain_view")),
            ("Esc", i.text("key.close")),
        ];
    }

    if dk.filtering {
        return vec![
            ("↵", i.text("docker.key.filter")),
            ("Esc", i.text("docker.modal.cancel")),
        ];
    }

    let tab_key = if dk.fullscreen { "1-6" } else { "←→" };
    let zoom_lbl = if dk.fullscreen {
        i.text("docker.key.unfocus")
    } else {
        i.text("docker.key.focus")
    };

    let mut pairs = match dk.tab {
        DockerTab::Containers => {
            let mut pairs = vec![
                ("↑↓", i.text("key.move")),
                (tab_key, i.text("docker.key.tabs")),
                ("↵", i.text("docker.key.menu")),
                ("s", i.text("docker.key.start_stop")),
                ("u", i.text("docker.key.restart")),
                ("e", i.text("docker.key.exec")),
                ("l", i.text("docker.key.logs")),
                ("/", i.text("docker.key.filter")),
                ("z", zoom_lbl),
                ("?", i.text("key.help")),
            ];
            if dk.container_sub == ContainerSubTab::Env {
                pairs.insert(6, ("Shift+S", i.text("docker.key.reveal_env")));
            }
            pairs
        }
        DockerTab::Compose => {
            let mut pairs = vec![
                ("↑↓", i.text("key.move")),
                (tab_key, i.text("docker.key.tabs")),
                ("↵", i.text("docker.key.menu")),
                ("u", "Up"),
                ("s", i.text("docker.key.start_stop")),
                ("l", i.text("docker.key.logs")),
                ("d", "Down"),
                ("z", zoom_lbl),
                ("?", i.text("key.help")),
            ];
            if dk.compose_sub == ComposeSubTab::Logs {
                pairs.insert(6, ("f", i.text("docker.key.follow")));
            }
            pairs
        }
        DockerTab::Images => vec![
            ("↑↓", i.text("key.move")),
            (tab_key, i.text("docker.key.tabs")),
            ("↵", i.text("docker.key.menu")),
            ("p", i.text("docker.key.pull")),
            ("d", i.text("docker.key.delete")),
            ("Shift+P", i.text("docker.key.prune")),
            ("/", i.text("docker.key.filter")),
            ("z", zoom_lbl),
            ("?", i.text("key.help")),
        ],
        DockerTab::Volumes => vec![
            ("↑↓", i.text("key.move")),
            (tab_key, i.text("docker.key.tabs")),
            ("↵", i.text("docker.key.menu")),
            ("d", i.text("docker.key.delete")),
            ("Shift+P", i.text("docker.key.prune")),
            ("y", i.text("key.copy")),
            ("z", zoom_lbl),
            ("?", i.text("key.help")),
        ],
        DockerTab::Networks => vec![
            ("↑↓", i.text("key.move")),
            (tab_key, i.text("docker.key.tabs")),
            ("↵", i.text("docker.key.menu")),
            ("c", i.text("docker.key.new_net")),
            ("d", i.text("docker.key.delete")),
            ("Shift+P", i.text("docker.key.prune")),
            ("z", zoom_lbl),
            ("?", i.text("key.help")),
        ],
        DockerTab::Daemon => vec![
            ("↑↓", i.text("key.move")),
            (tab_key, i.text("docker.key.tabs")),
            ("↵", i.text("key.open")),
            ("Shift+P", i.text("docker.key.prune")),
            ("r", i.text("key.refresh")),
            ("z", zoom_lbl),
            ("?", i.text("key.help")),
        ],
    };
    if app.viewport.width < 80 {
        pairs.insert(3, ("[", i.text("nav.toggle")));
    }
    pairs
}
