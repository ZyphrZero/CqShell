//! Left collapsible Sidebar (and compact-mode floating drawer overlay).

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
        layout::{SidebarGeometry, SidebarRow},
        state::{App, GROUPS, NavTarget},
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &SidebarGeometry) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    let bg_role = if geom.overlay {
        Role::Elevated
    } else {
        Role::Sidebar
    };
    let border_role = if geom.overlay {
        Role::BorderFocus
    } else {
        Role::Border
    };
    tokens::draw_surface_card_fx(
        frame,
        geom.area,
        theme,
        bg_role,
        border_role,
        geom.overlay,
        elapsed,
        reduced,
    );

    let base_bg = if theme.flags.paint_bg || geom.overlay {
        tokens::color(theme, bg_role)
    } else {
        Color::Reset
    };
    let header_bg = if geom.overlay {
        base_bg
    } else {
        tokens::card_title_bg(theme, bg_role)
    };
    let buf = frame.buffer_mut();

    // Header row inside sidebar
    let header_y = geom.area.y;
    let is_framed = geom.overlay || theme.flags.framed;
    if geom.area.width >= 6 {
        let title = tokens::format_card_title(theme, i.text("ui.categories"), is_framed);
        let title_fg = if geom.overlay {
            tokens::color(theme, Role::Accent)
        } else if !is_framed && theme.style == crate::theme::UiStyle::Bento {
            tokens::color(theme, Role::Text3)
        } else {
            tokens::card_title_fg(theme, false)
        };
        buf.set_stringn(
            geom.area.x + 2,
            header_y,
            clip_ascii_safe(
                &title,
                geom.area.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            geom.area.width.saturating_sub(4) as usize,
            Style::default()
                .fg(title_fg)
                .bg(header_bg)
                .add_modifier(Modifier::BOLD),
        );
    }

    let top_inset = if geom.overlay {
        1
    } else {
        tokens::card_top_inset(theme, geom.area.height)
    };
    let inner_y = geom.area.y + top_inset;
    let framed_bottom = u16::from(geom.overlay || theme.flags.framed);
    let footer_h = if geom.collapse_btn.is_some() {
        2 + framed_bottom
    } else {
        0
    };
    let list_h = geom.area.height.saturating_sub(top_inset + footer_h) as usize;
    let mihomo_running = matches!(
        app.snapshot.local.mihomo_state,
        Reading::Value(ClashState::Running(_))
    );
    let pulse = fx::pulse_factor(
        app.now.saturating_duration_since(app.started),
        app.reduced_motion(),
    );

    for (offset, row) in geom.rows.iter().skip(geom.scroll).take(list_h).enumerate() {
        let y = inner_y + offset as u16;
        let row_rect = Rect::new(geom.area.x + 1, y, geom.area.width.saturating_sub(2), 1);
        match row {
            SidebarRow::Spacer => {}
            SidebarRow::Rule => {
                tokens::draw_rule(
                    buf,
                    row_rect.x + 1,
                    y,
                    row_rect.width.saturating_sub(2),
                    theme,
                    base_bg,
                );
            }
            SidebarRow::Section(key) => {
                let label = clip_ascii_safe(
                    i.text(key),
                    row_rect.width.saturating_sub(3) as usize,
                    theme.flags.ascii,
                );
                let label_style = tokens::text(theme, Level::Tertiary)
                    .bg(base_bg)
                    .add_modifier(Modifier::BOLD);
                buf.set_stringn(
                    row_rect.x + 1,
                    y,
                    &label,
                    row_rect.width.saturating_sub(3) as usize,
                    label_style,
                );
                let rule_x = row_rect.x + 2 + label.width() as u16;
                let rule_w = row_rect.right().saturating_sub(rule_x + 1);
                if rule_w >= 2 {
                    tokens::draw_rule(buf, rule_x, y, rule_w, theme, base_bg);
                }
            }
            SidebarRow::Item(target) => {
                let active = *target == app.nav;
                let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
                let row_bg = if active {
                    tokens::color(theme, Role::Selection)
                } else if hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    base_bg
                };

                // Paint full row highlight
                for col in row_rect.left()..row_rect.right() {
                    buf[(col, y)].set_symbol(" ").set_bg(row_bg);
                }

                // Active left bar marker
                let mark = if active {
                    tokens::glyph(theme, Glyph::ActiveBar)
                } else {
                    " "
                };
                let mark_style = Style::default()
                    .fg(tokens::animated_selected_accent(theme, elapsed, reduced))
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD);
                buf.set_stringn(row_rect.x, y, mark, 1, mark_style);

                // Prefix icon or number shortcut
                let (prefix, label, badge, badge_color) = match *target {
                    NavTarget::Overview => (
                        tokens::glyph(theme, Glyph::Overview).to_owned(),
                        i.text("nav.overview").to_owned(),
                        String::new(),
                        tokens::color(theme, Role::Accent),
                    ),
                    NavTarget::Optimization => {
                        let b = if app.marked.is_empty() {
                            String::new()
                        } else {
                            app.marked.len().to_string()
                        };
                        (
                            tokens::glyph(theme, Glyph::Optimize).to_owned(),
                            i.text("group.optimization").to_owned(),
                            b,
                            tokens::color(theme, Role::Accent),
                        )
                    }
                    NavTarget::Group(g) => {
                        let num = app
                            .category_order
                            .iter()
                            .position(|id| *id == g)
                            .and_then(|idx| (idx < 9).then(|| (idx + 1).to_string()))
                            .unwrap_or_else(|| " ".into());
                        let is_clash = GROUPS[g] == "clash";
                        let is_docker = GROUPS[g] == "docker";
                        let (b, col) = if is_clash && mihomo_running {
                            (
                                tokens::glyph(theme, Glyph::DotOn).to_owned(),
                                fx::blend_ratatui(
                                    tokens::color(theme, Role::Text3),
                                    tokens::color(theme, Role::Success),
                                    pulse,
                                ),
                            )
                        } else if is_docker && let Some(running) = app.docker.running_badge() {
                            (
                                running.to_string(),
                                if running > 0 {
                                    tokens::color(theme, Role::Success)
                                } else {
                                    tokens::color(theme, Role::Text3)
                                },
                            )
                        } else {
                            (String::new(), tokens::color(theme, Role::Text3))
                        };
                        (
                            num,
                            i.text(&format!("group.{}", GROUPS[g])).to_owned(),
                            b,
                            col,
                        )
                    }
                };

                let prefix_fg = if active {
                    tokens::selected_accent(theme)
                } else {
                    tokens::color(theme, Role::Text3)
                };
                let text_fg = if active {
                    tokens::selected_fg(theme)
                } else if hovered {
                    tokens::color(theme, Role::Text1)
                } else {
                    tokens::color(theme, Role::Text2)
                };

                let text_x = row_rect.x + 2;
                buf.set_stringn(
                    text_x,
                    y,
                    &prefix,
                    2,
                    Style::default().fg(prefix_fg).bg(row_bg),
                );
                let badge_w = if badge.is_empty() {
                    0
                } else {
                    badge.width() + 2
                };
                let label_budget = row_rect.width.saturating_sub(4 + badge_w as u16) as usize;
                let clipped = clip_ascii_safe(&label, label_budget, theme.flags.ascii);
                let mut label_style = Style::default().fg(text_fg).bg(row_bg);
                if active {
                    label_style = label_style.add_modifier(Modifier::BOLD);
                }
                buf.set_stringn(text_x + 2, y, &clipped, label_budget, label_style);

                if !badge.is_empty() {
                    let b_style = Style::default()
                        .fg(if active && theme.flags.solid_selection {
                            tokens::selected_fg(theme)
                        } else {
                            badge_color
                        })
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD);
                    tokens::put_right(
                        buf,
                        Rect::new(row_rect.x, y, row_rect.width.saturating_sub(1), 1),
                        &badge,
                        b_style,
                    );
                }
            }
        }
    }

    // Bottom subtle sidebar action bar: `[ 折叠` | `T 风格`
    if let (Some(col_btn), Some(thm_btn)) = (geom.collapse_btn, geom.theme_btn) {
        let rule_y = col_btn.y.saturating_sub(1);
        tokens::draw_rule(
            buf,
            geom.area.x + 2,
            rule_y,
            geom.area.width.saturating_sub(4),
            theme,
            base_bg,
        );

        let c_hover = app.hover.is_some_and(|p| col_btn.contains(p));
        let c_bg = if c_hover && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            base_bg
        };
        let c_text = clip_ascii_safe(
            &format!("[ {}", i.text("nav.collapse")),
            col_btn.width as usize,
            theme.flags.ascii,
        );
        buf.set_stringn(
            col_btn.x + 1,
            col_btn.y,
            &c_text,
            col_btn.width.saturating_sub(1) as usize,
            Style::default()
                .fg(tokens::color(
                    theme,
                    if c_hover { Role::Accent } else { Role::Text3 },
                ))
                .bg(c_bg),
        );

        let t_hover = app.hover.is_some_and(|p| thm_btn.contains(p));
        let t_bg = if t_hover && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            base_bg
        };
        let t_text = clip_ascii_safe(
            &format!("T {}", i.text("nav.theme")),
            thm_btn.width as usize,
            theme.flags.ascii,
        );
        tokens::put_right(
            buf,
            Rect::new(thm_btn.x, thm_btn.y, thm_btn.width.saturating_sub(1), 1),
            &t_text,
            Style::default()
                .fg(tokens::color(
                    theme,
                    if t_hover { Role::Accent } else { Role::Text3 },
                ))
                .bg(t_bg),
        );
    }
}
