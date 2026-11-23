//! Right-hand (and Compact-mode fullscreen) rich inspector for all 6 Docker tabs.

use super::{container_state_glyph, draw_pill_chip, fill_rect_bg, logs, surface_bg};
use crate::{
    actions,
    ops::docker::model::{DockerTab, format_bytes},
    theme::{Level, Role},
    tui::{
        docker::{
            layout::DockerGeometry,
            state::{ComposeSubTab, ContainerSubTab, DaemonEntry, ImageSubTab},
        },
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
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &DockerGeometry) {
    let theme = &app.theme.active;
    let border_role = if app.docker.fullscreen && app.docker.focus_inspector {
        Role::BorderFocus
    } else {
        Role::Border
    };
    if theme.flags.framed {
        let elapsed = app.now.saturating_duration_since(app.started);
        let reduced = app.reduced_motion();
        tokens::draw_surface_card_fx(
            frame,
            geom.inspector_area,
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
            geom.inspector_area,
            surface_bg(theme, Role::SurfaceAlt),
        );
    }
    let bg = surface_bg(theme, Role::SurfaceAlt);

    draw_inspector_title(app, frame, geom.inspector_area, bg);
    draw_sub_pills(app, frame, geom, bg);

    // Clear clickable `$ docker ...` rect before rendering body
    app.docker.last_cmd_rect.set(None);

    let body = geom.inspector_body;
    if body.width >= 10 && body.height >= 1 {
        match app.docker.tab {
            DockerTab::Containers => match app.docker.container_sub {
                ContainerSubTab::Logs => logs::draw_logs_panel(app, frame, body, false),
                ContainerSubTab::Stats => draw_container_stats(app, frame, body, bg),
                ContainerSubTab::Env => draw_container_env(app, frame, body, bg),
                ContainerSubTab::Overview => draw_container_overview(app, frame, body, bg),
            },
            DockerTab::Compose => match app.docker.compose_sub {
                ComposeSubTab::Logs => logs::draw_logs_panel(app, frame, body, false),
                ComposeSubTab::Yaml => draw_compose_yaml(app, frame, body, bg),
                ComposeSubTab::Services => draw_compose_services(app, frame, body, bg),
            },
            DockerTab::Images => match app.docker.image_sub {
                ImageSubTab::Overview => draw_image_overview(app, frame, body, bg),
                ImageSubTab::History => draw_image_history(app, frame, body, bg),
            },
            DockerTab::Volumes => draw_volume_overview(app, frame, body, bg),
            DockerTab::Networks => draw_network_overview(app, frame, body, bg),
            DockerTab::Daemon => draw_daemon_inspector(app, frame, body, bg),
        }
    }

    draw_quick_buttons(app, frame, geom, bg);
}

pub(super) fn draw_fullscreen_inspector(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme.active;
    tokens::draw_surface_card(
        frame,
        area,
        theme,
        Role::SurfaceAlt,
        Role::BorderFocus,
        true,
    );
    let bg = surface_bg(theme, Role::SurfaceAlt);
    draw_inspector_title(app, frame, area, bg);
    let body = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    match app.docker.tab {
        DockerTab::Containers => draw_container_overview(app, frame, body, bg),
        DockerTab::Compose => draw_compose_services(app, frame, body, bg),
        DockerTab::Images => draw_image_overview(app, frame, body, bg),
        DockerTab::Volumes => draw_volume_overview(app, frame, body, bg),
        DockerTab::Networks => draw_network_overview(app, frame, body, bg),
        DockerTab::Daemon => draw_daemon_inspector(app, frame, body, bg),
    }
}

fn draw_inspector_title(app: &App, frame: &mut Frame, area: Rect, bg: Color) {
    let theme = &app.theme.active;
    if !theme.flags.framed || area.width < 12 {
        return;
    }
    let title = match app.docker.tab {
        DockerTab::Containers => app
            .docker
            .selected_container()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| app.ctx.i18n.text("docker.tab.containers").to_owned()),
        DockerTab::Compose => app
            .docker
            .selected_compose()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| app.ctx.i18n.text("docker.tab.compose").to_owned()),
        DockerTab::Images => app
            .docker
            .selected_image()
            .map(|img| img.display_ref())
            .unwrap_or_else(|| app.ctx.i18n.text("docker.tab.images").to_owned()),
        DockerTab::Volumes => app
            .docker
            .selected_volume()
            .map(|v| v.name.clone())
            .unwrap_or_else(|| app.ctx.i18n.text("docker.tab.volumes").to_owned()),
        DockerTab::Networks => app
            .docker
            .selected_network()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| app.ctx.i18n.text("docker.tab.networks").to_owned()),
        DockerTab::Daemon => app.ctx.i18n.text("docker.tab.daemon").to_owned(),
    };
    let label = format!(" {title} ");
    frame.buffer_mut().set_stringn(
        area.x + 2,
        area.y,
        clip_ascii_safe(
            &label,
            area.width.saturating_sub(6) as usize,
            theme.flags.ascii,
        ),
        area.width.saturating_sub(6) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
}

