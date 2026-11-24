//! Docker-specific modals: contextual ActionMenu, PullImage, CreateNetwork, ExecForm, and
//! interactive PruneWizard.

use super::{draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    ops::docker::model::format_bytes,
    theme::{Level, Role},
    tui::{
        docker::{layout::DockerModalGeometry, state::DockerModal},
        fx,
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
};

pub(in crate::tui) fn draw(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let Some(modal) = &app.docker.modal else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let geom = DockerModalGeometry::compute(area, modal);

    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    tokens::draw_surface_card(
        frame,
        geom.modal,
        theme,
        Role::Elevated,
        Role::BorderFocus,
        true,
    );
    let bg = surface_bg(theme, Role::Elevated);
    let buf = frame.buffer_mut();

    let title = match modal {
        DockerModal::ActionMenu { title, .. } => {
            format!(" {} - {title} ", i.text("docker.modal.actions_title"))
        }
        DockerModal::PullImage { .. } => format!(" {} ", i.text("docker.modal.pull_title")),
        DockerModal::CreateNetwork { .. } => {
            format!(" {} ", i.text("docker.modal.network_title"))
        }
        DockerModal::ExecForm { container_name, .. } => {
            format!(" {} - {container_name} ", i.text("docker.modal.exec_title"))
        }
        DockerModal::PruneWizard { .. } => format!(" {} ", i.text("docker.modal.prune_title")),
    };
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        clip_ascii_safe(&title, geom.modal.width.saturating_sub(4) as usize, ascii),
        geom.modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    match modal {
        DockerModal::ActionMenu { items, cursor, .. } => {
            for (idx, (item, row)) in items.iter().zip(&geom.rows).enumerate() {
                let active = idx == *cursor;
                let hovered = app.hover.is_some_and(|p| row.contains(p));
                let row_bg = if active {
                    tokens::color(theme, Role::Selection)
                } else if hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                };
                fill_rect_bg(buf, *row, row_bg);
                if active && row.width > 0 && row.height > 0 {
                    buf[(row.x, row.y)]
                        .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                        .set_style(
                            Style::default()
                                .fg(tokens::selected_accent(theme))
                                .bg(row_bg)
                                .add_modifier(Modifier::BOLD),
                        );
                }
                let lbl = i.text(item.label_key);
                let style = if item.disabled_toast.is_some() {
                    tokens::text(theme, Level::Tertiary).bg(row_bg)
                } else if active {
                    Style::default()
                        .fg(tokens::selected_fg(theme))
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    tokens::tone_style(theme, item.tone).bg(row_bg)
                };
                buf.set_stringn(
                    row.x + 2,
                    row.y,
                    clip_ascii_safe(lbl, row.width.saturating_sub(8) as usize, ascii),
                    row.width.saturating_sub(8) as usize,
                    style,
                );
                tokens::put_right(
                    buf,
                    *row,
                    &format!("[{}]", item.shortcut),
                    tokens::text(theme, Level::Tertiary).bg(row_bg),
                );
            }
        }
        DockerModal::PullImage { reference, error } => {
            if let Some(row) = geom.rows.first() {
                draw_input_field(
                    buf,
                    *row,
                    i.text("docker.modal.pull_field"),
                    reference,
                    true,
                    theme,
                    bg,
                );
            }
            let hint_y = geom.modal.y + 5;
            if hint_y < geom.modal.bottom().saturating_sub(2) {
                buf.set_stringn(
                    geom.modal.x + 2,
                    hint_y,
                    clip_ascii_safe(
                        i.text("docker.modal.pull_hint"),
                        geom.modal.width.saturating_sub(4) as usize,
                        ascii,
                    ),
                    geom.modal.width.saturating_sub(4) as usize,
                    tokens::text(theme, Level::Tertiary).bg(bg),
                );
            }
            if let Some(err) = error {
                draw_modal_error(buf, geom.modal, err, theme, bg);
            }
        }
        DockerModal::CreateNetwork {
            name,
            subnet,
            ipv6,
            field,
            error,
        } => {
            if let Some(r0) = geom.rows.first() {
                draw_input_field(
                    buf,
                    *r0,
                    i.text("docker.modal.network_name"),
                    name,
                    *field == 0,
                    theme,
                    bg,
                );
            }
            if let Some(r1) = geom.rows.get(1) {
                draw_input_field(
                    buf,
                    *r1,
                    i.text("docker.modal.network_subnet"),
                    subnet,
                    *field == 1,
                    theme,
                    bg,
                );
            }
            if let Some(r2) = geom.rows.get(2) {
                let mark = if *ipv6 {
                    if ascii { "[x]" } else { "[✓]" }
                } else {
                    "[ ]"
                };
                let row_bg = if *field == 2 {
                    tokens::color(theme, Role::Selection)
                } else {
                    bg
                };
                fill_rect_bg(buf, *r2, row_bg);
                let line = format!("{mark} {}", i.text("docker.modal.network_ipv6"));
                buf.set_stringn(
                    r2.x,
                    r2.y,
                    clip_ascii_safe(&line, r2.width as usize, ascii),
                    r2.width as usize,
                    Style::default()
                        .fg(if *field == 2 {
                            tokens::selected_fg(theme)
                        } else {
                            tokens::color(theme, Role::Text1)
                        })
                        .bg(row_bg),
                );
            }
            if let Some(err) = error {
                draw_modal_error(buf, geom.modal, err, theme, bg);
            }
        }
        DockerModal::ExecForm {
            shell,
            user,
            workdir,
            field,
            error,
            ..
        } => {
            let entries = [
                (i.text("docker.modal.exec_shell"), shell),
                (i.text("docker.modal.exec_user"), user),
                (i.text("docker.modal.exec_workdir"), workdir),
            ];
            for (idx, ((lbl, val), row)) in entries.iter().zip(&geom.rows).enumerate() {
                draw_input_field(buf, *row, lbl, val, *field == idx, theme, bg);
            }
            if let Some(err) = error {
                draw_modal_error(buf, geom.modal, err, theme, bg);
            }
        }
        DockerModal::PruneWizard {
            containers,
            images,
            all_unused_images,
            volumes,
            networks,
            build_cache,
            cursor,
        } => {
            buf.set_stringn(
                geom.modal.x + 2,
                geom.modal.y + 2,
                clip_ascii_safe(
                    i.text("docker.modal.prune_sub"),
                    geom.modal.width.saturating_sub(4) as usize,
                    ascii,
                ),
                geom.modal.width.saturating_sub(4) as usize,
                tokens::text(theme, Level::Secondary).bg(bg),
            );
            let reclaim_for = |kind: &str| -> String {
                app.docker
                    .snapshot
                    .disk_usage
                    .iter()
                    .find(|d| d.kind.eq_ignore_ascii_case(kind))
                    .map(|d| format_bytes(d.reclaimable_bytes))
                    .unwrap_or_default()
            };
            let rows_data = [
                (
                    *containers,
                    i.text("docker.modal.prune_containers"),
                    reclaim_for("Containers"),
                    false,
                ),
                (
                    *images,
                    i.text("docker.modal.prune_images"),
                    reclaim_for("Images"),
                    false,
                ),
                (
                    *all_unused_images,
                    i.text("docker.modal.prune_images_all"),
                    String::new(),
                    false,
                ),
                (
                    *volumes,
                    i.text("docker.modal.prune_volumes"),
                    reclaim_for("Local Volumes"),
                    true,
                ),
                (
                    *networks,
                    i.text("docker.modal.prune_networks"),
                    String::new(),
                    false,
                ),
                (
                    *build_cache,
                    i.text("docker.modal.prune_cache"),
                    reclaim_for("Build Cache"),
                    false,
                ),
            ];
            for (idx, ((checked, lbl, rec, danger), row)) in
                rows_data.iter().zip(&geom.rows).enumerate()
            {
                let active = idx == *cursor;
                let row_bg = if active {
                    tokens::color(theme, Role::Selection)
                } else {
                    bg
                };
                fill_rect_bg(buf, *row, row_bg);
                let mark = if *checked {
                    if ascii { "[x]" } else { "[✓]" }
                } else {
                    "[ ]"
                };
                let fg = if *danger && *checked {
                    tokens::color(theme, Role::Danger)
                } else if active {
                    tokens::selected_fg(theme)
                } else {
                    tokens::color(theme, Role::Text1)
                };
                let rec_reserve = if rec.is_empty() {
                    0
                } else {
                    (unicode_width::UnicodeWidthStr::width(rec.as_str()) + 2).min(12)
                };
                let text = format!("{mark} {lbl}");
                let text_budget = (row.width as usize).saturating_sub(rec_reserve);
                buf.set_stringn(
                    row.x,
                    row.y,
                    clip_ascii_safe(&text, text_budget, ascii),
                    text_budget,
                    Style::default().fg(fg).bg(row_bg),
                );
                if !rec.is_empty() {
                    tokens::put_right(
                        buf,
                        *row,
                        rec,
                        tokens::text(theme, Level::Tertiary).bg(row_bg),
                    );
                }
            }
        }
    }

    if let Some(btn) = geom.submit_btn {
        let hovered = app.hover.is_some_and(|p| btn.contains(p));
        let lbl = if matches!(modal, DockerModal::PruneWizard { .. }) {
            i.text("docker.modal.prune_run")
        } else {
            i.text("docker.modal.submit")
        };
        draw_pill_chip(buf, btn, lbl, true, hovered, theme, bg);
    }
    if let Some(btn) = geom.cancel_btn {
        let hovered = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(
            buf,
            btn,
            i.text("docker.modal.cancel"),
            false,
            hovered,
            theme,
            bg,
        );
    }

    if alpha < 0.995 {
        fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
    }
}

