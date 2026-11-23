//! Left-pane resource list renderers for all 6 Docker tabs (Containers, Compose, Images,
//! Volumes, Networks, and Daemon controls).

use super::{container_state_glyph, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    actions,
    ops::docker::model::{DockerTab, format_bytes, format_bytes_compact},
    theme::{Level, Role},
    tui::{
        docker::{layout::DockerGeometry, state::DaemonEntry},
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::{App, Tone},
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &DockerGeometry) {
    let theme = &app.theme.active;
    let border_role = if app.docker.fullscreen && !app.docker.focus_inspector {
        Role::BorderFocus
    } else {
        Role::Border
    };
    if theme.flags.framed {
        let elapsed = app.now.saturating_duration_since(app.started);
        let reduced = app.reduced_motion();
        tokens::draw_surface_card_fx(
            frame,
            geom.list_area,
            theme,
            Role::SurfaceAlt,
            border_role,
            true,
            elapsed,
            reduced,
        );
    } else {
        fill_rect_bg(
            frame.buffer_mut(),
            geom.list_area,
            surface_bg(theme, Role::SurfaceAlt),
        );
    }
    let bg = surface_bg(theme, Role::SurfaceAlt);

    if app.docker.tab == DockerTab::Daemon {
        draw_daemon_list(app, frame, geom, bg);
        return;
    }

    draw_filter_bar(app, frame, geom, bg);
    draw_table_header(app, frame, geom, bg);

    match app.docker.tab {
        DockerTab::Containers => draw_containers_rows(app, frame, geom, bg),
        DockerTab::Compose => draw_compose_rows(app, frame, geom, bg),
        DockerTab::Images => draw_images_rows(app, frame, geom, bg),
        DockerTab::Volumes => draw_volumes_rows(app, frame, geom, bg),
        DockerTab::Networks => draw_networks_rows(app, frame, geom, bg),
        DockerTab::Daemon => unreachable!(),
    }

    if let Some(batch) = geom.batch_bar {
        draw_batch_bar(app, frame, batch, bg);
    }
}

fn draw_filter_bar(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    if geom.filter_bar.width < 12 || geom.filter_bar.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let dk = &app.docker;
    let ascii = theme.flags.ascii;
    let buf = frame.buffer_mut();

    let right_limit = geom
        .project_chip
        .or(geom.scope_chip)
        .or(geom.sort_chip)
        .map(|r| r.x.saturating_sub(1))
        .unwrap_or(geom.filter_bar.right());
    let input_w = right_limit.saturating_sub(geom.filter_bar.x) as usize;

    if dk.filtering {
        let prompt = format!("/ {}_", dk.filter.text);
        buf.set_stringn(
            geom.filter_bar.x,
            geom.filter_bar.y,
            clip_ascii_safe(&prompt, input_w, ascii),
            input_w,
            tokens::fg(theme, Role::Accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    } else if !dk.filter.text.is_empty() {
        let prompt = format!("/ {} (Esc)", dk.filter.text);
        buf.set_stringn(
            geom.filter_bar.x,
            geom.filter_bar.y,
            clip_ascii_safe(&prompt, input_w, ascii),
            input_w,
            tokens::fg(theme, Role::Accent).bg(bg),
        );
    } else {
        let placeholder = format!("/ {}", i.text("docker.filter.placeholder"));
        buf.set_stringn(
            geom.filter_bar.x,
            geom.filter_bar.y,
            clip_ascii_safe(&placeholder, input_w, ascii),
            input_w,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }

    if let Some(chip) = geom.project_chip
        && let Some(proj) = &dk.project_filter
    {
        let lbl = format!(
            "{} x",
            i.format("docker.filter.project_badge", &[("name", proj)])
        );
        let hovered = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(buf, chip, &lbl, true, hovered, theme, bg);
    }
    if let Some(chip) = geom.scope_chip {
        let (lbl, active) = match dk.tab {
            DockerTab::Containers => (
                if dk.only_running {
                    format!("[a] {}", i.text("docker.filter.running"))
                } else {
                    format!("[a] {}", i.text("docker.filter.all"))
                },
                dk.only_running,
            ),
            DockerTab::Images => (
                if dk.show_dangling {
                    format!("[a] {}", i.text("docker.filter.dangling_on"))
                } else {
                    format!("[a] {}", i.text("docker.filter.dangling_off"))
                },
                dk.show_dangling,
            ),
            _ => (String::new(), false),
        };
        if !lbl.is_empty() {
            let hovered = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, active, hovered, theme, bg);
        }
    }
    if let Some(chip) = geom.sort_chip
        && let Some(key) = dk.sort_label_key()
    {
        let arrow = if dk.sort_ascending() {
            if ascii { "^" } else { "▴" }
        } else if ascii {
            "v"
        } else {
            "▾"
        };
        let lbl = format!("[o] {}{arrow}", i.text(key));
        let hovered = app.hover.is_some_and(|p| chip.contains(p));
        draw_pill_chip(buf, chip, &lbl, false, hovered, theme, bg);
    }
}

fn draw_table_header(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let hdr = geom.table_header;
    if hdr.width < 12 || hdr.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let buf = frame.buffer_mut();
    let style = tokens::text(theme, Level::Tertiary)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    match app.docker.tab {
        DockerTab::Containers => {
            buf.set_stringn(hdr.x + 1, hdr.y, "   ", 3, style);
            buf.set_stringn(hdr.x + 5, hdr.y, i.text("docker.col.name"), 18, style);
            if hdr.width >= 58 {
                let img_x = hdr.x + 23;
                buf.set_stringn(img_x, hdr.y, i.text("docker.col.image"), 18, style);
            }
            if hdr.width >= 38 {
                let cpu_x = hdr.right().saturating_sub(13);
                buf.set_stringn(cpu_x, hdr.y, i.text("docker.col.cpu"), 6, style);
                buf.set_stringn(cpu_x + 6, hdr.y, i.text("docker.col.mem"), 7, style);
            }
        }
        DockerTab::Compose => {
            buf.set_stringn(hdr.x + 2, hdr.y, i.text("docker.col.project"), 18, style);
            let st_x = hdr.x + (hdr.width / 2).min(24);
            let st_w = hdr.right().saturating_sub(st_x).min(16) as usize;
            buf.set_stringn(st_x, hdr.y, i.text("docker.col.status"), st_w, style);
            if hdr.width >= 56 {
                tokens::put_right(buf, hdr, i.text("docker.col.config"), style);
            }
        }
        DockerTab::Images => {
            buf.set_stringn(hdr.x + 5, hdr.y, i.text("docker.col.repo"), 22, style);
            if hdr.width >= 40 {
                let sz_x = hdr.right().saturating_sub(18);
                buf.set_stringn(sz_x, hdr.y, i.text("docker.col.size"), 8, style);
                tokens::put_right(buf, hdr, i.text("docker.col.refs"), style);
            }
        }
        DockerTab::Volumes => {
            buf.set_stringn(hdr.x + 5, hdr.y, i.text("docker.col.name"), 22, style);
            if hdr.width >= 40 {
                let sz_x = hdr.right().saturating_sub(18);
                buf.set_stringn(sz_x, hdr.y, i.text("docker.col.size"), 8, style);
                tokens::put_right(buf, hdr, i.text("docker.col.refs"), style);
            }
        }
        DockerTab::Networks => {
            buf.set_stringn(hdr.x + 5, hdr.y, i.text("docker.col.name"), 18, style);
            if hdr.width >= 38 {
                let drv_x = hdr.right().saturating_sub(20);
                buf.set_stringn(drv_x, hdr.y, i.text("docker.col.driver"), 9, style);
                tokens::put_right(buf, hdr, i.text("docker.col.containers"), style);
            }
        }
        DockerTab::Daemon => {}
    }
}

fn draw_empty_list(app: &App, frame: &mut Frame, inner: Rect, empty_key: &str, bg: Color) {
    if inner.height == 0 || inner.width < 10 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let msg = if !app.docker.filter.text.is_empty() {
        i.format(
            "docker.empty.no_match",
            &[("query", &app.docker.filter.text)],
        )
    } else {
        i.text(empty_key).to_owned()
    };
    let y = inner.y + (inner.height / 3).min(2);
    frame.buffer_mut().set_stringn(
        inner.x + 2,
        y,
        clip_ascii_safe(
            &msg,
            inner.width.saturating_sub(4) as usize,
            theme.flags.ascii,
        ),
        inner.width.saturating_sub(4) as usize,
        tokens::text(theme, Level::Tertiary).bg(bg),
    );
}

fn paint_selectable_row(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    row_rect: Rect,
    active: bool,
    bg: Color,
) -> Color {
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
    let row_bg = if active {
        tokens::color(theme, Role::Selection)
    } else if hovered && theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        bg
    };
    fill_rect_bg(buf, row_rect, row_bg);
    if active && row_rect.width > 0 && row_rect.height > 0 {
        buf[(row_rect.x, row_rect.y)]
            .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
            .set_style(
                Style::default()
                    .fg(tokens::selected_accent(theme))
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            );
    }
    row_bg
}

fn draw_checkbox(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    checked: bool,
    row_bg: Color,
    theme: &crate::theme::Theme,
) {
    let sym = if checked {
        if theme.flags.ascii { "[x]" } else { "[✓]" }
    } else {
        "[ ]"
    };
    let fg = if checked {
        tokens::color(theme, Role::Accent)
    } else {
        tokens::color(theme, Role::Text3)
    };
    buf.set_stringn(x, y, sym, 3, Style::default().fg(fg).bg(row_bg));
}

fn draw_containers_rows(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    let items = app.docker.filtered_containers();
    if items.is_empty() {
        draw_empty_list(app, frame, inner, "docker.empty.no_containers", bg);
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let selected = app.docker.cursor();
    let buf = frame.buffer_mut();

    for (offset, c) in items
        .iter()
        .skip(geom.list_scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        let checked = app.docker.selected_ids[0].contains(&c.id);
        draw_checkbox(buf, inner.x + 1, y, checked, row_bg, theme);

        let (st_sym, st_tone) = container_state_glyph(theme, c.state);
        buf.set_stringn(
            inner.x + 5,
            y,
            st_sym,
            2,
            tokens::tone_style(theme, st_tone).bg(row_bg),
        );

        let show_metrics = inner.width >= 38;
        let show_image = inner.width >= 58;
        let right_reserved = if show_metrics { 14 } else { 1 };
        let name_x = inner.x + 7;
        let name_budget = if show_image {
            16usize
        } else {
            inner.width.saturating_sub(7 + right_reserved) as usize
        };
        let mut name_style = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            name_x,
            y,
            clip_ascii_safe(&c.name, name_budget, ascii),
            name_budget,
            name_style,
        );

        if show_image {
            let img_x = inner.x + 23;
            let img_budget = inner.right().saturating_sub(img_x + right_reserved) as usize;
            let img_text = if inner.width >= 92
                && let Some(proj) = &c.compose_project
            {
                let sep = tokens::glyph(theme, Glyph::Separator);
                format!("{} {sep} {proj}", c.image)
            } else {
                c.image.clone()
            };
            buf.set_stringn(
                img_x,
                y,
                clip_ascii_safe(&img_text, img_budget, ascii),
                img_budget,
                tokens::text(theme, Level::Secondary).bg(row_bg),
            );
        }

        if show_metrics {
            let cpu_x = inner.right().saturating_sub(13);
            if let Some(st) = app.docker.snapshot.stats.get(&c.id) {
                let cpu_s = format!("{:>4.1}%", st.cpu_percent);
                let mem_s = format!("{:>6}", format_bytes_compact(st.mem_used_bytes));
                buf.set_stringn(
                    cpu_x,
                    y,
                    &cpu_s,
                    6,
                    Style::default()
                        .fg(crate::tui::fx::meter_color(st.cpu_percent, theme))
                        .bg(row_bg),
                );
                buf.set_stringn(
                    cpu_x + 6,
                    y,
                    &mem_s,
                    7,
                    Style::default()
                        .fg(crate::tui::fx::meter_color(st.mem_percent, theme))
                        .bg(row_bg),
                );
            } else {
                buf.set_stringn(
                    cpu_x,
                    y,
                    "   -      -",
                    13,
                    tokens::text(theme, Level::Tertiary).bg(row_bg),
                );
            }
        }
    }
}

fn draw_compose_rows(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    let items = app.docker.filtered_compose();
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let buf = frame.buffer_mut();

    let no_plugin = app
        .docker
        .snapshot
        .engine
        .as_ref()
        .is_some_and(|e| e.compose_version.is_none());
    let mut start_y = inner.y;
    if no_plugin && inner.height >= 3 {
        buf.set_stringn(
            inner.x + 1,
            start_y,
            clip_ascii_safe(
                i.text("docker.empty.no_compose_plugin"),
                inner.width.saturating_sub(2) as usize,
                ascii,
            ),
            inner.width.saturating_sub(2) as usize,
            tokens::fg(theme, Role::Warning).bg(bg),
        );
        start_y += 1;
    }

    if items.is_empty() {
        draw_empty_list(
            app,
            frame,
            Rect::new(
                inner.x,
                start_y,
                inner.width,
                inner.bottom().saturating_sub(start_y),
            ),
            "docker.empty.no_compose",
            bg,
        );
        return;
    }

    let selected = app.docker.cursor();
    let avail_h = inner.bottom().saturating_sub(start_y) as usize;
    for (offset, p) in items
        .iter()
        .skip(geom.list_scroll)
        .take(avail_h)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = start_y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        let (dot, tone) = if p.running_count == p.total_count && p.total_count > 0 {
            (tokens::glyph(theme, Glyph::DotOn), Tone::Success)
        } else if p.running_count > 0 {
            (if ascii { "~" } else { "◐" }, Tone::Warning)
        } else {
            (tokens::glyph(theme, Glyph::DotOff), Tone::Muted)
        };
        buf.set_stringn(
            inner.x + 2,
            y,
            dot,
            2,
            tokens::tone_style(theme, tone).bg(row_bg),
        );

        let name_budget = (inner.width / 2).saturating_sub(5).max(10) as usize;
        let mut st = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner.x + 4,
            y,
            clip_ascii_safe(&p.name, name_budget, ascii),
            name_budget,
            st,
        );

        let st_x = inner.x + (inner.width / 2).min(24);
        let st_budget = inner.right().saturating_sub(st_x).min(22) as usize;
        let status_txt = format!("{}/{} {}", p.running_count, p.total_count, p.status);
        buf.set_stringn(
            st_x,
            y,
            clip_ascii_safe(&status_txt, st_budget, ascii),
            st_budget,
            tokens::tone_style(theme, tone).bg(row_bg),
        );

        if inner.width >= 56
            && let Some(cfg) = p.config_files.first()
        {
            let max_cfg = inner.right().saturating_sub(st_x + 23) as usize;
            if max_cfg >= 6 {
                tokens::put_right(
                    buf,
                    row_rect,
                    &clip_ascii_safe(cfg, max_cfg, ascii),
                    tokens::text(theme, Level::Tertiary).bg(row_bg),
                );
            }
        }
    }
}

