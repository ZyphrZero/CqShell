//! FlClash-style Proxies & Policy Groups renderer:
//! - Top scrollable horizontal Policy Group pill bar + `[g]` quick-jump picker
//! - Inline search + `[t]` group delay test + `[o]` sort + `[h]` hide timeout + `[P]` providers
//! - Active group type, resolved exit chain, and unfix hint for pinned automatic groups
//! - Responsive multi-column (2-4 cols wide, 1 col narrow) node card grid with colored delays

use super::{delay_role, draw_inline_filter, draw_pill_chip, fill_rect_bg, surface_bg};
use crate::{
    theme::{Level, Role, Theme},
    tui::{
        clash::layout::ProxiesGeom,
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
    style::{Color, Modifier},
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
            let testing = cl
                .current_group()
                .is_some_and(|g| cl.testing_groups.contains(&g.name));
            let lbl = if testing {
                format!(" {} {} ", app.spin(), i.text("clash.key.testing"))
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

    // 3. Active group chain & metadata strip
    draw_chain_bar(app, buf, geom.chain_bar, bg);

    // 4. Multi-column node card grid
    draw_node_grid(app, buf, geom, bg);
}

fn draw_group_bar(app: &App, buf: &mut Buffer, geom: &ProxiesGeom, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let cl = &app.clash;
    let ascii = theme.flags.ascii;

    if geom.group_overflow_left {
        buf.set_stringn(
            geom.group_bar.x,
            geom.group_bar.y,
            if ascii { "<" } else { "‹" },
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    }
    if geom.group_overflow_right {
        buf.set_stringn(
            geom.group_bar.right().saturating_sub(1),
            geom.group_bar.y,
            if ascii { ">" } else { "›" },
            1,
            tokens::text(theme, Level::Tertiary).bg(bg),
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

fn draw_chain_bar(app: &App, buf: &mut Buffer, rect: Rect, bg: Color) {
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
    let mut left = format!(
        "[{}] {} ({}/{}) {sep} {chain}",
        group.kind.label(),
        group.name,
        nodes.len(),
        group.members.len(),
    );
    if group.fixed.is_some() {
        left.push_str(&format!(" {sep} {}", i.text("clash.proxies.pinned")));
    }

    let hint = if group.fixed.is_some() && group.kind.supports_unfix() {
        format!(
            "[x] {}  [f] {}  [d] URL",
            i.text("clash.key.unfix"),
            i.text("clash.key.fastest")
        )
    } else {
        format!(
            "[p] {}  [f] {}  [c] {}  [d] URL",
            i.text("clash.key.ping_node"),
            i.text("clash.key.fastest"),
            i.text("clash.key.locate")
        )
    };

    let w = rect.width as usize;
    let show_hint = w >= 66 && hint.width() + 24 <= w;
    let left_budget = if show_hint {
        w.saturating_sub(hint.width() + 2)
    } else {
        w
    };
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&left, left_budget, ascii),
        left_budget,
        tokens::text(theme, Level::Secondary).bg(bg),
    );
    if show_hint {
        tokens::put_right(
            buf,
            rect,
            &clip_ascii_safe(&hint, w, ascii),
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
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
    let active_bar = tokens::glyph(theme, Glyph::ActiveBar);

    for &(rect, idx) in &geom.node_hits {
        let Some(node) = nodes.get(idx) else {
            continue;
        };
        let is_cursor = idx == cl.node_cursor;
        let is_selected = node.name == group.now;
        let is_hovered = app.hover.is_some_and(|p| rect.contains(p));
        let is_testing =
            cl.testing_nodes.contains(&node.name) || cl.testing_groups.contains(&group.name);

        let card_bg = if is_cursor {
            tokens::color(theme, Role::Selection)
        } else if is_hovered {
            tokens::color(theme, Role::Surface)
        } else {
            tokens::color(theme, Role::SurfaceAlt)
        };
        fill_rect_bg(buf, rect, card_bg);

        // Compact 1-row mobile layout
        if rect.height == 1 {
            let iw = rect.width.saturating_sub(2) as usize;
            let delay_str = format_delay_badge(
                node.delay_ms,
                node.tested,
                is_testing,
                app.spin(),
                i.text("clash.delay.timeout"),
            );
            let d_role = if is_testing {
                Role::Accent
            } else {
                delay_role(node.delay_ms, node.tested)
            };
            let prefix = if is_selected {
                check_glyph
            } else if is_cursor {
                active_bar
            } else {
                " "
            };
            buf.set_stringn(
                rect.x,
                rect.y,
                prefix,
                1,
                tokens::fg(
                    theme,
                    if is_selected {
                        Role::Success
                    } else {
                        Role::Accent
                    },
                )
                .bg(card_bg),
            );
            let name_w = iw.saturating_sub(delay_str.width() + 2);
            buf.set_stringn(
                rect.x + 2,
                rect.y,
                clip_ascii_safe(&node.name, name_w, ascii),
                name_w,
                tokens::text(theme, Level::Primary)
                    .bg(card_bg)
                    .add_modifier(if is_cursor || is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
            tokens::put_right(
                buf,
                Rect::new(rect.x + 1, rect.y, rect.width.saturating_sub(2), 1),
                &delay_str,
                tokens::fg(theme, d_role).bg(card_bg),
            );
            continue;
        }

        // Card border/accent indicator for 2-row or 3-row cards
        let accent_role = if is_cursor {
            Role::BorderFocus
        } else if is_selected {
            Role::Accent
        } else {
            Role::Border
        };

        if rect.height >= 3 && rect.width >= 10 {
            draw_mini_card_frame(
                buf,
                rect,
                accent_role,
                is_cursor || is_selected,
                theme,
                card_bg,
            );
            let ix = rect.x + 1;
            let iw = rect.width.saturating_sub(2) as usize;
            let name_y = rect.y + 1;
            let meta_y = rect.y + 2;

            // Row 1: node name + checkmark if selected
            let mark_w = if is_selected { 2 } else { 0 };
            let name_w = iw.saturating_sub(mark_w);
            buf.set_stringn(
                ix,
                name_y,
                clip_ascii_safe(&node.name, name_w, ascii),
                name_w,
                tokens::text(theme, Level::Primary)
                    .bg(card_bg)
                    .add_modifier(if is_cursor || is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
            if is_selected {
                tokens::put_right(
                    buf,
                    Rect::new(ix, name_y, iw as u16, 1),
                    check_glyph,
                    tokens::fg(theme, Role::Success)
                        .bg(card_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }

            // Row 2: protocol (left) + delay (right)
            let delay_str = format_delay_badge(
                node.delay_ms,
                node.tested,
                is_testing,
                app.spin(),
                i.text("clash.delay.timeout"),
            );
            let d_role = if is_testing {
                Role::Accent
            } else {
                delay_role(node.delay_ms, node.tested)
            };
            let proto_w = iw.saturating_sub(delay_str.width() + 1);
            let proto = if node.is_group {
                match &node.now {
                    Some(now) if proto_w >= 14 => format!("{} > {now}", node.kind),
                    _ => node.kind.clone(),
                }
            } else if node.udp && proto_w >= node.kind.width() + 4 {
                format!("{} UDP", node.kind)
            } else {
                node.kind.clone()
            };
            buf.set_stringn(
                ix,
                meta_y,
                clip_ascii_safe(&proto, proto_w, ascii),
                proto_w,
                tokens::text(theme, Level::Tertiary).bg(card_bg),
            );
            tokens::put_right(
                buf,
                Rect::new(ix, meta_y, iw as u16, 1),
                &delay_str,
                tokens::fg(theme, d_role)
                    .bg(card_bg)
                    .add_modifier(if node.delay_ms.is_some() {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
        } else {
            // 2-row compact card (narrow single-column)
            for dy in 0..rect.height {
                buf.set_stringn(
                    rect.x,
                    rect.y + dy,
                    if is_cursor || is_selected {
                        active_bar
                    } else {
                        " "
                    },
                    1,
                    tokens::fg(theme, accent_role).bg(card_bg),
                );
            }
            let ix = rect.x + 2;
            let iw = rect.width.saturating_sub(3) as usize;
            let mark_w = if is_selected { 2 } else { 0 };
            buf.set_stringn(
                ix,
                rect.y,
                clip_ascii_safe(&node.name, iw.saturating_sub(mark_w), ascii),
                iw.saturating_sub(mark_w),
                tokens::text(theme, Level::Primary)
                    .bg(card_bg)
                    .add_modifier(if is_cursor || is_selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
            if is_selected {
                tokens::put_right(
                    buf,
                    Rect::new(ix, rect.y, iw as u16, 1),
                    check_glyph,
                    tokens::fg(theme, Role::Success)
                        .bg(card_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }
            let delay_str = format_delay_badge(
                node.delay_ms,
                node.tested,
                is_testing,
                app.spin(),
                i.text("clash.delay.timeout"),
            );
            let d_role = if is_testing {
                Role::Accent
            } else {
                delay_role(node.delay_ms, node.tested)
            };
            let proto_w = iw.saturating_sub(delay_str.width() + 1);
            buf.set_stringn(
                ix,
                rect.y + 1,
                clip_ascii_safe(&node.kind, proto_w, ascii),
                proto_w,
                tokens::text(theme, Level::Tertiary).bg(card_bg),
            );
            tokens::put_right(
                buf,
                Rect::new(ix, rect.y + 1, iw as u16, 1),
                &delay_str,
                tokens::fg(theme, d_role).bg(card_bg),
            );
        }
    }
}

fn draw_mini_card_frame(
    buf: &mut Buffer,
    rect: Rect,
    border_role: Role,
    bold: bool,
    theme: &Theme,
    bg: Color,
) {
    let mut style = tokens::fg(theme, border_role).bg(bg);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    let h_ch = if theme.flags.ascii { '-' } else { '─' };
    let v_ch = if theme.flags.ascii { '|' } else { '│' };
    // Draw a subtle top hairline + left accent edge so 3-row cards keep 2 full text rows inside.
    for x in rect.x..rect.right() {
        buf[(x, rect.y)].set_char(h_ch).set_style(style);
    }
    for y in (rect.y + 1)..rect.bottom() {
        buf[(rect.x, y)].set_char(v_ch).set_style(style);
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