fn draw_sub_pills(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    if geom.sub_pills.is_empty() {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let buf = frame.buffer_mut();
    let (active_idx, keys): (usize, &[&str]) = match app.docker.tab {
        DockerTab::Containers => (
            app.docker.container_sub.index(),
            &[
                ContainerSubTab::Overview.label_key(),
                ContainerSubTab::Logs.label_key(),
                ContainerSubTab::Stats.label_key(),
                ContainerSubTab::Env.label_key(),
            ],
        ),
        DockerTab::Compose => (
            app.docker.compose_sub.index(),
            &[
                ComposeSubTab::Services.label_key(),
                ComposeSubTab::Logs.label_key(),
                ComposeSubTab::Yaml.label_key(),
            ],
        ),
        DockerTab::Images => (
            app.docker.image_sub.index(),
            &[
                ImageSubTab::Overview.label_key(),
                ImageSubTab::History.label_key(),
            ],
        ),
        _ => return,
    };

    for (rect, idx) in &geom.sub_pills {
        if let Some(key) = keys.get(*idx) {
            let hovered = app.hover.is_some_and(|p| rect.contains(p));
            draw_pill_chip(
                buf,
                *rect,
                i.text(key),
                *idx == active_idx,
                hovered,
                theme,
                bg,
            );
        }
    }

    if let Some((first, _)) = geom.sub_pills.first() {
        tokens::draw_rule(
            buf,
            geom.inspector_area.x + 1,
            first.y + 1,
            geom.inspector_area.width.saturating_sub(2),
            theme,
            bg,
        );
    }
}

fn draw_quick_buttons(app: &App, frame: &mut Frame, geom: &DockerGeometry, bg: Color) {
    if geom.quick_btns.is_empty() {
        return;
    }
    let theme = &app.theme.active;
    let buf = frame.buffer_mut();
    for btn in &geom.quick_btns {
        let hovered = app.hover.is_some_and(|p| btn.rect.contains(p));
        let txt = format!("[{}] {}", btn.key, btn.label);
        draw_pill_chip(buf, btn.rect, &txt, false, hovered, theme, bg);
    }
}

fn render_lines(app: &App, frame: &mut Frame, body: Rect, lines: Vec<Line<'static>>, bg: Color) {
    let scroll = app.docker.inspector_scroll;
    let para = Paragraph::new(lines)
        .style(Style::default().bg(bg))
        .scroll((scroll, 0));
    frame.render_widget(para, body);
}

fn kv_line(
    theme: &crate::theme::Theme,
    label: &str,
    value: &str,
    tone: Tone,
    width: usize,
) -> Line<'static> {
    let lbl_w = 10usize.min(width / 3).max(6);
    let clipped_lbl = clip_ascii_safe(label, lbl_w, theme.flags.ascii);
    let pad = lbl_w.saturating_sub(clipped_lbl.width()) + 1;
    let val_w = width.saturating_sub(lbl_w + 1);
    let clipped_val = clip_ascii_safe(value, val_w, theme.flags.ascii);
    Line::from(vec![
        Span::styled(clipped_lbl, tokens::text(theme, Level::Tertiary)),
        Span::raw(" ".repeat(pad)),
        Span::styled(clipped_val, tokens::tone_style(theme, tone)),
    ])
}

