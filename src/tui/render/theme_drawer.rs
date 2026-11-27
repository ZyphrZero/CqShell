//! Right-docked Live Appearance & Theme Drawer (`T`).
//! Features top dual tabs (`[配色]` / `[样式]`) for independent color palette and UI design
//! style live preview, plus live toggles for transparent canvas (`B`) and reduced motion (`M`).

use super::{
    clip_ascii_safe,
    tokens::{self, Glyph},
    wrap_words,
};
use crate::{
    theme::{self, Level, Role, StyleSelection, ThemeSource},
    tui::{
        layout::ThemeDrawerGeometry,
        state::App,
        theme_drawer::{DrawerTab, STYLE_CHOICES, ThemeEntry},
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

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &ThemeDrawerGeometry) {
    let Some(drawer) = &app.theme_drawer else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    tokens::draw_surface_card_fx(
        frame,
        geom.area,
        theme,
        Role::Elevated,
        Role::BorderFocus,
        true,
        elapsed,
        reduced,
    );
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    let buf = frame.buffer_mut();

    // Header
    let title = tokens::format_card_title(theme, i.text("ui.style_title"), true);
    buf.set_stringn(
        geom.area.x + 2,
        geom.area.y,
        &title,
        geom.area.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    // Top dual tabs: `[配色]` / `[样式]`
    draw_tab_pill(
        app,
        buf,
        geom.tab_palette,
        i.text("ui.drawer_tab_palette"),
        drawer.tab == DrawerTab::Palette,
        bg,
    );
    draw_tab_pill(
        app,
        buf,
        geom.tab_style,
        i.text("ui.drawer_tab_style"),
        drawer.tab == DrawerTab::Style,
        bg,
    );
    if geom.tab_palette.bottom() < geom.list_area.y {
        tokens::draw_rule(
            buf,
            geom.list_area.x,
            geom.tab_palette.bottom(),
            geom.list_area.width,
            theme,
            bg,
        );
    }

    let visible_entries = (geom.list_area.height / geom.row_height.max(1)) as usize;
    match drawer.tab {
        DrawerTab::Palette => draw_palette_list(
            app,
            buf,
            geom,
            drawer,
            visible_entries,
            bg,
            elapsed,
            reduced,
        ),
        DrawerTab::Style => draw_style_list(
            app,
            buf,
            geom,
            drawer,
            visible_entries,
            bg,
            elapsed,
            reduced,
        ),
    }

    if geom.prev_btn.is_some() || geom.next_btn.is_some() {
        tokens::draw_pager_buttons(
            buf,
            geom.prev_btn,
            geom.next_btn,
            geom.can_prev,
            geom.can_next,
            None,
            app.hover,
            theme,
            i,
            bg,
        );
    }

    // Divider above toggles
    let rule_y = geom.transparent_toggle.y.saturating_sub(1);
    if rule_y > geom.list_area.y {
        tokens::draw_rule(
            buf,
            geom.list_area.x,
            rule_y,
            geom.list_area.width,
            theme,
            bg,
        );
    }

    // Live Toggle 1: Transparent Background (`B`)
    draw_toggle_row(
        app,
        buf,
        geom.transparent_toggle,
        "B",
        i.text("ui.style_transparent"),
        app.theme.transparent_bg,
        bg,
    );

    // Live Toggle 2: Reduced Motion (`M`)
    draw_toggle_row(
        app,
        buf,
        geom.motion_toggle,
        "M",
        i.text("ui.style_motion"),
        app.theme.reduced_motion,
        bg,
    );

    // Live Toggle 3: Touch Density Mode (`D`)
    draw_value_row(
        app,
        buf,
        geom.density_toggle,
        "D",
        i.text("ui.style_density"),
        i.text(app.ui_density.label_key()),
        !matches!(app.ui_density, crate::tui::layout::UiDensityMode::Compact),
        bg,
    );

    // Selected entry details
    if geom.details_area.height >= 2 {
        let mut lines = Vec::new();
        let w = geom.details_area.width as usize;
        let sep = tokens::glyph(theme, Glyph::Separator);
        let style_sel = drawer.selected_style();
        let active_style_name = i.text(theme.style.name_key());

        match drawer.tab {
            DrawerTab::Palette => match drawer.selected() {
                ThemeEntry::Valid {
                    definition,
                    custom_path,
                } => {
                    let desc = if let Some(path) = custom_path {
                        i.format("ui.style_custom", &[("path", &path.display().to_string())])
                    } else {
                        let key = format!("theme.{}.description", definition.id);
                        i.text(&key).to_owned()
                    };
                    for row in wrap_words(&desc, w).into_iter().take(2) {
                        lines.push(Line::from(Span::styled(
                            row,
                            tokens::text(theme, Level::Secondary),
                        )));
                    }
                    let bg_mode = if app.theme.transparent_bg {
                        i.text("ui.style_terminal")
                    } else {
                        i.text("ui.style_paint")
                    };
                    let def_style = i.text(definition.default_style.name_key());
                    lines.push(Line::from(Span::styled(
                        clip_ascii_safe(
                            &format!(
                                "{} {sep} {} {sep} {bg_mode}",
                                i.text(&format!(
                                    "ui.style_background.{}",
                                    definition.appearance.name()
                                )),
                                i.format("ui.style_default_tag", &[("style", def_style)])
                            ),
                            w,
                            theme.flags.ascii,
                        ),
                        tokens::text(theme, Level::Tertiary),
                    )));
                    let saved_text = drawer
                        .saved_id
                        .as_deref()
                        .unwrap_or_else(|| i.text("ui.style_unsaved"));
                    lines.push(Line::from(Span::styled(
                        clip_ascii_safe(
                            &format!(
                                "{} {sep} {}",
                                i.format("ui.style_saved", &[("theme", saved_text)]),
                                active_style_name
                            ),
                            w,
                            theme.flags.ascii,
                        ),
                        tokens::text(theme, Level::Tertiary),
                    )));
                }
                ThemeEntry::Invalid { error, .. } => {
                    for row in wrap_words(error, w).into_iter().take(3) {
                        lines.push(Line::from(Span::styled(
                            row,
                            tokens::fg(theme, Role::Danger),
                        )));
                    }
                }
            },
            DrawerTab::Style => {
                let desc = match style_sel {
                    StyleSelection::Auto => i.text("style.auto.description"),
                    StyleSelection::Fixed(s) => i.text(s.description_key()),
                };
                for row in wrap_words(desc, w).into_iter().take(2) {
                    lines.push(Line::from(Span::styled(
                        row,
                        tokens::text(theme, Level::Secondary),
                    )));
                }
                let pal_name = drawer.selected().display_name(i);
                lines.push(Line::from(Span::styled(
                    clip_ascii_safe(
                        &i.format(
                            "ui.style_pairing",
                            &[("palette", &pal_name), ("style", active_style_name)],
                        ),
                        w,
                        theme.flags.ascii,
                    ),
                    tokens::fg(theme, Role::Accent),
                )));
                let saved_style = drawer
                    .saved_style
                    .as_deref()
                    .unwrap_or_else(|| i.text("style.auto.name"));
                lines.push(Line::from(Span::styled(
                    clip_ascii_safe(
                        &i.format("ui.style_saved_style", &[("style", saved_style)]),
                        w,
                        theme.flags.ascii,
                    ),
                    tokens::text(theme, Level::Tertiary),
                )));
            }
        }

        if matches!(app.theme.source, ThemeSource::Flag | ThemeSource::Env)
            || matches!(app.theme.style_source, ThemeSource::Flag | ThemeSource::Env)
        {
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(i.text("ui.style_override"), w, theme.flags.ascii),
                tokens::fg(theme, Role::Warning),
            )));
        }

        frame.render_widget(Paragraph::new(lines), geom.details_area);
    }
}

