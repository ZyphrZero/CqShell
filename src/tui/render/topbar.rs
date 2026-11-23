//! Row 0: Ultra-clean Top Status Strip with brand mark, breadcrumb, compact live telemetry
//! pills (or pulsing active task pill), host, privilege badge, and clickable language toggle.

use super::{
    clip_ascii_safe,
    tokens::{self, Glyph},
};
use crate::{
    dashboard::Reading,
    ops::clash::State as ClashState,
    theme::{Level, Role},
    tui::{
        fx,
        layout::{self, LayoutRegions},
        state::{App, GROUPS, NavTarget, Row, Tone},
    },
};
use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, regions: &LayoutRegions) {
    let bar = regions.topbar;
    if bar.height == 0 || bar.width == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Sidebar)
    } else {
        Color::Reset
    };
    tokens::fill_surface(frame.buffer_mut(), bar, theme, Role::Sidebar);

    let sep = format!(" {} ", tokens::glyph(theme, Glyph::Separator));
    let chev = format!(" {} ", tokens::glyph(theme, Glyph::Chevron));
    let hover = app.hover;
    let mobile = bar.width < 80;

    // Right: compact telemetry / running pill + host + role badge + language toggle
    let mut right: Vec<Span<'static>> = Vec::new();
    let elapsed = app.now.saturating_duration_since(app.started);
    let pulse = fx::pulse_factor(elapsed, app.reduced_motion());

    if app.worker.is_some() {
        let pill_text = layout::topbar_running_pill_text(app, bar.width);
        let pill_fg = fx::blend_ratatui(
            tokens::color(theme, Role::Text2),
            tokens::color(theme, Role::Info),
            pulse,
        );
        right.push(Span::styled(
            pill_text,
            Style::default()
                .fg(pill_fg)
                .bg(tokens::color(theme, Role::SurfaceAlt))
                .add_modifier(Modifier::BOLD),
        ));
        if !mobile {
            right.push(Span::styled(
                sep.clone(),
                tokens::text(theme, Level::Tertiary).bg(bg),
            ));
        } else {
            right.push(Span::styled(" ", Style::default().bg(bg)));
        }
    } else if bar.width >= 108 {
        let chip_bg = if theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        if app.is_docker_view()
            && !app.docker.fullscreen
            && app.docker.snapshot.availability.is_ready()
            && bar.width >= 116
        {
            let running = app.docker.snapshot.running_containers();
            let total = app.docker.snapshot.containers.len();
            right.push(Span::styled(
                format!(
                    " {} Docker {running}/{total} ",
                    tokens::glyph(theme, Glyph::DotOn)
                ),
                Style::default()
                    .fg(tokens::color(
                        theme,
                        if running > 0 {
                            Role::Success
                        } else {
                            Role::Text2
                        },
                    ))
                    .bg(chip_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            right.push(Span::styled(" ", Style::default().bg(bg)));
        }
        if let Reading::Value(cpu) = app.snapshot.local.cpu_usage {
            right.push(Span::styled(
                " CPU ",
                tokens::text(theme, Level::Tertiary).bg(chip_bg),
            ));
            right.push(Span::styled(
                format!("{cpu:.0}% "),
                Style::default()
                    .fg(fx::meter_color(f64::from(cpu), theme))
                    .bg(chip_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            right.push(Span::styled(" ", Style::default().bg(bg)));
        }
        if let Reading::Value(mem) = &app.snapshot.local.memory
            && mem.total > 0
        {
            let pct = mem.percent();
            right.push(Span::styled(
                " MEM ",
                tokens::text(theme, Level::Tertiary).bg(chip_bg),
            ));
            right.push(Span::styled(
                format!("{pct:.0}% "),
                Style::default()
                    .fg(fx::meter_color(pct, theme))
                    .bg(chip_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            right.push(Span::styled(" ", Style::default().bg(bg)));
        }
        if matches!(
            app.snapshot.local.mihomo_state,
            Reading::Value(ClashState::Running(_))
        ) {
            let dot_fg = fx::blend_ratatui(
                tokens::color(theme, Role::Text3),
                tokens::color(theme, Role::Success),
                pulse,
            );
            right.push(Span::styled(
                format!(" {} Mihomo ", tokens::glyph(theme, Glyph::DotOn)),
                Style::default()
                    .fg(dot_fg)
                    .bg(chip_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            right.push(Span::styled(" ", Style::default().bg(bg)));
        }
    }

    if bar.width >= 90
        && let Reading::Value(host) = &app.snapshot.local.host
        && !host.is_empty()
    {
        right.push(Span::styled(
            clip_ascii_safe(host, 18, theme.flags.ascii),
            tokens::text(theme, Level::Primary).bg(bg),
        ));
        right.push(Span::styled(
            sep.clone(),
            tokens::text(theme, Level::Tertiary).bg(bg),
        ));
    }

    let (role_key, role_tone) = if app.platform_error.is_some() {
        ("ui.mode_preview", Tone::Warning)
    } else if app.root {
        ("ui.mode_root", Tone::Danger)
    } else {
        ("ui.mode_user", Tone::Muted)
    };
    if !mobile || app.worker.is_none() {
        if bar.width < 52 {
            right.push(Span::styled(
                tokens::glyph(theme, Glyph::DotOn).to_owned(),
                tokens::tone_style(theme, role_tone).bg(bg),
            ));
            right.push(Span::styled(" ", Style::default().bg(bg)));
        } else {
            right.push(Span::styled(
                format!(
                    "{} {}",
                    tokens::glyph(theme, Glyph::DotOn),
                    i.text(role_key)
                ),
                tokens::tone_style(theme, role_tone).bg(bg),
            ));
            right.push(Span::styled(
                if mobile { " " } else { "  " },
                Style::default().bg(bg),
            ));
        }
    }

    let lang_hovered = regions
        .topbar_hits
        .lang_btn
        .zip(hover)
        .is_some_and(|(r, p)| r.contains(p));
    let lang_bg = if lang_hovered {
        tokens::color(theme, Role::Selection)
    } else if theme.flags.paint_bg {
        tokens::color(theme, Role::SurfaceAlt)
    } else {
        bg
    };
    let lang_label = layout::topbar_lang_label(app, bar.width);
    right.push(Span::styled(
        format!(" {lang_label} "),
        tokens::text(theme, Level::Secondary).bg(lang_bg),
    ));
    right.push(Span::styled(" ", Style::default().bg(bg)));

    // Left: optional nav toggle pill + brand + optional version + breadcrumb
    let mut left: Vec<Span<'static>> = vec![Span::raw(" ")];

    if let Some(nav_rect) = regions.topbar_hits.nav_toggle {
        let hovered = hover.is_some_and(|p| nav_rect.contains(p));
        let nav_bg = if hovered {
            tokens::color(theme, Role::Selection)
        } else {
            tokens::color(theme, Role::SurfaceAlt)
        };
        left.push(Span::styled(
            format!(
                " {} {} ",
                tokens::glyph(theme, Glyph::Menu),
                i.text("nav.toggle")
            ),
            Style::default()
                .fg(tokens::color(theme, Role::Accent))
                .bg(nav_bg)
                .add_modifier(Modifier::BOLD),
        ));
        left.push(Span::raw(" "));
    }

    let show_brand_text = bar.width >= 56 && !(mobile && app.worker.is_some());
    let brand_fg = if theme.flags.ascii {
        tokens::color(theme, Role::Accent)
    } else {
        fx::focus_glow(
            theme,
            tokens::color(theme, Role::Accent),
            elapsed,
            app.reduced_motion(),
        )
    };
    if theme.flags.ascii || theme.style == crate::theme::UiStyle::Editorial {
        let brand = if show_brand_text {
            format!("{} CqShell", tokens::glyph(theme, Glyph::Brand))
        } else {
            tokens::glyph(theme, Glyph::Brand).to_owned()
        };
        left.push(Span::styled(
            brand,
            Style::default()
                .fg(brand_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ));
    } else if matches!(
        theme.style,
        crate::theme::UiStyle::Atelier | crate::theme::UiStyle::Heavy
    ) {
        let brand = if show_brand_text {
            format!("[{} CqShell]", tokens::glyph(theme, Glyph::Brand))
        } else {
            format!("[{}]", tokens::glyph(theme, Glyph::Brand))
        };
        left.push(Span::styled(
            brand,
            Style::default()
                .fg(brand_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        let brand_bg = if theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        let brand = if show_brand_text {
            format!(" {} CqShell ", tokens::glyph(theme, Glyph::Brand))
        } else {
            format!(" {} ", tokens::glyph(theme, Glyph::Brand))
        };
        left.push(Span::styled(
            brand,
            Style::default()
                .fg(brand_fg)
                .bg(brand_bg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !mobile {
        left.push(Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            tokens::text(theme, Level::Tertiary).bg(bg),
        ));
    }

    let right_w: usize = right.iter().map(Span::width).sum();
    let total_w = bar.width as usize;

    if mobile {
        // Compute compact breadcrumb clipped to remaining budget so we never leave a dangling separator.
        let prefix_w: usize = left.iter().map(Span::width).sum();
        let avail = total_w.saturating_sub(prefix_w + right_w + 1);
        let crumb = match app.nav {
            NavTarget::Overview => i.text("nav.overview").to_owned(),
            NavTarget::Optimization => {
                if app.marked.is_empty() {
                    i.text("group.optimization").to_owned()
                } else {
                    format!("{} ({})", i.text("group.optimization"), app.marked.len())
                }
            }
            NavTarget::Group(g) if app.is_docker_view() => {
                let grp = i.text(&format!("group.{}", GROUPS[g]));
                let tab = i.text(app.docker.tab.label_key());
                format!("{grp}{chev}{tab}")
            }
            NavTarget::Group(g) => i.text(&format!("group.{}", GROUPS[g])).to_owned(),
        };
        if avail >= sep.width() + 4 {
            left.push(Span::styled(
                sep.clone(),
                tokens::text(theme, Level::Tertiary).bg(bg),
            ));
            let crumb_w = avail.saturating_sub(sep.width());
            left.push(Span::styled(
                clip_ascii_safe(&crumb, crumb_w, theme.flags.ascii),
                tokens::text(theme, Level::Primary)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if avail >= 5 {
            left.push(Span::raw(" "));
            let crumb_w = avail.saturating_sub(1);
            left.push(Span::styled(
                clip_ascii_safe(&crumb, crumb_w, theme.flags.ascii),
                tokens::text(theme, Level::Primary)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        left.push(Span::styled(
            sep.clone(),
            tokens::text(theme, Level::Tertiary).bg(bg),
        ));

        match app.nav {
            NavTarget::Overview => {
                left.push(Span::styled(
                    i.text("nav.overview").to_owned(),
                    tokens::text(theme, Level::Primary)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            NavTarget::Optimization => {
                left.push(Span::styled(
                    i.text("group.optimization").to_owned(),
                    tokens::text(theme, Level::Primary)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
                if !app.marked.is_empty() {
                    left.push(Span::styled(
                        format!(" ({})", app.marked.len()),
                        tokens::fg(theme, Role::Accent)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }
            NavTarget::Group(g) if app.is_docker_view() => {
                left.push(Span::styled(
                    i.text(&format!("group.{}", GROUPS[g])).to_owned(),
                    tokens::text(theme, Level::Secondary).bg(bg),
                ));
                left.push(Span::styled(
                    chev.clone(),
                    tokens::text(theme, Level::Tertiary).bg(bg),
                ));
                left.push(Span::styled(
                    i.text(app.docker.tab.label_key()).to_owned(),
                    tokens::fg(theme, Role::Accent)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
                if app.docker.fullscreen && bar.width >= 96 {
                    left.push(Span::styled(
                        format!(" [{}]", i.text("docker.badge.focus")),
                        tokens::text(theme, Level::Tertiary).bg(bg),
                    ));
                }
            }
            NavTarget::Group(g) => {
                left.push(Span::styled(
                    i.text(&format!("group.{}", GROUPS[g])).to_owned(),
                    tokens::text(theme, Level::Secondary).bg(bg),
                ));
                if bar.width >= 100
                    && let Some(sec_idx) = app.section_tab(g)
                    && let Some(secs) = app.group_sections(g)
                    && let Some((sec_id, _)) = secs.get(sec_idx)
                {
                    left.push(Span::styled(
                        chev.clone(),
                        tokens::text(theme, Level::Tertiary).bg(bg),
                    ));
                    left.push(Span::styled(
                        i.text(&format!("section.{}.{sec_id}", GROUPS[g]))
                            .to_owned(),
                        tokens::fg(theme, Role::Accent)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                if bar.width >= 106
                    && let Some(Row::Action(spec)) = app.selected_row()
                {
                    left.push(Span::styled(
                        chev,
                        tokens::text(theme, Level::Tertiary).bg(bg),
                    ));
                    let action_name = clip_ascii_safe(i.text(&spec.label()), 22, theme.flags.ascii);
                    left.push(Span::styled(
                        action_name,
                        tokens::text(theme, Level::Primary).bg(bg),
                    ));
                }
            }
        }
    }

    let left_w: usize = left.iter().map(Span::width).sum();

    let mut line = Vec::new();
    if left_w + right_w + 2 <= total_w {
        line.extend(left);
        line.push(Span::styled(
            " ".repeat(total_w - left_w - right_w),
            Style::default().bg(bg),
        ));
        line.extend(right);
    } else {
        // Clip left side gracefully so right-side status & language stay intact
        let budget = total_w.saturating_sub(right_w + 1);
        let mut used = 0;
        for span in left {
            let sw = span.width();
            if used + sw > budget {
                break;
            }
            used += sw;
            line.push(span);
        }
        if total_w > used + right_w {
            line.push(Span::styled(
                " ".repeat(total_w - used - right_w),
                Style::default().bg(bg),
            ));
        }
        line.extend(right);
    }

    frame.render_widget(Paragraph::new(Line::from(line)), bar);
}