fn draw_cmd_strip(app: &App, buf: &mut ratatui::buffer::Buffer, rect: Rect, cmd: &str, bg: Color) {
    let theme = &app.theme.active;
    let strip_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        bg
    };
    fill_rect_bg(buf, rect, strip_bg);
    let text = format!(" $ {cmd}");
    let copy_tag = " [y] ";
    let budget = rect.width.saturating_sub(copy_tag.len() as u16) as usize;
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&text, budget, theme.flags.ascii),
        budget,
        tokens::fg(theme, Role::Accent).bg(strip_bg),
    );
    tokens::put_right(
        buf,
        rect,
        copy_tag,
        tokens::text(theme, Level::Tertiary).bg(strip_bg),
    );
    app.docker.last_cmd_rect.set(Some(rect));
}

fn draw_container_overview(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(c) = app.docker.selected_container() else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.empty.no_containers").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };

    // Raw JSON toggle (`J`)
    if app.docker.show_raw_json {
        let raw = app
            .docker
            .container_inspects
            .get(&c.id)
            .map(|insp| insp.raw_json.as_str())
            .unwrap_or_else(|| i.text("docker.empty.no_inspect"));
        let lines: Vec<Line<'static>> = raw
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    clip_ascii_safe(l, w, theme.flags.ascii),
                    tokens::text(theme, Level::Secondary),
                ))
            })
            .collect();
        render_lines(app, frame, body, lines, bg);
        return;
    }

    let (sym, st_tone) = container_state_glyph(theme, c.state);
    let mut lines = Vec::new();
    lines.push(kv_line(
        theme,
        i.text("docker.field.status"),
        &format!("{sym} {}", c.status),
        st_tone,
        w,
    ));
    lines.push(kv_line(
        theme,
        i.text("docker.field.image"),
        &c.image,
        Tone::Plain,
        w,
    ));
    lines.push(kv_line(
        theme,
        i.text("docker.field.id"),
        &c.short_id,
        Tone::Muted,
        w,
    ));
    if let Some(proj) = &c.compose_project {
        let svc = c.compose_service.as_deref().unwrap_or("-");
        lines.push(kv_line(
            theme,
            i.text("docker.field.compose"),
            &format!("{proj} / {svc}"),
            Tone::Info,
            w,
        ));
    }

    if let Some(insp) = app.docker.container_inspects.get(&c.id) {
        if !insp.networks.is_empty() {
            let net_s = insp
                .networks
                .iter()
                .map(|n| {
                    if n.ipv4.is_empty() {
                        n.name.clone()
                    } else {
                        format!("{} ({})", n.name, n.ipv4)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(kv_line(
                theme,
                i.text("docker.field.networks"),
                &net_s,
                Tone::Plain,
                w,
            ));
        }
        if !insp.ports.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.ports"),
                &insp.ports.join(", "),
                Tone::Accent,
                w,
            ));
        }
        if !insp.mounts.is_empty() {
            for (idx, m) in insp.mounts.iter().take(4).enumerate() {
                let lbl = if idx == 0 {
                    i.text("docker.field.mounts")
                } else {
                    ""
                };
                let mode = if m.rw { "rw" } else { "ro" };
                lines.push(kv_line(
                    theme,
                    lbl,
                    &format!("{} -> {} ({mode})", m.source, m.destination),
                    Tone::Muted,
                    w,
                ));
            }
        }
        if !insp.restart_policy.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.restart"),
                &format!("{} ({})", insp.restart_policy, insp.restart_count),
                Tone::Plain,
                w,
            ));
        }
        if let Some(h) = &insp.health_status {
            let htone = if h == "healthy" {
                Tone::Success
            } else if h == "unhealthy" {
                Tone::Danger
            } else {
                Tone::Warning
            };
            lines.push(kv_line(theme, i.text("docker.field.health"), h, htone, w));
        }
        if !insp.cmd.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.command"),
                &insp.cmd.join(" "),
                Tone::Muted,
                w,
            ));
        }
    } else {
        if !c.networks.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.networks"),
                &c.networks.join(", "),
                Tone::Plain,
                w,
            ));
        }
        if !c.ports.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.ports"),
                &c.ports.join(", "),
                Tone::Accent,
                w,
            ));
        }
        if !c.command.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.command"),
                &c.command,
                Tone::Muted,
                w,
            ));
        }
    }

    let list_body = if body.height >= 4 {
        Rect::new(body.x, body.y, body.width, body.height - 2)
    } else {
        body
    };
    render_lines(app, frame, list_body, lines, bg);

    if body.height >= 4
        && let Some(cmd) = app.docker.current_cli_hint()
    {
        let strip = Rect::new(body.x, body.bottom() - 1, body.width, 1);
        draw_cmd_strip(app, frame.buffer_mut(), strip, &cmd, bg);
    }
}