fn draw_tab_pill(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    label: &str,
    active: bool,
    base_bg: Color,
) {
    if rect.width < 2 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    if rect.height >= 2 {
        let kind = if active {
            tokens::TouchBtnKind::Active
        } else {
            tokens::TouchBtnKind::Secondary
        };
        tokens::draw_touch_button(buf, rect, label, None, kind, hovered, theme, base_bg);
        return;
    }
    if theme.flags.ascii {
        let text = if active {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        };
        let style = if active {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            tokens::text(theme, Level::Secondary)
        };
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&text, rect.width as usize, true),
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
            tokens::color(theme, Role::Selection),
        )
    } else if theme.flags.paint_bg {
        (
            tokens::color(theme, Role::Text2),
            tokens::color(theme, Role::SurfaceAlt),
        )
    } else {
        (tokens::color(theme, Role::Text2), base_bg)
    };
    for col in rect.left()..rect.right() {
        buf[(col, rect.y)].set_symbol(" ").set_bg(pbg);
    }
    let text = format!(" {label} ");
    let mut style = Style::default().fg(fg).bg(pbg);
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    let pad = rect.width.saturating_sub(text.width() as u16) / 2;
    buf.set_stringn(
        rect.x + pad,
        rect.y,
        clip_ascii_safe(&text, rect.width as usize, false),
        rect.width as usize,
        style,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_palette_list(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    geom: &ThemeDrawerGeometry,
    drawer: &crate::tui::theme_drawer::ThemeDrawer,
    list_h: usize,
    bg: Color,
    elapsed: std::time::Duration,
    reduced: bool,
) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    for (offset, entry) in drawer
        .entries
        .iter()
        .skip(geom.list_scroll)
        .take(list_h)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = geom.list_area.y + (offset as u16) * geom.row_height;
        let row_rect = Rect::new(geom.list_area.x, y, geom.list_area.width, geom.row_height);
        let active = idx == drawer.cursor;
        let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
        } else if hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        for ry in row_rect.top()..row_rect.bottom() {
            for col in row_rect.left()..row_rect.right() {
                buf[(col, ry)].set_symbol(" ").set_bg(row_bg);
            }
            if active {
                buf[(row_rect.x, ry)]
                    .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                    .set_style(
                        Style::default()
                            .fg(tokens::animated_selected_accent(theme, elapsed, reduced))
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD),
                    );
            }
        }

        let name = entry.display_name(i);
        let swatches_w = 11u16;
        let name_budget = row_rect.width.saturating_sub(3 + swatches_w) as usize;
        let name_fg = if active {
            tokens::selected_fg(theme)
        } else if !entry.valid() {
            tokens::color(theme, Role::Danger)
        } else {
            tokens::color(theme, Role::Text2)
        };
        let mut name_style = Style::default().fg(name_fg).bg(row_bg);
        if active {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            row_rect.x + 2,
            y,
            clip_ascii_safe(&name, name_budget, theme.flags.ascii),
            name_budget,
            name_style,
        );

        // 5 miniature palette swatch blocks on the right
        if let ThemeEntry::Valid { definition, .. } = entry {
            let preview = theme::resolve(
                definition,
                StyleSelection::Auto,
                &app.theme.background,
                false,
            );
            let roles = [
                Role::Canvas,
                Role::Surface,
                Role::Accent,
                Role::Accent2,
                Role::Success,
            ];
            let start_x = row_rect.right().saturating_sub(swatches_w);
            let block = if theme.flags.ascii { "##" } else { "▬▬" };
            for (si, role) in roles.iter().enumerate() {
                let sw_fg = tokens::color(&preview, *role);
                buf.set_stringn(
                    start_x + (si as u16) * 2,
                    y,
                    block,
                    2,
                    Style::default().fg(sw_fg).bg(row_bg),
                );
            }
        } else {
            let err_mark = tokens::glyph(theme, Glyph::Warning);
            tokens::put_right(
                buf,
                row_rect,
                err_mark,
                Style::default()
                    .fg(tokens::color(theme, Role::Danger))
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_style_list(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    geom: &ThemeDrawerGeometry,
    drawer: &crate::tui::theme_drawer::ThemeDrawer,
    list_h: usize,
    bg: Color,
    elapsed: std::time::Duration,
    reduced: bool,
) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let palette_default = match drawer.selected() {
        ThemeEntry::Valid { definition, .. } => definition.default_style,
        ThemeEntry::Invalid { .. } => theme.default_style,
    };

    for (offset, choice) in STYLE_CHOICES
        .iter()
        .skip(geom.list_scroll)
        .take(list_h)
        .enumerate()
    {
        let idx = geom.list_scroll + offset;
        let y = geom.list_area.y + (offset as u16) * geom.row_height;
        let row_rect = Rect::new(geom.list_area.x, y, geom.list_area.width, geom.row_height);
        let active = idx == drawer.style_cursor;
        let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
        } else if hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        for ry in row_rect.top()..row_rect.bottom() {
            for col in row_rect.left()..row_rect.right() {
                buf[(col, ry)].set_symbol(" ").set_bg(row_bg);
            }
            if active {
                buf[(row_rect.x, ry)]
                    .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                    .set_style(
                        Style::default()
                            .fg(tokens::animated_selected_accent(theme, elapsed, reduced))
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD),
                    );
            }
        }

        let (label, right_badge) = match *choice {
            StyleSelection::Auto => (
                i.text("style.auto.name").to_owned(),
                format!(
                    "{} {}",
                    if theme.flags.ascii { "->" } else { "→" },
                    i.text(palette_default.name_key())
                ),
            ),
            StyleSelection::Fixed(style) => (
                i.text(style.name_key()).to_owned(),
                style.preview_glyph(theme.flags.ascii).to_owned(),
            ),
        };
        let right_w = right_badge.width().min((row_rect.width / 2) as usize);
        let name_budget = row_rect.width.saturating_sub(4 + right_w as u16) as usize;
        let name_fg = if active {
            tokens::selected_fg(theme)
        } else {
            tokens::color(theme, Role::Text2)
        };
        let mut name_style = Style::default().fg(name_fg).bg(row_bg);
        if active {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            row_rect.x + 2,
            y,
            clip_ascii_safe(&label, name_budget, theme.flags.ascii),
            name_budget,
            name_style,
        );

        let badge_fg = if active {
            tokens::selected_accent(theme)
        } else if matches!(*choice, StyleSelection::Fixed(s) if s == palette_default) {
            tokens::color(theme, Role::Accent2)
        } else {
            tokens::color(theme, Role::Text3)
        };
        tokens::put_right(
            buf,
            row_rect,
            &clip_ascii_safe(&right_badge, right_w, theme.flags.ascii),
            Style::default().fg(badge_fg).bg(row_bg),
        );
    }
}

