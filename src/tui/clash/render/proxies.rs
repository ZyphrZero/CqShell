//! FlClash-style Proxies & Policy Groups renderer:
//! - Top scrollable horizontal Policy Group pill bar + clickable `‹`/`›` arrows + `[g]` picker
//! - Inline search + `[t]` live-progress group delay test + `[o]` sort + `[h]` hide timeout + `[P]` providers
//! - Active group type, resolved exit chain, scroll position, and clickable quick-action chips
//! - Responsive multi-column separated floating node tiles with unmistakable Active vs Focus vs Hover states

use super::{
    blend_role_bg, delay_role, draw_inline_filter, draw_pill_chip, fill_rect_bg, progress_track_fg,
    surface_bg,
};
use crate::{
    theme::{Level, Role, Theme},
    tui::{
        clash::layout::{ProxiesGeom, proxy_card_delay_hit},
        render::{
            clip_ascii_safe,
            tokens::{self, Glyph},
        },
        state::App,
    },
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &ProxiesGeom) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let bg = surface_bg(theme, Role::Surface);
    let buf = frame.buffer_mut();

    // 1. Policy group pill bar (FlClash style)
    draw_group_bar(app, buf, geom, bg);

    // 2. Toolbar (search + test + sort + hide-timeout + providers)
    if geom.toolbar.height > 0 {
        draw_inline_filter(
            buf,
            geom.filter_bar,
            &cl.filter,
            cl.filtering,
            i.text("clash.filter.proxies"),
            theme,
            bg,
        );
        if let Some(btn) = geom.test_btn {
            let group_name = cl.current_group().map(|g| g.name.as_str());
            let testing = group_name.is_some_and(|n| cl.testing_groups.contains(n));
            let lbl = if testing {
                if let Some((done, total)) =
                    group_name.and_then(|n| cl.testing_group_progress.get(n))
                {
                    format!(" {} {done}/{total} ", app.spin())
                } else {
                    format!(" {} {} ", app.spin(), i.text("clash.key.testing"))
                }
            } else {
                format!(" [t] {} ", i.text("clash.key.test_group"))
            };
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, testing, hov, theme, bg);
        }
        if let Some(chip) = geom.sort_chip {
            let arrow = if cl.proxy_sort_asc {
                if ascii { "^" } else { "▴" }
            } else if ascii {
                "v"
            } else {
                "▾"
            };
            let lbl = format!(" [o] {}{arrow} ", i.text(cl.proxy_sort.label_key()));
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, false, hov, theme, bg);
        }
        if let Some(chip) = geom.hide_chip {
            let lbl = format!(
                " [h] {} ",
                i.text(if cl.hide_timeout {
                    "clash.filter.hide_on"
                } else {
                    "clash.filter.hide_off"
                })
            );
            let hov = app.hover.is_some_and(|p| chip.contains(p));
            draw_pill_chip(buf, chip, &lbl, cl.hide_timeout, hov, theme, bg);
        }
        if let Some(btn) = geom.providers_btn {
            let lbl = format!(
                " [P] {} ({}) ",
                i.text("clash.key.providers"),
                cl.snapshot.proxy_providers.len()
            );
            let hov = app.hover.is_some_and(|p| btn.contains(p));
            draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
        }
    }

    // 3. Active group chain, scroll range, and clickable quick actions
    draw_chain_bar(app, buf, geom, bg);

    // 4. Multi-column separated floating node tile grid + subtle scrollbar
    draw_node_grid(app, buf, geom, bg);
}