fn draw_container_stats(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(c) = app.docker.selected_container() else {
        return;
    };
    let Some(st) = app.docker.snapshot.stats.get(&c.id) else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.empty.no_stats").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };

    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    let bar_w = if w >= 24 {
        w.saturating_sub(14).clamp(8, 30)
    } else {
        w.saturating_sub(12).clamp(4, 8)
    };
    let mut lines = Vec::new();

    let mut cpu_spans = vec![Span::styled("CPU  ", tokens::text(theme, Level::Secondary))];
    cpu_spans.extend(tokens::gauge_spans_fx(
        st.cpu_percent,
        bar_w,
        theme,
        elapsed,
        reduced,
        None,
    ));
    cpu_spans.push(Span::styled(
        format!(" {:>5.1}%", st.cpu_percent),
        Style::default()
            .fg(crate::tui::fx::meter_pct_color(
                st.cpu_percent,
                theme,
                elapsed,
                reduced,
            ))
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(cpu_spans));

    if let Some(hist) = app.docker.snapshot.cpu_history.get(&c.id) {
        let spark = tokens::sparkline(hist, bar_w.min(24), theme);
        lines.push(Line::from(vec![
            Span::styled("     ", tokens::text(theme, Level::Tertiary)),
            spark,
        ]));
    }

    let mut mem_spans = vec![Span::styled("MEM  ", tokens::text(theme, Level::Secondary))];
    mem_spans.extend(tokens::gauge_spans_fx(
        st.mem_percent,
        bar_w,
        theme,
        elapsed,
        reduced,
        None,
    ));
    mem_spans.push(Span::styled(
        format!(" {:>5.1}%", st.mem_percent),
        Style::default()
            .fg(crate::tui::fx::meter_pct_color(
                st.mem_percent,
                theme,
                elapsed,
                reduced,
            ))
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(mem_spans));
    lines.push(kv_line(
        theme,
        i.text("docker.col.mem"),
        &st.mem_usage_text,
        Tone::Muted,
        w,
    ));
    lines.push(Line::raw(""));
    lines.push(kv_line(
        theme,
        i.text("docker.field.net_io"),
        &st.net_io_text,
        Tone::Info,
        w,
    ));
    lines.push(kv_line(
        theme,
        i.text("docker.field.block_io"),
        &st.block_io_text,
        Tone::Plain,
        w,
    ));
    lines.push(kv_line(
        theme,
        i.text("docker.field.pids"),
        &st.pids.to_string(),
        Tone::Plain,
        w,
    ));

    if let Some(insp) = app.docker.container_inspects.get(&c.id)
        && !insp.top.is_empty()
    {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            i.text("docker.field.processes").to_owned(),
            tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
        )));
        for row in insp.top.iter().take(6) {
            let line = format!("{:>6} {:>4}% {}", row.pid, row.cpu, row.command);
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&line, w, theme.flags.ascii),
                tokens::text(theme, Level::Secondary),
            )));
        }
    }

    render_lines(app, frame, body, lines, bg);
}