fn draw_images_rows(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    let items = app.docker.filtered_images();
    if items.is_empty() {
        draw_empty_list(app, frame, inner, "docker.empty.no_images", bg);
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let selected = app.docker.cursor();
    let buf = frame.buffer_mut();

    for (offset, img) in items
        .iter()
        .skip(geom.list_scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        let checked = app.docker.selected_ids[2].contains(&img.id);
        draw_checkbox(buf, inner.x + 1, y, checked, row_bg, theme);

        let show_right = inner.width >= 40;
        let right_w = if show_right { 20 } else { 1 };
        let name_budget = inner.width.saturating_sub(5 + right_w) as usize;
        let mut st = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else if img.dangling {
                tokens::color(theme, Role::Text3)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner.x + 5,
            y,
            clip_ascii_safe(&img.display_ref(), name_budget, ascii),
            name_budget,
            st,
        );

        if show_right {
            let sz_x = inner.right().saturating_sub(18);
            buf.set_stringn(
                sz_x,
                y,
                clip_ascii_safe(&img.size_text, 8, ascii),
                8,
                tokens::text(theme, Level::Secondary).bg(row_bg),
            );
            let (badge, tone) = if !img.used_by.is_empty() {
                (
                    i.format(
                        "docker.badge.in_use",
                        &[("count", &img.used_by.len().to_string())],
                    ),
                    Tone::Success,
                )
            } else if img.dangling {
                (i.text("docker.badge.dangling").to_owned(), Tone::Warning)
            } else {
                (i.text("docker.badge.unused").to_owned(), Tone::Muted)
            };
            tokens::put_right(
                buf,
                row_rect,
                &badge,
                tokens::tone_style(theme, tone).bg(row_bg),
            );
        }
    }
}

fn draw_volumes_rows(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    let items = app.docker.filtered_volumes();
    if items.is_empty() {
        draw_empty_list(app, frame, inner, "docker.empty.no_volumes", bg);
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let selected = app.docker.cursor();
    let buf = frame.buffer_mut();

    for (offset, vol) in items
        .iter()
        .skip(geom.list_scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        let checked = app.docker.selected_ids[3].contains(&vol.name);
        draw_checkbox(buf, inner.x + 1, y, checked, row_bg, theme);

        let show_right = inner.width >= 40;
        let right_w = if show_right { 20 } else { 1 };
        let name_budget = inner.width.saturating_sub(5 + right_w) as usize;
        let display_name = if vol.anonymous {
            format!(
                "[{}] {}",
                i.text("docker.badge.anonymous"),
                &vol.name[..vol.name.len().min(12)]
            )
        } else {
            vol.name.clone()
        };
        let mut st = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner.x + 5,
            y,
            clip_ascii_safe(&display_name, name_budget, ascii),
            name_budget,
            st,
        );

        if show_right {
            let sz_x = inner.right().saturating_sub(18);
            buf.set_stringn(
                sz_x,
                y,
                clip_ascii_safe(&vol.size_text, 8, ascii),
                8,
                tokens::text(theme, Level::Secondary).bg(row_bg),
            );
            let (badge, tone) = if !vol.used_by.is_empty() {
                (
                    i.format(
                        "docker.badge.in_use",
                        &[("count", &vol.used_by.len().to_string())],
                    ),
                    Tone::Success,
                )
            } else {
                (i.text("docker.badge.unused").to_owned(), Tone::Muted)
            };
            tokens::put_right(
                buf,
                row_rect,
                &badge,
                tokens::tone_style(theme, tone).bg(row_bg),
            );
        }
    }
}

fn draw_networks_rows(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    let items = app.docker.filtered_networks();
    if items.is_empty() {
        draw_empty_list(app, frame, inner, "docker.empty.no_networks", bg);
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let selected = app.docker.cursor();
    let buf = frame.buffer_mut();

    for (offset, net) in items
        .iter()
        .skip(geom.list_scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        if net.builtin {
            buf.set_stringn(
                inner.x + 1,
                y,
                " - ",
                3,
                tokens::text(theme, Level::Tertiary).bg(row_bg),
            );
        } else {
            let checked = app.docker.selected_ids[4].contains(&net.id);
            draw_checkbox(buf, inner.x + 1, y, checked, row_bg, theme);
        }

        let show_right = inner.width >= 38;
        let right_w = if show_right { 21 } else { 1 };
        let name_budget = inner.width.saturating_sub(5 + right_w) as usize;
        let mut st = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner.x + 5,
            y,
            clip_ascii_safe(&net.name, name_budget, ascii),
            name_budget,
            st,
        );

        if show_right {
            let drv_x = inner.right().saturating_sub(20);
            let drv_lbl = if net.ipv6 {
                format!("{}+v6", net.driver)
            } else {
                net.driver.clone()
            };
            buf.set_stringn(
                drv_x,
                y,
                clip_ascii_safe(&drv_lbl, 10, ascii),
                10,
                tokens::text(theme, Level::Secondary).bg(row_bg),
            );
            let badge = if net.builtin {
                i.text("docker.badge.builtin").to_owned()
            } else {
                format!("{}", net.containers.len())
            };
            tokens::put_right(
                buf,
                row_rect,
                &badge,
                tokens::text(
                    theme,
                    if net.builtin {
                        Level::Tertiary
                    } else {
                        Level::Secondary
                    },
                )
                .bg(row_bg),
            );
        }
    }
}

fn draw_daemon_list(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    let inner = geom.list_inner;
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let selected = app.docker.cursor();
    let buf = frame.buffer_mut();

    // Render all 10 interactive Daemon entries directly aligned with `geom.row_at()`
    for (offset, entry) in DaemonEntry::ALL
        .iter()
        .skip(geom.list_scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let active = idx == selected;
        let row_bg = paint_selectable_row(app, buf, row_rect, active, bg);

        let (icon, label, tag, tone) = match *entry {
            DaemonEntry::ServiceStart => (
                tokens::glyph(theme, Glyph::Execute),
                i.text("docker.op.service_start").to_owned(),
                "systemd".to_owned(),
                Tone::Success,
            ),
            DaemonEntry::ServiceStop => (
                tokens::glyph(theme, Glyph::Cancelled),
                i.text("docker.op.service_stop").to_owned(),
                "systemd".to_owned(),
                Tone::Warning,
            ),
            DaemonEntry::ServiceRestart => (
                tokens::glyph(theme, Glyph::Execute),
                i.text("docker.op.service_restart").to_owned(),
                "systemd".to_owned(),
                Tone::Info,
            ),
            DaemonEntry::PruneWizard => {
                let rec = app.docker.snapshot.total_reclaimable_bytes();
                let tag = if rec > 0 {
                    format!(
                        "{} {}",
                        i.text("docker.field.reclaimable"),
                        format_bytes(rec)
                    )
                } else {
                    "Shift+P".to_owned()
                };
                (
                    tokens::glyph(theme, Glyph::Optimize),
                    i.text("docker.op.prune").to_owned(),
                    tag,
                    Tone::Accent,
                )
            }
            DaemonEntry::CqsAction(id) => {
                let spec_lbl = actions::find(id)
                    .map(|s| i.text(&s.label()).to_owned())
                    .unwrap_or_else(|_| id.to_owned());
                let is_danger = matches!(id, "docker.clean" | "docker.uninstall");
                (
                    tokens::glyph(theme, Glyph::Chevron),
                    spec_lbl,
                    id.to_owned(),
                    if is_danger { Tone::Danger } else { Tone::Plain },
                )
            }
        };

        buf.set_stringn(
            inner.x + 2,
            y,
            icon,
            2,
            tokens::tone_style(theme, tone).bg(row_bg),
        );

        let tag_w = if inner.width >= 42 {
            (tag.width() + 2).min(20)
        } else {
            0
        };
        let lbl_budget = inner.width.saturating_sub(5 + tag_w as u16) as usize;
        let mut st = Style::default()
            .fg(if active {
                tokens::selected_fg(theme)
            } else {
                tokens::color(theme, Role::Text1)
            })
            .bg(row_bg);
        if active {
            st = st.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            inner.x + 4,
            y,
            clip_ascii_safe(&label, lbl_budget, ascii),
            lbl_budget,
            st,
        );
        if tag_w > 0 {
            tokens::put_right(
                buf,
                row_rect,
                &clip_ascii_safe(&tag, tag_w, ascii),
                tokens::text(theme, Level::Tertiary).bg(row_bg),
            );
        }
    }
}

fn draw_batch_bar(app: &App, frame: &mut Frame, bar: Rect, _bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let count = app.docker.selected_count();
    let badge = i.format("docker.badge.selected", &[("count", &count.to_string())]);
    let hint = match app.docker.tab {
        DockerTab::Containers => format!(
            " {badge}: [s] {}  [u] {}  [d] {}  [Esc] ",
            i.text("docker.key.start_stop"),
            i.text("docker.key.restart"),
            i.text("docker.key.delete"),
        ),
        _ => format!(" {badge}: [d] {}  [Esc] ", i.text("docker.key.delete")),
    };
    let bar_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Selection)
    } else {
        Color::Reset
    };
    let buf = frame.buffer_mut();
    fill_rect_bg(buf, bar, bar_bg);
    buf.set_stringn(
        bar.x,
        bar.y,
        clip_ascii_safe(&hint, bar.width as usize, theme.flags.ascii),
        bar.width as usize,
        Style::default()
            .fg(tokens::selected_accent(theme))
            .bg(bar_bg)
            .add_modifier(Modifier::BOLD),
    );
}