fn draw_group_bar(app: &App, buf: &mut Buffer, geom: &ProxiesGeom, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;

    if geom.group_overflow_left {
        let hov = geom
            .group_prev_hit
            .is_some_and(|r| app.hover.is_some_and(|p| r.contains(p)));
        let st = if hov {
            tokens::fg(theme, Role::Accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            tokens::text(theme, Level::Tertiary).bg(bg)
        };
        buf.set_stringn(
            geom.group_bar.x,
            geom.group_bar.y,
            if ascii { "<" } else { "‹" },
            1,
            st,
        );
    }
    if geom.group_overflow_right {
        let hov = geom
            .group_next_hit
            .is_some_and(|r| app.hover.is_some_and(|p| r.contains(p)));
        let st = if hov {
            tokens::fg(theme, Role::Accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            tokens::text(theme, Level::Tertiary).bg(bg)
        };
        buf.set_stringn(
            geom.group_bar.right().saturating_sub(1),
            geom.group_bar.y,
            if ascii { ">" } else { "›" },
            1,
            st,
        );
    }

    if geom.group_pills.is_empty() {
        buf.set_stringn(
            geom.group_bar.x,
            geom.group_bar.y,
            clip_ascii_safe(
                i.text("clash.proxies.empty"),
                geom.group_bar.width as usize,
                ascii,
            ),
            geom.group_bar.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    } else {
        for pill in &geom.group_pills {
            let active = pill.group_idx == cl.group_cursor;
            let hovered = app.hover.is_some_and(|p| pill.rect.contains(p));
            let inner = format!(
                " {} ",
                clip_ascii_safe(
                    &pill.label,
                    pill.rect.width.saturating_sub(2) as usize,
                    ascii
                )
            );
            draw_pill_chip(buf, pill.rect, &inner, active, hovered, theme, bg);
        }
    }

    if let Some(btn) = geom.group_picker_btn {
        let lbl = format!(" [g] {} ", i.text("clash.key.groups"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_pill_chip(buf, btn, &lbl, false, hov, theme, bg);
    }
}

fn draw_chain_action(
    buf: &mut Buffer,
    rect: Rect,
    label: &str,
    accent: bool,
    hovered: bool,
    theme: &Theme,
    bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let style = if hovered {
        tokens::text(theme, Level::Primary)
            .bg(tokens::color(theme, Role::Selection))
            .add_modifier(Modifier::BOLD)
    } else if accent {
        tokens::fg(theme, Role::Warning)
            .bg(surface_bg(theme, Role::SurfaceAlt))
            .add_modifier(Modifier::BOLD)
    } else {
        tokens::text(theme, Level::Tertiary).bg(bg)
    };
    let fill = style.bg.unwrap_or(bg);
    if hovered || accent {
        fill_rect_bg(buf, rect, fill);
    }
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(label, rect.width as usize, theme.flags.ascii),
        rect.width as usize,
        style,
    );
}

fn draw_chain_bar(app: &App, buf: &mut Buffer, geom: &ProxiesGeom, bg: Color) {
    let rect = geom.chain_bar;
    if rect.width < 10 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let Some(group) = cl.current_group() else {
        return;
    };

    let arrow = if ascii { " -> " } else { " → " };
    let sep = tokens::glyph(theme, Glyph::Separator);
    let chain = if group.resolved_chain.is_empty() {
        "-".to_owned()
    } else {
        group.resolved_chain.join(arrow)
    };
    let nodes = cl.filtered_nodes();
    let range_part = if geom.grid_total_rows > geom.grid_visible_rows
        && geom.visible_node_range.0 > 0
        && geom.visible_node_range.1 >= geom.visible_node_range.0
    {
        format!(
            " ({}-{}/{})",
            geom.visible_node_range.0,
            geom.visible_node_range.1,
            nodes.len()
        )
    } else if nodes.len() == group.members.len() {
        format!(" ({})", nodes.len())
    } else {
        format!(" ({}/{})", nodes.len(), group.members.len())
    };

    let mut left = format!(
        "[{}] {}{range_part} {sep} {chain}",
        group.kind.label(),
        group.name
    );
    if group.fixed.is_some() {
        left.push_str(&format!(" {sep} {}", i.text("clash.proxies.pinned")));
    }

    let leftmost_btn_x = [
        geom.unfix_btn,
        geom.ping_btn,
        geom.fastest_btn,
        geom.locate_btn,
        geom.delay_url_btn,
    ]
    .into_iter()
    .flatten()
    .map(|r| r.x)
    .min();
    let left_budget = leftmost_btn_x
        .map(|bx| bx.saturating_sub(rect.x + 1) as usize)
        .unwrap_or(rect.width as usize);

    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&left, left_budget, ascii),
        left_budget,
        tokens::text(theme, Level::Secondary).bg(bg),
    );

    if let Some(btn) = geom.unfix_btn {
        let lbl = format!(" [x] {} ", i.text("clash.key.unfix"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_chain_action(buf, btn, &lbl, true, hov, theme, bg);
    }
    if let Some(btn) = geom.ping_btn {
        let lbl = format!(" [p] {} ", i.text("clash.key.ping_node"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_chain_action(buf, btn, &lbl, false, hov, theme, bg);
    }
    if let Some(btn) = geom.fastest_btn {
        let lbl = format!(" [f] {} ", i.text("clash.key.fastest"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_chain_action(buf, btn, &lbl, false, hov, theme, bg);
    }
    if let Some(btn) = geom.locate_btn {
        let lbl = format!(" [c] {} ", i.text("clash.key.locate"));
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_chain_action(buf, btn, &lbl, false, hov, theme, bg);
    }
    if let Some(btn) = geom.delay_url_btn {
        let hov = app.hover.is_some_and(|p| btn.contains(p));
        draw_chain_action(buf, btn, " [d] URL ", false, hov, theme, bg);
    }
}

fn proxy_card_bg(theme: &Theme, is_selected: bool, is_cursor: bool, is_hovered: bool) -> Color {
    if theme.flags.solid_selection {
        return if is_selected {
            surface_bg(theme, Role::Selection)
        } else {
            surface_bg(theme, Role::SurfaceAlt)
        };
    }
    if is_selected && is_cursor {
        blend_role_bg(
            theme,
            Role::Selection,
            Role::Accent,
            if theme.light { 0.12 } else { 0.16 },
            Role::Selection,
        )
    } else if is_selected {
        surface_bg(theme, Role::Selection)
    } else if is_cursor {
        blend_role_bg(
            theme,
            Role::SurfaceAlt,
            Role::Accent2,
            if theme.light { 0.13 } else { 0.17 },
            Role::Selection,
        )
    } else if is_hovered {
        blend_role_bg(
            theme,
            Role::SurfaceAlt,
            Role::Selection,
            if theme.light { 0.55 } else { 0.45 },
            Role::SurfaceAlt,
        )
    } else {
        surface_bg(theme, Role::SurfaceAlt)
    }
}

fn draw_node_grid(app: &App, buf: &mut Buffer, geom: &ProxiesGeom, bg: Color) {
    let grid = geom.grid_area;
    if grid.width < 12 || grid.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;
    let Some(group) = cl.current_group() else {
        buf.set_stringn(
            grid.x,
            grid.y,
            clip_ascii_safe(i.text("clash.proxies.empty"), grid.width as usize, ascii),
            grid.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    };
    let nodes = cl.filtered_nodes();
    if nodes.is_empty() {
        buf.set_stringn(
            grid.x,
            grid.y,
            clip_ascii_safe(i.text("clash.proxies.no_match"), grid.width as usize, ascii),
            grid.width as usize,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
        return;
    }

    let check_glyph = tokens::glyph(theme, Glyph::Success);
    let sep = tokens::glyph(theme, Glyph::Separator);

    for &(rect, idx) in &geom.node_hits {
        let Some(node) = nodes.get(idx) else {
            continue;
        };
        let is_cursor = idx == cl.node_cursor;
        let is_selected = node.name == group.now;
        let is_pinned =
            group.kind.supports_unfix() && group.fixed.as_deref() == Some(node.name.as_str());
        let is_hovered = app.hover.is_some_and(|p| rect.contains(p));
        let delay_hit = proxy_card_delay_hit(rect);
        let delay_hovered = delay_hit.is_some_and(|r| app.hover.is_some_and(|p| r.contains(p)));
        let is_testing =
            cl.testing_nodes.contains(&node.name) || cl.testing_groups.contains(&group.name);

        let card_bg = proxy_card_bg(theme, is_selected, is_cursor, is_hovered);
        let solid_sel_card = theme.flags.solid_selection && is_selected;
        fill_rect_bg(buf, rect, card_bg);

        let delay_str = format_delay_badge(
            node.delay_ms,
            node.tested,
            is_testing,
            app.spin(),
            i.text("clash.delay.timeout"),
        );
        let d_role = if solid_sel_card {
            Role::OnAccent
        } else if is_testing {
            Role::Accent
        } else {
            delay_role(node.delay_ms, node.tested)
        };

        // Compact 1-row mobile layout
        if rect.height == 1 {
            let iw = rect.width.saturating_sub(2) as usize;
            let (prefix, p_role) = if is_cursor {
                (
                    if ascii { ">" } else { "▸" },
                    if solid_sel_card {
                        Role::OnAccent
                    } else {
                        Role::BorderFocus
                    },
                )
            } else if is_selected {
                (
                    check_glyph,
                    if solid_sel_card {
                        Role::OnAccent
                    } else {
                        Role::Success
                    },
                )
            } else {
                (" ", Role::Border)
            };
            buf.set_stringn(
                rect.x,
                rect.y,
                prefix,
                1,
                tokens::fg(theme, p_role)
                    .bg(card_bg)
                    .add_modifier(if is_cursor || is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
            let badge = if is_selected && is_cursor {
                format!("{check_glyph} ")
            } else {
                String::new()
            };
            let right_w = delay_str.width() + badge.width() + 2;
            let name_w = iw.saturating_sub(right_w);
            let name_st = if solid_sel_card {
                Style::default()
                    .fg(tokens::selected_fg(theme))
                    .bg(card_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                tokens::text(theme, Level::Primary)
                    .bg(card_bg)
                    .add_modifier(if is_cursor || is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
            };
            buf.set_stringn(
                rect.x + 2,
                rect.y,
                clip_ascii_safe(&node.name, name_w, ascii),
                name_w,
                name_st,
            );
            let right_label = format!("{badge}{delay_str}");
            let d_bg = if delay_hovered && !solid_sel_card {
                tokens::color(theme, Role::Selection)
            } else {
                card_bg
            };
            tokens::put_right(
                buf,
                Rect::new(rect.x + 1, rect.y, rect.width.saturating_sub(2), 1),
                &right_label,
                tokens::fg(theme, d_role).bg(d_bg).add_modifier(
                    if delay_hovered || node.delay_ms.is_some() {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    },
                ),
            );
            continue;
        }

        // 2-row separated floating tile (multi-column & standard single-column)
        let (rail_top, rail_bot, rail_role, rail_bold) = if is_cursor && is_selected {
            (
                if ascii { ">" } else { "▸" },
                if ascii { "*" } else { "┃" },
                if solid_sel_card {
                    Role::OnAccent
                } else {
                    Role::BorderFocus
                },
                true,
            )
        } else if is_selected {
            (
                if ascii { "*" } else { "┃" },
                if ascii { "*" } else { "┃" },
                if solid_sel_card {
                    Role::OnAccent
                } else {
                    Role::Success
                },
                true,
            )
        } else if is_cursor {
            (
                if ascii { ">" } else { "▸" },
                if ascii { "|" } else { "│" },
                Role::BorderFocus,
                true,
            )
        } else if is_hovered {
            (
                if ascii { "|" } else { "│" },
                if ascii { "|" } else { "│" },
                Role::Accent,
                false,
            )
        } else {
            (
                if ascii { "|" } else { "│" },
                if ascii { "|" } else { "│" },
                Role::Border,
                false,
            )
        };
        let mut rail_style = tokens::fg(theme, rail_role).bg(card_bg);
        if rail_bold {
            rail_style = rail_style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(rect.x, rect.y, rail_top, 1, rail_style);
        buf.set_stringn(rect.x, rect.y + 1, rail_bot, 1, rail_style);

        let ix = rect.x + 2;
        let iw = rect.width.saturating_sub(3) as usize;

        // Row 0: Node name + Active/Pinned badge on the right
        let status_badge = if is_pinned {
            let pin_txt = i.text("clash.proxies.pin_badge");
            if iw >= 18 {
                format!("{check_glyph} {pin_txt}")
            } else {
                check_glyph.to_owned()
            }
        } else if is_selected {
            check_glyph.to_owned()
        } else {
            String::new()
        };
        let badge_w = if status_badge.is_empty() {
            0
        } else {
            status_badge.width() + 1
        };
        let name_w = iw.saturating_sub(badge_w);
        let name_style = if solid_sel_card {
            Style::default()
                .fg(tokens::selected_fg(theme))
                .bg(card_bg)
                .add_modifier(Modifier::BOLD)
        } else if is_selected && !is_cursor {
            tokens::fg(theme, Role::Accent)
                .bg(card_bg)
                .add_modifier(Modifier::BOLD)
        } else if is_cursor || is_selected {
            tokens::text(theme, Level::Primary)
                .bg(card_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            tokens::text(theme, Level::Primary).bg(card_bg)
        };
        buf.set_stringn(
            ix,
            rect.y,
            clip_ascii_safe(&node.name, name_w, ascii),
            name_w,
            name_style,
        );
        if !status_badge.is_empty() {
            let badge_role = if solid_sel_card {
                Role::OnAccent
            } else if is_pinned {
                Role::Warning
            } else {
                Role::Success
            };
            tokens::put_right(
                buf,
                Rect::new(ix, rect.y, iw as u16, 1),
                &status_badge,
                tokens::fg(theme, badge_role)
                    .bg(card_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }

        // Row 1: Protocol / nested group resolved exit (left) + clickable delay badge (right)
        let proto_w = iw.saturating_sub(delay_str.width() + 1);
        let arrow_ch = if ascii { ">" } else { "›" };
        let proto = if node.is_group {
            match &node.now {
                Some(now) if proto_w >= 12 => format!("{} {arrow_ch} {now}", node.kind),
                _ => node.kind.clone(),
            }
        } else if node.udp && proto_w >= node.kind.width() + 5 {
            format!("{} {sep} UDP", node.kind)
        } else {
            node.kind.clone()
        };
        let proto_style = if solid_sel_card {
            Style::default().fg(tokens::selected_fg(theme)).bg(card_bg)
        } else if is_selected || is_cursor {
            tokens::text(theme, Level::Secondary).bg(card_bg)
        } else {
            tokens::text(theme, Level::Tertiary).bg(card_bg)
        };
        buf.set_stringn(
            ix,
            rect.y + 1,
            clip_ascii_safe(&proto, proto_w, ascii),
            proto_w,
            proto_style,
        );

        let delay_bg = if delay_hovered && !solid_sel_card {
            blend_role_bg(
                theme,
                Role::Selection,
                Role::Accent,
                if theme.light { 0.14 } else { 0.20 },
                Role::Selection,
            )
        } else {
            card_bg
        };
        if delay_hovered
            && !solid_sel_card
            && let Some(dh) = delay_hit
        {
            fill_rect_bg(buf, dh, delay_bg);
        }
        tokens::put_right(
            buf,
            Rect::new(ix, rect.y + 1, iw as u16, 1),
            &delay_str,
            tokens::fg(theme, d_role).bg(delay_bg).add_modifier(
                if delay_hovered || node.delay_ms.is_some() {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            ),
        );
    }

    // Subtle right-edge scrollbar when node grid exceeds visible viewport
    if let Some(sb) = geom.grid_scrollbar
        && geom.grid_total_rows > geom.grid_visible_rows
        && sb.height >= 2
    {
        draw_grid_scrollbar(
            buf,
            sb,
            geom.grid_scroll_rows,
            geom.grid_visible_rows,
            geom.grid_total_rows,
            theme,
            bg,
        );
    }
}

fn draw_grid_scrollbar(
    buf: &mut Buffer,
    rect: Rect,
    scroll_rows: usize,
    visible_rows: usize,
    total_rows: usize,
    theme: &Theme,
    bg: Color,
) {
    let h = rect.height as usize;
    let thumb_h = ((h * visible_rows) / total_rows.max(1)).clamp(1, h);
    let max_scroll = total_rows.saturating_sub(visible_rows).max(1);
    let max_top = h.saturating_sub(thumb_h);
    let thumb_top = (scroll_rows.min(max_scroll) * max_top) / max_scroll;
    let (track_ch, thumb_ch) = if theme.flags.ascii {
        ('|', '#')
    } else {
        ('│', '┃')
    };
    let track_style = Style::default().fg(progress_track_fg(theme, bg)).bg(bg);
    let thumb_style = tokens::fg(theme, Role::Accent).bg(bg);
    for row in 0..h {
        let cell = &mut buf[(rect.x, rect.y + row as u16)];
        if row >= thumb_top && row < thumb_top + thumb_h {
            cell.set_char(thumb_ch).set_style(thumb_style);
        } else {
            cell.set_char(track_ch).set_style(track_style);
        }
    }
}

fn format_delay_badge(
    delay_ms: Option<u64>,
    tested: bool,
    testing: bool,
    spin: &str,
    timeout_lbl: &str,
) -> String {
    if testing {
        format!("{spin} ...")
    } else if let Some(ms) = delay_ms {
        format!("{ms} ms")
    } else if tested {
        timeout_lbl.to_owned()
    } else {
        "--".to_owned()
    }
}