fn draw_container_env(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(c) = app.docker.selected_container() else {
        return;
    };
    let Some(insp) = app.docker.container_inspects.get(&c.id) else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.empty.no_inspect").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };

    let mut lines = Vec::new();
    let (banner, btone) = if app.docker.reveal_secrets {
        (i.text("docker.field.env_revealed"), Tone::Warning)
    } else {
        (i.text("docker.field.env_masked"), Tone::Info)
    };
    lines.push(Line::from(Span::styled(
        clip_ascii_safe(banner, w, theme.flags.ascii),
        tokens::tone_style(theme, btone).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));

    for var in &insp.env {
        let shown_val = if var.sensitive && !app.docker.reveal_secrets {
            if theme.flags.ascii {
                "******"
            } else {
                "••••••"
            }
        } else {
            &var.value
        };
        let val_tone = if var.sensitive {
            Tone::Warning
        } else {
            Tone::Plain
        };
        let raw = format!("{}={shown_val}", var.key);
        lines.push(Line::from(Span::styled(
            clip_ascii_safe(&raw, w, theme.flags.ascii),
            tokens::tone_style(theme, val_tone),
        )));
    }

    if !insp.mounts.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            i.text("docker.field.mounts").to_owned(),
            tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
        )));
        for m in &insp.mounts {
            let mode = if m.rw { "rw" } else { "ro" };
            let txt = format!("[{}] {} -> {} ({mode})", m.kind, m.source, m.destination);
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&txt, w, theme.flags.ascii),
                tokens::text(theme, Level::Secondary),
            )));
        }
    }

    render_lines(app, frame, body, lines, bg);
}

fn draw_compose_services(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(proj) = app.docker.selected_compose() else {
        return;
    };
    let mut lines = Vec::new();
    lines.push(kv_line(
        theme,
        i.text("docker.col.status"),
        &proj.status,
        Tone::Info,
        w,
    ));
    if let Some(cfg) = proj.config_files.first() {
        lines.push(kv_line(
            theme,
            i.text("docker.col.config"),
            cfg,
            Tone::Muted,
            w,
        ));
    }
    if proj.synthesized_from_labels {
        lines.push(Line::from(Span::styled(
            i.text("docker.badge.from_labels").to_owned(),
            tokens::fg(theme, Role::Warning),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        i.text("docker.sub.services").to_owned(),
        tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
    )));

    for c in &app.docker.snapshot.containers {
        if c.compose_project.as_deref() == Some(proj.name.as_str()) {
            let (sym, tone) = container_state_glyph(theme, c.state);
            let svc = c.compose_service.as_deref().unwrap_or(&c.name);
            let ports = c.compact_ports();
            let row = format!("{sym} {svc} ({})  {ports}", c.status);
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&row, w, theme.flags.ascii),
                tokens::tone_style(theme, tone),
            )));
        }
    }
    render_lines(app, frame, body, lines, bg);
}