fn draw_input_field(
    buf: &mut ratatui::buffer::Buffer,
    row: Rect,
    label: &str,
    input: &crate::tui::input::InputBuffer,
    focused: bool,
    theme: &crate::theme::Theme,
    bg: Color,
) {
    let ascii = theme.flags.ascii;
    if row.y > 0 {
        buf.set_stringn(
            row.x,
            row.y - 1,
            clip_ascii_safe(label, row.width as usize, ascii),
            row.width as usize,
            tokens::text(
                theme,
                if focused {
                    Level::Primary
                } else {
                    Level::Tertiary
                },
            )
            .bg(bg),
        );
    }
    let field_bg = if focused {
        tokens::color(theme, Role::Selection)
    } else if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        bg
    };
    fill_rect_bg(buf, row, field_bg);
    let avail = row.width.saturating_sub(2) as usize;
    if avail == 0 {
        return;
    }
    if focused {
        let (shown, cur_col) = input.visible(avail);
        buf.set_stringn(
            row.x + 1,
            row.y,
            &shown,
            avail,
            Style::default().fg(tokens::selected_fg(theme)).bg(field_bg),
        );
        let cx = row.x + 1 + cur_col.min(avail as u16);
        if cx < row.right() {
            let cell = &mut buf[(cx, row.y)];
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
    } else {
        buf.set_stringn(
            row.x + 1,
            row.y,
            clip_ascii_safe(&input.text, avail, ascii),
            avail,
            Style::default()
                .fg(tokens::color(theme, Role::Text1))
                .bg(field_bg),
        );
    }
}

fn draw_modal_error(
    buf: &mut ratatui::buffer::Buffer,
    modal: Rect,
    err: &str,
    theme: &crate::theme::Theme,
    bg: Color,
) {
    let y = modal.bottom().saturating_sub(3);
    if y > modal.y + 1 {
        buf.set_stringn(
            modal.x + 2,
            y,
            clip_ascii_safe(
                err,
                modal.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            modal.width.saturating_sub(4) as usize,
            tokens::fg(theme, Role::Danger).bg(bg),
        );
    }
}