fn draw_toggle_row(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    key: &str,
    label: &str,
    enabled: bool,
    base_bg: Color,
) {
    if rect.height == 0 || rect.width < 10 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    let bg = if hovered && theme.flags.paint_bg {
        tokens::color(theme, Role::SurfaceAlt)
    } else {
        base_bg
    };
    for ry in rect.top()..rect.bottom() {
        for col in rect.left()..rect.right() {
            buf[(col, ry)].set_symbol(" ").set_bg(bg);
        }
    }

    let state_label = if enabled {
        format!(
            "{} {}",
            tokens::glyph(theme, Glyph::DotOn),
            i.text("ui.style_on")
        )
    } else {
        format!(
            "{} {}",
            tokens::glyph(theme, Glyph::DotOff),
            i.text("ui.style_off")
        )
    };
    let state_color = if enabled {
        tokens::color(theme, Role::Success)
    } else {
        tokens::color(theme, Role::Text3)
    };
    let key_str = format!("[{key}] ");
    buf.set_stringn(
        rect.x,
        rect.y,
        &key_str,
        key_str.width(),
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let label_budget = rect
        .width
        .saturating_sub(key_str.width() as u16 + state_label.width() as u16 + 2)
        as usize;
    buf.set_stringn(
        rect.x + key_str.width() as u16,
        rect.y,
        clip_ascii_safe(label, label_budget, theme.flags.ascii),
        label_budget,
        tokens::text(theme, Level::Secondary).bg(bg),
    );
    tokens::put_right(
        buf,
        rect,
        &state_label,
        Style::default()
            .fg(state_color)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_value_row(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    key: &str,
    label: &str,
    value: &str,
    highlighted: bool,
    base_bg: Color,
) {
    if rect.height == 0 || rect.width < 10 {
        return;
    }
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    let bg = if hovered && theme.flags.paint_bg {
        tokens::color(theme, Role::SurfaceAlt)
    } else {
        base_bg
    };
    for ry in rect.top()..rect.bottom() {
        for col in rect.left()..rect.right() {
            buf[(col, ry)].set_symbol(" ").set_bg(bg);
        }
    }
    let state_label = format!("{} {value}", tokens::glyph(theme, Glyph::DotOn));
    let state_color = if highlighted {
        tokens::color(theme, Role::Accent)
    } else {
        tokens::color(theme, Role::Text3)
    };
    let key_str = format!("[{key}] ");
    buf.set_stringn(
        rect.x,
        rect.y,
        &key_str,
        key_str.width(),
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let label_budget = rect
        .width
        .saturating_sub(key_str.width() as u16 + state_label.width() as u16 + 2)
        as usize;
    buf.set_stringn(
        rect.x + key_str.width() as u16,
        rect.y,
        clip_ascii_safe(label, label_budget, theme.flags.ascii),
        label_budget,
        tokens::text(theme, Level::Secondary).bg(bg),
    );
    tokens::put_right(
        buf,
        Rect::new(rect.x, rect.y, rect.width, 1),
        &state_label,
        Style::default()
            .fg(state_color)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
}