fn draw_compose_yaml(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(proj) = app.docker.selected_compose() else {
        return;
    };
    let Some(path) = proj.config_files.first() else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.toast.no_config_file").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };
    let Some(text) = app.docker.compose_yamls.get(path) else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.empty.no_inspect").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };
    let lines: Vec<Line<'static>> = text
        .lines()
        .map(|raw| {
            let trimmed = raw.trim_start();
            let style = if trimmed.starts_with('#') {
                tokens::text(theme, Level::Tertiary)
            } else if trimmed.ends_with(':') {
                tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD)
            } else {
                tokens::text(theme, Level::Primary)
            };
            Line::from(Span::styled(
                clip_ascii_safe(raw, w, theme.flags.ascii),
                style,
            ))
        })
        .collect();
    render_lines(app, frame, body, lines, bg);
}

fn draw_image_overview(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(img) = app.docker.selected_image() else {
        return;
    };
    let mut lines = vec![
        kv_line(
            theme,
            i.text("docker.col.repo"),
            &img.display_ref(),
            Tone::Plain,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.col.id"),
            &img.short_id,
            Tone::Muted,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.col.size"),
            &img.size_text,
            Tone::Info,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.used_by"),
            &if img.used_by.is_empty() {
                i.text("docker.badge.unused").to_owned()
            } else {
                img.used_by.join(", ")
            },
            if img.used_by.is_empty() {
                Tone::Muted
            } else {
                Tone::Success
            },
            w,
        ),
    ];
    if let Some(insp) = app.docker.image_inspects.get(&img.id) {
        lines.push(kv_line(
            theme,
            "OS/Arch",
            &format!("{}/{}", insp.os, insp.arch),
            Tone::Plain,
            w,
        ));
        if !insp.exposed_ports.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.exposed"),
                &insp.exposed_ports.join(", "),
                Tone::Accent,
                w,
            ));
        }
        if !insp.cmd.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.command"),
                &insp.cmd.join(" "),
                Tone::Muted,
                w,
            ));
        }
    }
    render_lines(app, frame, body, lines, bg);
}

fn draw_image_history(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(img) = app.docker.selected_image() else {
        return;
    };
    let Some(insp) = app.docker.image_inspects.get(&img.id) else {
        render_lines(
            app,
            frame,
            body,
            vec![Line::from(Span::styled(
                i.text("docker.empty.no_inspect").to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))],
            bg,
        );
        return;
    };
    let lines: Vec<Line<'static>> = insp
        .history
        .iter()
        .map(|h| {
            let row = format!("{:>8}  {}", h.size_text, h.created_by);
            Line::from(Span::styled(
                clip_ascii_safe(&row, w, theme.flags.ascii),
                tokens::text(
                    theme,
                    if h.size_bytes > 0 {
                        Level::Primary
                    } else {
                        Level::Tertiary
                    },
                ),
            ))
        })
        .collect();
    render_lines(app, frame, body, lines, bg);
}

fn draw_volume_overview(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(vol) = app.docker.selected_volume() else {
        return;
    };
    let lines = vec![
        kv_line(theme, i.text("docker.col.name"), &vol.name, Tone::Plain, w),
        kv_line(
            theme,
            i.text("docker.col.driver"),
            &vol.driver,
            Tone::Info,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.col.size"),
            &vol.size_text,
            Tone::Plain,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.mountpoint"),
            &vol.mountpoint,
            Tone::Muted,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.used_by"),
            &if vol.used_by.is_empty() {
                i.text("docker.badge.unused").to_owned()
            } else {
                vol.used_by.join(", ")
            },
            if vol.used_by.is_empty() {
                Tone::Warning
            } else {
                Tone::Success
            },
            w,
        ),
    ];
    render_lines(app, frame, body, lines, bg);
}

fn draw_network_overview(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let Some(net) = app.docker.selected_network() else {
        return;
    };
    let mut lines = vec![
        kv_line(theme, i.text("docker.col.name"), &net.name, Tone::Plain, w),
        kv_line(
            theme,
            i.text("docker.col.driver"),
            &net.driver,
            Tone::Info,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.subnet"),
            if net.subnet.is_empty() {
                "-"
            } else {
                &net.subnet
            },
            Tone::Plain,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.gateway"),
            if net.gateway.is_empty() {
                "-"
            } else {
                &net.gateway
            },
            Tone::Muted,
            w,
        ),
        kv_line(
            theme,
            i.text("docker.field.ipv6_subnet"),
            if net.ipv6_subnet.is_empty() {
                if net.ipv6 { "enabled" } else { "-" }
            } else {
                &net.ipv6_subnet
            },
            if net.ipv6 { Tone::Success } else { Tone::Muted },
            w,
        ),
    ];
    if !net.containers.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            i.text("docker.field.attached").to_owned(),
            tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
        )));
        for (cname, addr) in &net.containers {
            lines.push(kv_line(theme, cname, addr, Tone::Plain, w));
        }
    }
    render_lines(app, frame, body, lines, bg);
}

fn draw_daemon_inspector(app: &App, frame: &mut Frame, body: Rect, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let w = body.width as usize;
    let mut lines = Vec::new();

    if let Some(eng) = &app.docker.snapshot.engine {
        if !eng.server_version.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.version"),
                &format!("v{} (API {})", eng.server_version, eng.api_version),
                Tone::Success,
                w,
            ));
        }
        lines.push(kv_line(
            theme,
            i.text("docker.field.compose_ver"),
            eng.compose_version.as_deref().unwrap_or("-"),
            Tone::Info,
            w,
        ));
        lines.push(kv_line(
            theme,
            i.text("docker.field.storage"),
            &format!("{} / cgroup v{}", eng.storage_driver, eng.cgroup_version),
            Tone::Plain,
            w,
        ));
        if !eng.root_dir.is_empty() {
            lines.push(kv_line(
                theme,
                i.text("docker.field.root_dir"),
                &eng.root_dir,
                Tone::Muted,
                w,
            ));
        }
        let mirrors = if eng.daemon_mirrors.is_empty() {
            "-".to_owned()
        } else {
            eng.daemon_mirrors.join(", ")
        };
        lines.push(kv_line(
            theme,
            i.text("docker.field.mirrors"),
            &mirrors,
            Tone::Accent,
            w,
        ));
        let ipv6_s = match (eng.daemon_ipv6, &eng.daemon_ipv6_cidr) {
            (Some(true), Some(cidr)) => format!("on ({cidr})"),
            (Some(true), None) => "on".into(),
            (Some(false), _) => "off".into(),
            (None, _) => "-".into(),
        };
        lines.push(kv_line(
            theme,
            i.text("docker.field.ipv6"),
            &ipv6_s,
            Tone::Plain,
            w,
        ));
    }

    if !app.docker.snapshot.disk_usage.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            i.text("docker.daemon.df_section").to_owned(),
            tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
        )));
        for row in &app.docker.snapshot.disk_usage {
            let val = format!(
                "{} ({} {})",
                row.size_text,
                i.text("docker.field.reclaimable"),
                format_bytes(row.reclaimable_bytes)
            );
            lines.push(kv_line(theme, &row.kind, &val, Tone::Muted, w));
        }
    }

    if let DaemonEntry::CqsAction(id) = app.docker.selected_daemon_entry()
        && let Ok(spec) = actions::find(id)
    {
        lines.push(Line::raw(""));
        for wrapped in wrap_words(i.text(&spec.description()), w) {
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&wrapped, w, theme.flags.ascii),
                tokens::text(theme, Level::Secondary),
            )));
        }
    }

    let list_body = if body.height >= 4 {
        Rect::new(body.x, body.y, body.width, body.height - 2)
    } else {
        body
    };
    render_lines(app, frame, list_body, lines, bg);

    if body.height >= 4
        && let Some(cmd) = app.docker.current_cli_hint()
    {
        let strip = Rect::new(body.x, body.bottom() - 1, body.width, 1);
        draw_cmd_strip(app, frame.buffer_mut(), strip, &cmd, bg);
    }
    let _ = Glyph::Separator;
}
