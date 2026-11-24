//! Module Action List, One-Click Optimization Checklist, and Right Inspector / Execution Plan /
//! Live Step Progress drawer.

use super::{
    clip_ascii_safe,
    tokens::{self, Glyph},
    wrap_words,
};
use crate::{
    actions::Spec,
    runtime::Outcome,
    theme::{Level, Role},
    tui::{
        fx,
        layout::WorkbenchGeometry,
        preview, related,
        selection::{self, SelectablePane, SelectableRow, TextPane},
        state::{App, GROUPS, OPTIMIZATION_GROUP, Row, StepView, Tone, format_mm_ss, row_index},
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

pub(super) fn draw(app: &App, frame: &mut Frame, geom: &WorkbenchGeometry) {
    let Some(group) = app.active_group() else {
        return;
    };
    draw_action_list(app, frame, geom, group);
    if geom.inspector_area.width >= 10 && geom.inspector_area.height >= 4 {
        draw_inspector(app, frame, geom, group);
    }
}

fn draw_action_list(app: &App, frame: &mut Frame, geom: &WorkbenchGeometry, group: usize) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    let area = geom.list_area;
    tokens::draw_surface_card_fx(
        frame,
        area,
        theme,
        Role::Surface,
        Role::Border,
        false,
        elapsed,
        reduced,
    );
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        Color::Reset
    };
    let title_bg = tokens::card_title_bg(theme, Role::Surface);

    let rows = app.rows(group);
    let selectable_total = rows.iter().filter(|r| r.selectable()).count();
    let selected_pos = app.selected_in(group);
    let selected_visual = if selectable_total > 0 {
        row_index(&rows, selected_pos)
    } else {
        0
    };

    // Card title row (shows active section breadcrumb when filtered to a single section)
    let title = if group == OPTIMIZATION_GROUP {
        i.format(
            "ui.optimization_title",
            &[("count", &app.marked.len().to_string())],
        )
    } else {
        let base = i.text(&format!("group.{}", GROUPS[group]));
        if let Some(sec_idx) = app.section_tab(group)
            && let Some(secs) = app.group_sections(group)
            && let Some((sec_id, _)) = secs.get(sec_idx)
        {
            let sec_label = i.text(&format!("section.{}.{sec_id}", GROUPS[group]));
            let sep = if theme.flags.ascii { "/" } else { "·" };
            format!("{base} {sep} {sec_label}")
        } else {
            base.to_owned()
        }
    };
    let count_badge = if selectable_total > 0 {
        format!("{}/{}", selected_pos + 1, selectable_total)
    } else {
        String::new()
    };

    let buf = frame.buffer_mut();
    let title_y = area.y;
    let formatted_title = tokens::format_card_title(theme, &title, theme.flags.framed);
    buf.set_stringn(
        area.x + 2,
        title_y,
        clip_ascii_safe(
            &formatted_title,
            area.width.saturating_sub(14) as usize,
            theme.flags.ascii,
        ),
        area.width.saturating_sub(14) as usize,
        Style::default()
            .fg(tokens::card_title_fg(theme, false))
            .bg(title_bg)
            .add_modifier(Modifier::BOLD),
    );
    if !count_badge.is_empty() {
        tokens::put_right(
            buf,
            Rect::new(area.x + 2, title_y, area.width.saturating_sub(4), 1),
            &if theme.flags.framed {
                format!(" {count_badge} ")
            } else {
                count_badge
            },
            tokens::text(theme, Level::Tertiary).bg(title_bg),
        );
    }

    // Section sub-tab pill bar (for grouped modules such as Mihomo, System, Network, Docker, Tools)
    if !geom.section_pills.is_empty() {
        let show_counts = geom.section_bar.width >= 40;
        let active_tab = app.section_tab(group);
        if geom.section_overflow_left {
            let sym = if theme.flags.ascii { "<" } else { "‹" };
            buf.set_stringn(
                geom.section_bar.x,
                geom.section_bar.y,
                sym,
                1,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
        if geom.section_overflow_right {
            let sym = if theme.flags.ascii { ">" } else { "›" };
            buf.set_stringn(
                geom.section_bar.right().saturating_sub(1),
                geom.section_bar.y,
                sym,
                1,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
        for pill in &geom.section_pills {
            let is_active = pill.tab == active_tab;
            let is_hovered = app.hover.is_some_and(|p| pill.rect.contains(p));
            let text_body = if show_counts {
                format!("{} {}", pill.label, pill.count)
            } else {
                pill.label.clone()
            };
            if theme.flags.ascii {
                let text = if is_active {
                    format!("[{text_body}]")
                } else {
                    format!(" {text_body} ")
                };
                let style = if is_active {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    tokens::text(theme, Level::Secondary)
                };
                buf.set_stringn(
                    pill.rect.x,
                    pill.rect.y,
                    clip_ascii_safe(&text, pill.rect.width as usize, true),
                    pill.rect.width as usize,
                    style,
                );
            } else if is_active {
                let pill_bg = tokens::color(theme, Role::Accent);
                let pill_fg = tokens::color(theme, Role::OnAccent);
                for col in pill.rect.left()..pill.rect.right() {
                    buf[(col, pill.rect.y)].set_symbol(" ").set_bg(pill_bg);
                }
                let text = format!(" {text_body} ");
                buf.set_stringn(
                    pill.rect.x,
                    pill.rect.y,
                    clip_ascii_safe(&text, pill.rect.width as usize, false),
                    pill.rect.width as usize,
                    Style::default()
                        .fg(pill_fg)
                        .bg(pill_bg)
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                let pill_bg = if is_hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::Elevated)
                } else if theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                };
                for col in pill.rect.left()..pill.rect.right() {
                    buf[(col, pill.rect.y)].set_symbol(" ").set_bg(pill_bg);
                }
                let label_fg = if is_hovered {
                    tokens::color(theme, Role::Text1)
                } else {
                    tokens::color(theme, Role::Text2)
                };
                let label_part = format!(" {}", pill.label);
                let label_clipped = clip_ascii_safe(
                    &label_part,
                    pill.rect.width.saturating_sub(1) as usize,
                    false,
                );
                buf.set_stringn(
                    pill.rect.x,
                    pill.rect.y,
                    &label_clipped,
                    pill.rect.width as usize,
                    Style::default().fg(label_fg).bg(pill_bg),
                );
                if show_counts {
                    let count_x = pill.rect.x + label_clipped.width() as u16;
                    let remain = pill.rect.right().saturating_sub(count_x) as usize;
                    if remain >= 2 {
                        buf.set_stringn(
                            count_x,
                            pill.rect.y,
                            format!(" {} ", pill.count),
                            remain,
                            Style::default()
                                .fg(tokens::color(theme, Role::Text3))
                                .bg(pill_bg),
                        );
                    }
                }
            }
        }
    }
    if let Some(rule_y) = geom.list_rule_y {
        tokens::draw_rule(
            buf,
            area.x + 2,
            rule_y,
            area.width.saturating_sub(4),
            theme,
            bg,
        );
    }

    let inner = geom.list_inner;
    if inner.height == 0 || inner.width < 8 {
        return;
    }

    let grouped_all = rows.iter().any(|r| matches!(r, Row::Section(..)));
    let scroll = geom.list_scroll;
    let visible_h = inner.height as usize;

    for (offset, row) in rows.iter().skip(scroll).take(visible_h).enumerate() {
        let visual_idx = scroll + offset;
        let y = inner.y + offset as u16;
        let row_rect = Rect::new(inner.x, y, inner.width, 1);

        match row {
            Row::Spacer => {}
            Row::Section(sec_id, count) => {
                let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
                let sec_bg = if hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                };
                if hovered && theme.flags.paint_bg {
                    for col in row_rect.left()..row_rect.right() {
                        buf[(col, y)].set_symbol(" ").set_bg(sec_bg);
                    }
                }
                let key = format!("section.{}.{sec_id}", GROUPS[group]);
                let label = i.text(&key);
                let icon = tokens::glyph(theme, Glyph::Execute);
                let heading = format!("{icon} {label}");
                let max_head = row_rect.width.saturating_sub(8) as usize;
                let clipped_head = clip_ascii_safe(&heading, max_head, theme.flags.ascii);
                let head_style = if theme.flags.ascii {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    tokens::fg(theme, Role::Accent)
                        .bg(sec_bg)
                        .add_modifier(Modifier::BOLD)
                };
                buf.set_stringn(row_rect.x + 1, y, &clipped_head, max_head, head_style);

                let count_x = row_rect.x + 1 + clipped_head.width() as u16;
                let count_str = format!(" ({count}) ");
                let count_avail = row_rect.right().saturating_sub(count_x) as usize;
                if count_avail >= count_str.len() {
                    buf.set_stringn(
                        count_x,
                        y,
                        &count_str,
                        count_avail,
                        tokens::text(theme, Level::Tertiary).bg(sec_bg),
                    );
                    let rule_x = count_x + count_str.width() as u16;
                    let rule_w = row_rect.right().saturating_sub(rule_x + 1);
                    if rule_w >= 2 {
                        tokens::draw_rule(buf, rule_x, y, rule_w, theme, sec_bg);
                    }
                }
            }
            Row::Rule => {
                tokens::draw_rule(
                    buf,
                    row_rect.x + 2,
                    y,
                    row_rect.width.saturating_sub(4),
                    theme,
                    bg,
                );
            }
            Row::Execute => {
                let active = visual_idx == selected_visual;
                let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
                let row_bg = if active {
                    tokens::color(theme, Role::Selection)
                } else if hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                };
                for col in row_rect.left()..row_rect.right() {
                    buf[(col, y)].set_symbol(" ").set_bg(row_bg);
                }
                if active {
                    buf[(row_rect.x, y)]
                        .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                        .set_style(
                            Style::default()
                                .fg(tokens::selected_accent(theme))
                                .bg(row_bg)
                                .add_modifier(Modifier::BOLD),
                        );
                }
                let text = format!(
                    "{} {}",
                    tokens::glyph(theme, Glyph::Execute),
                    i.format(
                        "ui.execute_selected",
                        &[("count", &app.marked.len().to_string())]
                    )
                );
                let fg = if active {
                    tokens::selected_accent(theme)
                } else if app.marked.is_empty() {
                    tokens::color(theme, Role::Text3)
                } else {
                    tokens::color(theme, Role::Accent)
                };
                buf.set_stringn(
                    row_rect.x + 2,
                    y,
                    clip_ascii_safe(
                        &text,
                        row_rect.width.saturating_sub(3) as usize,
                        theme.flags.ascii,
                    ),
                    row_rect.width.saturating_sub(3) as usize,
                    Style::default()
                        .fg(fg)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }
            Row::Action(spec) => {
                let active = visual_idx == selected_visual;
                let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
                let available = app.available(spec).is_ok();
                let row_bg = if active {
                    tokens::color(theme, Role::Selection)
                } else if hovered && theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    bg
                };
                // Indent child items under section headers in All view with a clear vertical guide.
                let indent = if grouped_all { 2u16 } else { 0u16 };
                let bar_x = row_rect.x + indent;
                for col in bar_x..row_rect.right() {
                    buf[(col, y)].set_symbol(" ").set_bg(row_bg);
                }
                if active {
                    buf[(bar_x, y)]
                        .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                        .set_style(
                            Style::default()
                                .fg(tokens::animated_selected_accent(theme, elapsed, reduced))
                                .bg(row_bg)
                                .add_modifier(Modifier::BOLD),
                        );
                } else if grouped_all {
                    let rail = if theme.flags.ascii { "|" } else { "│" };
                    buf[(bar_x, y)]
                        .set_symbol(rail)
                        .set_style(tokens::fg(theme, Role::Rule).bg(row_bg));
                }

                // Right-aligned live preview tag or optimization parameter summary
                let (tag_text, tag_tone) = row_tag(app, spec, group == OPTIMIZATION_GROUP);
                let max_tag = if row_rect.width < 48 {
                    (row_rect.width / 3) as usize
                } else {
                    (row_rect.width / 2) as usize
                };
                let tag_w = if tag_text.is_empty() {
                    0
                } else {
                    tag_text.width().min(max_tag) + 2
                };

                let mut prefix = String::new();
                let mut kind_glyph: Option<(&'static str, Color)> = None;
                if group == OPTIMIZATION_GROUP {
                    let checked = app.marked.contains(spec.id);
                    if theme.flags.ascii {
                        prefix.push_str(if checked { "[x] " } else { "[ ] " });
                    } else {
                        prefix.push_str(if checked { "[✓] " } else { "[ ] " });
                    }
                } else {
                    let (sym, role) = if !spec.changes {
                        (if theme.flags.ascii { "o" } else { "◦" }, Role::Info)
                    } else if !spec.fields.is_empty() {
                        (if theme.flags.ascii { "+" } else { "▸" }, Role::Accent2)
                    } else {
                        (if theme.flags.ascii { "*" } else { "•" }, Role::Warning)
                    };
                    let fg = if active && theme.flags.solid_selection {
                        tokens::selected_fg(theme)
                    } else if active {
                        tokens::selected_accent(theme)
                    } else if !available {
                        tokens::color(theme, Role::Text3)
                    } else {
                        tokens::color(theme, role)
                    };
                    kind_glyph = Some((sym, fg));
                }
                let mut label = i.text(&spec.label()).to_owned();
                if !spec.fields.is_empty() {
                    label.push_str(if theme.flags.ascii { "..." } else { "…" });
                }
                if spec.external {
                    label.push(' ');
                    label.push_str(tokens::glyph(theme, Glyph::External));
                }

                let text_x = bar_x + 2;
                let (label_start_x, glyph_pad) = if let Some((sym, sym_fg)) = kind_glyph {
                    buf.set_stringn(text_x, y, sym, 1, Style::default().fg(sym_fg).bg(row_bg));
                    (text_x + 2, 2u16)
                } else {
                    (text_x, 0u16)
                };

                let active_cue = if active && tag_text.is_empty() {
                    Some(tokens::glyph(theme, Glyph::Enter))
                } else {
                    None
                };
                let right_reserve = if !tag_text.is_empty() {
                    tag_w as u16
                } else if let Some(cue) = active_cue {
                    cue.width() as u16 + 2
                } else {
                    0
                };
                let label_budget = row_rect
                    .right()
                    .saturating_sub(text_x + glyph_pad)
                    .saturating_sub(1 + right_reserve) as usize;
                let full_label = format!("{prefix}{label}");
                let clipped = clip_ascii_safe(&full_label, label_budget, theme.flags.ascii);

                let text_fg = if active {
                    tokens::selected_fg(theme)
                } else if !available {
                    tokens::color(theme, Role::Text3)
                } else if hovered || (group == OPTIMIZATION_GROUP && app.marked.contains(spec.id)) {
                    tokens::color(theme, Role::Text1)
                } else {
                    tokens::color(theme, Role::Text2)
                };
                let mut style = Style::default().fg(text_fg).bg(row_bg);
                if active {
                    style = style.add_modifier(Modifier::BOLD);
                }
                buf.set_stringn(label_start_x, y, &clipped, label_budget, style);

                if !tag_text.is_empty() {
                    let clipped_tag =
                        clip_ascii_safe(&tag_text, tag_w.saturating_sub(1), theme.flags.ascii);
                    let tag_fg = if active && theme.flags.solid_selection {
                        tokens::selected_fg(theme)
                    } else {
                        tokens::color(theme, tokens::tone_role(tag_tone))
                    };
                    tokens::put_right(
                        buf,
                        Rect::new(row_rect.x, y, row_rect.width.saturating_sub(1), 1),
                        &clipped_tag,
                        Style::default().fg(tag_fg).bg(row_bg),
                    );
                } else if let Some(cue) = active_cue {
                    tokens::put_right(
                        buf,
                        Rect::new(row_rect.x, y, row_rect.width.saturating_sub(1), 1),
                        cue,
                        Style::default()
                            .fg(tokens::selected_accent(theme))
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }

    // Subtle scroll indicator on right edge if rows overflow
    if rows.len() > visible_h && visible_h >= 3 {
        let thumb_h = ((visible_h * visible_h) / rows.len()).max(1);
        let max_scroll = rows.len() - visible_h;
        let thumb_pos = (scroll * (visible_h - thumb_h))
            .checked_div(max_scroll)
            .unwrap_or(0);
        let track_x = area.right().saturating_sub(1);
        for i in 0..visible_h {
            let y = inner.y + i as u16;
            let in_thumb = i >= thumb_pos && i < thumb_pos + thumb_h;
            let sym = if theme.flags.ascii {
                if in_thumb { "#" } else { "|" }
            } else if in_thumb {
                "┃"
            } else {
                "│"
            };
            buf[(track_x, y)].set_symbol(sym).set_style(
                Style::default()
                    .fg(tokens::color(
                        theme,
                        if in_thumb { Role::Accent } else { Role::Rule },
                    ))
                    .bg(bg),
            );
        }
    }
}

fn row_tag(app: &App, spec: &Spec, optimization: bool) -> (String, Tone) {
    let i = &app.ctx.i18n;
    if optimization {
        let params = app.saved_parameters(spec);
        let summary = related::summary(spec, &params);
        let missing_req = spec.fields.iter().any(|f| {
            f.required
                && params
                    .get(f.name)
                    .map_or(f.default, String::as_str)
                    .is_empty()
        });
        if missing_req {
            return (
                format!(
                    "{} {}",
                    tokens::glyph(&app.theme.active, Glyph::Warning),
                    i.text("ui.not_set")
                ),
                Tone::Warning,
            );
        }
        if !summary.is_empty() {
            let matches_curr = app
                .related_for(spec)
                .is_some_and(|rel| (rel.matches_current)(&params, &app.snapshot));
            if matches_curr {
                return (
                    format!("{summary} {}", i.text("ui.current_equal")),
                    Tone::Success,
                );
            }
            return (summary, Tone::Muted);
        }
        return (String::new(), Tone::Muted);
    }

    if let Some(rel) = app.related_for(spec)
        && !rel.tag.is_empty()
    {
        let sep = format!(" {} ", tokens::glyph(&app.theme.active, Glyph::Separator));
        let joined = rel
            .tag
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(&sep);
        let tone = if joined.contains('●') {
            Tone::Success
        } else {
            Tone::Muted
        };
        return (joined, tone);
    }
    if spec.root && !app.root {
        return ("root".into(), Tone::Warning);
    }
    (String::new(), Tone::Muted)
}

fn draw_inspector(app: &App, frame: &mut Frame, geom: &WorkbenchGeometry, group: usize) {
    app.last_cmd_rect.set(None);
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let area = geom.inspector_area;
    tokens::draw_surface_card(frame, area, theme, Role::Surface, Role::Border, false);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        Color::Reset
    };

    // Show Live Batch Progress when running or right after a batch run in Optimization view
    if group == OPTIMIZATION_GROUP && app.progress.is_some() {
        draw_batch_progress(app, frame, geom, bg);
        return;
    }

    // Show Execution Plan when on Execute row in Optimization view
    if group == OPTIMIZATION_GROUP && matches!(app.selected_row(), Some(Row::Execute)) {
        draw_optimization_plan(app, frame, geom, bg);
        return;
    }

    let Some(Row::Action(spec)) = app.selected_row() else {
        return;
    };

    let buf = frame.buffer_mut();
    let title_y = area.y;
    let insp_title_bg = tokens::card_title_bg(theme, Role::Surface);
    let header_title = i.text("ui.details");
    let formatted_hdr = tokens::format_card_title(theme, header_title, theme.flags.framed);
    buf.set_stringn(
        area.x + 2,
        title_y,
        &formatted_hdr,
        area.width.saturating_sub(4) as usize,
        Style::default()
            .fg(tokens::card_title_fg(theme, false))
            .bg(insp_title_bg)
            .add_modifier(Modifier::BOLD),
    );
    if area.width >= 26 {
        let id_tag = if theme.flags.framed {
            format!(" {} ", spec.id)
        } else {
            spec.id.to_owned()
        };
        if id_tag.width() + formatted_hdr.width() + 4 <= area.width as usize {
            tokens::put_right(
                buf,
                Rect::new(area.x + 2, title_y, area.width.saturating_sub(4), 1),
                &id_tag,
                tokens::text(theme, Level::Tertiary).bg(insp_title_bg),
            );
        }
    }

    let inner = geom.inspector_inner;
    if inner.width < 8 || inner.height == 0 {
        return;
    }

    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Action title
    lines.push(Line::from(Span::styled(
        clip_ascii_safe(i.text(&spec.label()), width, theme.flags.ascii),
        tokens::fg(theme, Role::Text1).add_modifier(Modifier::BOLD),
    )));

    // Permission / impact / parameter-count badges (wrap cleanly across lines so pills never clip;
    // omit on ultra-short stacked mobile inspectors so title + description stay visible).
    if !(geom.stacked && inner.height <= 2) {
        let mut badge_chips = vec![
            tokens::badge_span(
                theme,
                if spec.changes {
                    i.text("ui.mutating")
                } else {
                    i.text("ui.readonly")
                },
                if spec.changes {
                    Tone::Info
                } else {
                    Tone::Success
                },
            ),
            tokens::badge_span(
                theme,
                if spec.root {
                    i.text("ui.root")
                } else {
                    i.text("ui.user")
                },
                if spec.root && !app.root {
                    Tone::Warning
                } else {
                    Tone::Muted
                },
            ),
        ];
        if !spec.fields.is_empty() {
            let count_lbl = i.format(
                "ui.params_count",
                &[("count", &spec.fields.len().to_string())],
            );
            badge_chips.push(tokens::badge_span(theme, count_lbl, Tone::Accent));
        }

        let mut cur_row: Vec<Span<'static>> = Vec::new();
        let mut cur_w = 0usize;
        for chip in badge_chips {
            let cw = chip.width();
            if !cur_row.is_empty() && cur_w + 1 + cw > width {
                if geom.stacked {
                    break;
                }
                lines.push(Line::from(std::mem::take(&mut cur_row)));
                cur_w = 0;
            }
            if !cur_row.is_empty() {
                cur_row.push(Span::raw(" "));
                cur_w += 1;
            }
            cur_w += cw;
            cur_row.push(chip);
        }
        if !cur_row.is_empty() {
            lines.push(Line::from(cur_row));
        }
    }

    // Availability warning if blocked (e.g. non-root or Windows preview)
    if let Err(err) = app.available(&spec) {
        lines.push(Line::from(Span::styled(
            clip_ascii_safe(&i.error(&err), width, theme.flags.ascii),
            tokens::tone_style(theme, Tone::Warning),
        )));
    }

    if !geom.stacked {
        let rule = tokens::glyph(theme, Glyph::Rule).repeat(width);
        lines.push(Line::from(Span::styled(
            rule,
            tokens::fg(theme, Role::Rule),
        )));
    }

    // Description
    for row in wrap_words(i.text(&spec.description()), width) {
        lines.push(Line::from(Span::styled(
            row,
            tokens::text(theme, Level::Secondary),
        )));
    }

    // Live Related state block
    if !geom.stacked
        && let Some(rel) = app.related_for(&spec)
        && !rel.state.is_empty()
    {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            i.text("ui.current_state").to_owned(),
            tokens::fg(theme, Role::Accent2).add_modifier(Modifier::BOLD),
        )));
        for state_line in rel.state {
            let text = state_line.to_string();
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&text, width, theme.flags.ascii),
                tokens::text(theme, Level::Primary),
            )));
        }
    }

    // Expected changes preview (e.g. tcp.tune / bbr.enable / timezone)
    if !geom.stacked {
        let params = app.saved_parameters(&spec);
        let changes = related::expected(&spec, &params, &app.snapshot);
        if !changes.is_empty() {
            lines.push(Line::raw(""));
            for ch in changes {
                let arrow = if theme.flags.ascii { "->" } else { "→" };
                let line = format!("{}  {} {arrow} {}", i.text(ch.label), ch.from, ch.to);
                lines.push(Line::from(Span::styled(
                    clip_ascii_safe(&line, width, theme.flags.ascii),
                    tokens::fg(theme, Role::Accent2),
                )));
            }
        }
    }

    // Live miniature terminal prompt preview in the Terminal Beautification inspector
    if !geom.stacked && spec.group == "terminal" && width >= 20 {
        let live_opts = app.terminal_preview_options();
        let host = app.preview_host();
        lines.push(Line::raw(""));
        let header = format!(
            "{} · {} / {}",
            i.text("ui.prompt_live_preview"),
            i.text(&format!("prompt.style.{}.name", live_opts.style)),
            i.text(&format!("prompt.color.{}.name", live_opts.color)),
        );
        lines.push(Line::from(Span::styled(
            clip_ascii_safe(&header, width, theme.flags.ascii),
            tokens::text(theme, Level::Tertiary).add_modifier(Modifier::BOLD),
        )));
        for pline in crate::ops::terminal::preview_lines(
            &live_opts,
            host,
            crate::ops::terminal::PreviewScenario::Normal,
            width,
            theme.flags.ascii,
        ) {
            lines.push(super::terminal_preview::to_ratatui_line(&pline, theme));
        }
    }

    // Equivalent CLI command preview inside a recessed surface strip (click to copy)
    let params = app.saved_parameters(&spec);
    let words = preview::single_words(&spec, &params);
    let shown: Vec<String> = words.iter().map(|w| w.shown().into_owned()).collect();
    if !geom.stacked {
        lines.push(Line::raw(""));
    }
    let cmd_start_line = lines.len();
    let cmd_budget = width.saturating_sub(4).max(1);
    let cmd_rows: Vec<String> = preview::wrap(&shown, cmd_budget)
        .into_iter()
        .take(if geom.stacked { 1 } else { 3 })
        .collect();
    let cmd_end_line = cmd_start_line + cmd_rows.len();

    // Precompute command strip screen rect (accounting for body height & scroll) so hover/click works
    let body_h_est = inner
        .height
        .saturating_sub(if geom.inspector_cta.is_some() { 2 } else { 0 });
    let total_est = cmd_end_line + usize::from(!geom.stacked) * 2;
    let scroll_est = app.inspector_scroll.min(
        total_est
            .saturating_sub(body_h_est as usize)
            .min(u16::MAX as usize) as u16,
    ) as usize;
    let cmd_screen_rect =
        if cmd_end_line > scroll_est && cmd_start_line < scroll_est + body_h_est as usize {
            let vis_start = cmd_start_line.saturating_sub(scroll_est) as u16;
            let vis_end = (cmd_end_line.saturating_sub(scroll_est)).min(body_h_est as usize) as u16;
            (vis_end > vis_start).then(|| {
                Rect::new(
                    inner.x,
                    inner.y + vis_start,
                    inner.width,
                    vis_end - vis_start,
                )
            })
        } else {
            None
        };
    app.last_cmd_rect.set(cmd_screen_rect);

    let cmd_hovered = cmd_screen_rect.is_some_and(|r| app.hover.is_some_and(|p| r.contains(p)));
    let cmd_bg = if cmd_hovered && theme.flags.paint_bg {
        tokens::color(theme, Role::Selection)
    } else if theme.flags.paint_bg {
        tokens::color(theme, Role::SurfaceAlt)
    } else {
        bg
    };
    let cmd_style = if preview::incomplete(&words) {
        tokens::text(theme, Level::Tertiary).bg(cmd_bg)
    } else {
        tokens::fg(theme, Role::Accent).bg(cmd_bg)
    };
    for (idx, wrapped) in cmd_rows.into_iter().enumerate() {
        let prefix = if idx == 0 { " $ " } else { "   " };
        let clipped = clip_ascii_safe(&wrapped, cmd_budget, theme.flags.ascii);
        let pad = width.saturating_sub(prefix.width() + clipped.width());
        lines.push(Line::from(vec![
            Span::styled(
                prefix,
                tokens::fg(theme, Role::Accent)
                    .bg(cmd_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(clipped, cmd_style),
            Span::styled(" ".repeat(pad), Style::default().bg(cmd_bg)),
        ]));
    }

    // Last session result
    if !geom.stacked {
        lines.push(Line::raw(""));
        if let Some(last) = app.records.last.get(spec.id) {
            let (sym, tone) = match last.outcome {
                Outcome::Done => (tokens::glyph(theme, Glyph::Success), Tone::Success),
                Outcome::Failed => (tokens::glyph(theme, Glyph::Failure), Tone::Danger),
                Outcome::Cancelled => (tokens::glyph(theme, Glyph::Cancelled), Tone::Warning),
            };
            let rel = related::relative_age(i, app.now.saturating_duration_since(last.finished));
            let dur = format_mm_ss(last.elapsed);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", i.text("ui.last_result")),
                    tokens::text(theme, Level::Tertiary),
                ),
                Span::styled(
                    format!("{sym} {dur} · {rel}"),
                    tokens::tone_style(theme, tone),
                ),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                format!("{}  {}", i.text("ui.last_result"), i.text("ui.not_run_yet")),
                tokens::text(theme, Level::Tertiary),
            )));
        }
    }

    let body_h = inner
        .height
        .saturating_sub(if geom.inspector_cta.is_some() { 2 } else { 0 });
    let max_scroll = lines
        .len()
        .saturating_sub(body_h as usize)
        .min(u16::MAX as usize) as u16;
    app.max_inspector_scroll.set(max_scroll);
    let scroll = app.inspector_scroll.min(max_scroll);
    let body_rect = Rect::new(inner.x, inner.y, inner.width, body_h);
    let sel_rows: Vec<SelectableRow> = lines
        .iter()
        .skip(scroll as usize)
        .take(body_h as usize)
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            SelectableRow {
                text: text.trim_end().to_owned(),
                soft_wrap: false,
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body_rect);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane: TextPane::Inspector,
        bounds: body_rect,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == TextPane::Inspector
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }

    // Bottom clickable primary CTA pill + secondary Copy Command pill inside Inspector
    if let Some(cta_rect) = geom.inspector_cta {
        let action_hint = if !spec.fields.is_empty() {
            i.text("key.open")
        } else {
            i.text("key.accept")
        };
        let hovered = app.hover.is_some_and(|p| cta_rect.contains(p));
        let available = app.available(&spec).is_ok();
        let buf = frame.buffer_mut();
        if theme.flags.ascii {
            let cta = format!("[ {} {action_hint} ]", tokens::glyph(theme, Glyph::Enter));
            let style = if available {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                tokens::text(theme, Level::Tertiary)
            };
            buf.set_stringn(
                cta_rect.x,
                cta_rect.y,
                clip_ascii_safe(&cta, cta_rect.width as usize, true),
                cta_rect.width as usize,
                style,
            );
        } else {
            let cta = format!("  {} {action_hint}  ", tokens::glyph(theme, Glyph::Enter));
            let pill_w = (cta.width() as u16).min(cta_rect.width);
            let (cta_fg, cta_bg) = if !available {
                (
                    tokens::color(theme, Role::Text3),
                    if theme.flags.paint_bg {
                        tokens::color(theme, Role::SurfaceAlt)
                    } else {
                        bg
                    },
                )
            } else if hovered {
                (
                    tokens::color(theme, Role::OnAccent),
                    tokens::color(theme, Role::Accent2),
                )
            } else {
                (
                    tokens::color(theme, Role::OnAccent),
                    tokens::color(theme, Role::Accent),
                )
            };
            for col in cta_rect.x..cta_rect.x + pill_w {
                buf[(col, cta_rect.y)].set_symbol(" ").set_bg(cta_bg);
            }
            buf.set_stringn(
                cta_rect.x,
                cta_rect.y,
                clip_ascii_safe(&cta, pill_w as usize, false),
                pill_w as usize,
                Style::default()
                    .fg(cta_fg)
                    .bg(cta_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
    if let Some(copy_rect) = geom.inspector_copy_btn {
        draw_inspector_copy_btn(app, frame.buffer_mut(), copy_rect, bg);
    }
}

fn draw_inspector_copy_btn(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    base_bg: Color,
) {
    let theme = &app.theme.active;
    let lbl = app.ctx.i18n.text("key.copy_cmd");
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    if theme.flags.ascii {
        let text = format!("[c {lbl}]");
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&text, rect.width as usize, true),
            rect.width as usize,
            tokens::text(theme, Level::Secondary),
        );
    } else {
        let (fg_col, bg_col) = if hovered {
            (
                tokens::color(theme, Role::Accent),
                tokens::color(theme, Role::Selection),
            )
        } else {
            (
                tokens::color(theme, Role::Text2),
                if theme.flags.paint_bg {
                    tokens::color(theme, Role::SurfaceAlt)
                } else {
                    base_bg
                },
            )
        };
        for col in rect.x..rect.right() {
            buf[(col, rect.y)].set_symbol(" ").set_bg(bg_col);
        }
        buf.set_stringn(
            rect.x,
            rect.y,
            " c ",
            rect.width as usize,
            Style::default()
                .fg(tokens::color(theme, Role::Accent))
                .bg(bg_col)
                .add_modifier(Modifier::BOLD),
        );
        let rest_w = rect.width.saturating_sub(3) as usize;
        if rest_w > 0 {
            buf.set_stringn(
                rect.x + 3,
                rect.y,
                clip_ascii_safe(&format!("{lbl} "), rest_w, false),
                rest_w,
                Style::default().fg(fg_col).bg(bg_col),
            );
        }
    }
}

fn draw_optimization_plan(app: &App, frame: &mut Frame, geom: &WorkbenchGeometry, bg: Color) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let area = geom.inspector_area;
    let buf = frame.buffer_mut();
    let title_y = area.y;
    let title_bg = tokens::card_title_bg(theme, Role::Surface);
    let plan_title = tokens::format_card_title(theme, i.text("ui.plan_title"), theme.flags.framed);
    buf.set_stringn(
        area.x + 2,
        title_y,
        &plan_title,
        area.width.saturating_sub(4) as usize,
        Style::default()
            .fg(tokens::card_title_fg(theme, false))
            .bg(title_bg)
            .add_modifier(Modifier::BOLD),
    );

    let inner = geom.inspector_inner;
    if inner.width < 8 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let jobs = app.checklist();
    let mut lines = Vec::new();
    if jobs.is_empty() {
        lines.push(Line::from(Span::styled(
            i.text("ui.no_selection").to_owned(),
            tokens::text(theme, Level::Secondary),
        )));
    } else {
        for (idx, (spec, params)) in jobs.iter().enumerate() {
            let summary = related::summary(spec, params);
            let item = if summary.is_empty() {
                format!("{}. {}", idx + 1, i.text(&spec.label()))
            } else {
                format!("{}. {} · {summary}", idx + 1, i.text(&spec.label()))
            };
            lines.push(Line::from(Span::styled(
                clip_ascii_safe(&item, width, theme.flags.ascii),
                tokens::text(theme, Level::Primary),
            )));
        }
        lines.push(Line::raw(""));
        let cmd_start = lines.len();
        let words = preview::batch_words(&jobs);
        let shown: Vec<String> = words.iter().map(|w| w.shown().into_owned()).collect();
        let cmd_budget = width.saturating_sub(4).max(1);
        let cmd_rows: Vec<String> = preview::wrap(&shown, cmd_budget)
            .into_iter()
            .take(4)
            .collect();
        let body_h = inner
            .height
            .saturating_sub(if geom.inspector_copy_btn.is_some() {
                2
            } else {
                0
            });
        if cmd_start < body_h as usize && !cmd_rows.is_empty() {
            let vis_h = (cmd_rows.len() as u16).min(body_h.saturating_sub(cmd_start as u16));
            app.last_cmd_rect.set(Some(Rect::new(
                inner.x,
                inner.y + cmd_start as u16,
                inner.width,
                vis_h,
            )));
        }
        let cmd_hovered = app
            .last_cmd_rect
            .get()
            .is_some_and(|r| app.hover.is_some_and(|p| r.contains(p)));
        let cmd_bg = if cmd_hovered && theme.flags.paint_bg {
            tokens::color(theme, Role::Selection)
        } else if theme.flags.paint_bg {
            tokens::color(theme, Role::SurfaceAlt)
        } else {
            bg
        };
        let style = if preview::incomplete(&words) {
            tokens::text(theme, Level::Tertiary).bg(cmd_bg)
        } else {
            tokens::fg(theme, Role::Accent).bg(cmd_bg)
        };
        for (idx, row) in cmd_rows.into_iter().enumerate() {
            let prefix = if idx == 0 { " $ " } else { "   " };
            let clipped = clip_ascii_safe(&row, cmd_budget, theme.flags.ascii);
            let pad = width.saturating_sub(prefix.width() + clipped.width());
            lines.push(Line::from(vec![
                Span::styled(
                    prefix,
                    tokens::fg(theme, Role::Accent)
                        .bg(cmd_bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(clipped, style),
                Span::styled(" ".repeat(pad), Style::default().bg(cmd_bg)),
            ]));
        }
    }
    let body_h =
        inner
            .height
            .saturating_sub(if !jobs.is_empty() && geom.inspector_copy_btn.is_some() {
                2
            } else {
                0
            });
    let body_rect = Rect::new(inner.x, inner.y, inner.width, body_h);
    let sel_rows: Vec<SelectableRow> = lines
        .iter()
        .take(body_h as usize)
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            SelectableRow {
                text: text.trim_end().to_owned(),
                soft_wrap: false,
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body_rect);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane: TextPane::Inspector,
        bounds: body_rect,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == TextPane::Inspector
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }
    if !jobs.is_empty() {
        if let Some(cta_rect) = geom.inspector_cta {
            let action_hint = i.text("key.run_checklist");
            let hovered = app.hover.is_some_and(|p| cta_rect.contains(p));
            let buf = frame.buffer_mut();
            if theme.flags.ascii {
                let cta = format!("[ {} {action_hint} ]", tokens::glyph(theme, Glyph::Enter));
                buf.set_stringn(
                    cta_rect.x,
                    cta_rect.y,
                    clip_ascii_safe(&cta, cta_rect.width as usize, true),
                    cta_rect.width as usize,
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
                );
            } else {
                let cta = format!("  {} {action_hint}  ", tokens::glyph(theme, Glyph::Enter));
                let pill_w = (cta.width() as u16).min(cta_rect.width);
                let cta_bg = if hovered {
                    tokens::color(theme, Role::Accent2)
                } else {
                    tokens::color(theme, Role::Accent)
                };
                for col in cta_rect.x..cta_rect.x + pill_w {
                    buf[(col, cta_rect.y)].set_symbol(" ").set_bg(cta_bg);
                }
                buf.set_stringn(
                    cta_rect.x,
                    cta_rect.y,
                    clip_ascii_safe(&cta, pill_w as usize, false),
                    pill_w as usize,
                    Style::default()
                        .fg(tokens::color(theme, Role::OnAccent))
                        .bg(cta_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
        if let Some(copy_rect) = geom.inspector_copy_btn {
            draw_inspector_copy_btn(app, frame.buffer_mut(), copy_rect, bg);
        }
    }
}

fn draw_batch_progress(app: &App, frame: &mut Frame, geom: &WorkbenchGeometry, bg: Color) {
    let Some(progress) = &app.progress else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let area = geom.inspector_area;
    let buf = frame.buffer_mut();
    let title_y = area.y;
    let title_bg = tokens::card_title_bg(theme, Role::Surface);
    let progress_title =
        tokens::format_card_title(theme, i.text("ui.progress_title"), theme.flags.framed);
    buf.set_stringn(
        area.x + 2,
        title_y,
        &progress_title,
        area.width.saturating_sub(4) as usize,
        Style::default()
            .fg(tokens::card_title_fg(theme, true))
            .bg(title_bg)
            .add_modifier(Modifier::BOLD),
    );

    let inner = geom.inspector_inner;
    if inner.width < 10 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let mut lines = Vec::new();
    for (idx, step_state) in progress.steps.iter().enumerate() {
        let (sym, tone, time_str) = match *step_state {
            StepView::Running(started) => (
                app.spin(),
                Tone::Info,
                format_mm_ss(app.now.saturating_duration_since(started)),
            ),
            StepView::Done(dur) => (
                tokens::glyph(theme, Glyph::Success),
                Tone::Success,
                format_mm_ss(dur),
            ),
            StepView::Failed(dur) => (
                tokens::glyph(theme, Glyph::Failure),
                Tone::Danger,
                format_mm_ss(dur),
            ),
            StepView::Cancelled(dur) => (
                tokens::glyph(theme, Glyph::Cancelled),
                Tone::Warning,
                format_mm_ss(dur),
            ),
            StepView::Pending => (
                tokens::glyph(theme, Glyph::Pending),
                Tone::Muted,
                String::new(),
            ),
            StepView::Skipped => (
                tokens::glyph(theme, Glyph::Skipped),
                Tone::Muted,
                String::new(),
            ),
        };
        let label = app
            .task_ids
            .get(idx)
            .map(|id| i.text(&format!("action.{id}")))
            .unwrap_or("?");
        let left = format!("{sym} {} {label}", idx + 1);
        let right_w = if time_str.is_empty() {
            0
        } else {
            time_str.width() + 2
        };
        let clipped_left = clip_ascii_safe(&left, width.saturating_sub(right_w), theme.flags.ascii);
        let gap = width.saturating_sub(clipped_left.width() + time_str.width());
        lines.push(Line::from(vec![
            Span::styled(clipped_left, tokens::tone_style(theme, tone)),
            Span::raw(" ".repeat(gap)),
            Span::styled(time_str, tokens::text(theme, Level::Tertiary)),
        ]));
    }

    let bar_reserve = if inner.height >= 3 { 2 } else { 0 };
    let body_h = inner.height.saturating_sub(bar_reserve);
    let body_rect = Rect::new(inner.x, inner.y, inner.width, body_h);
    let sel_rows: Vec<SelectableRow> = lines
        .iter()
        .take(body_h as usize)
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            SelectableRow {
                text: text.trim_end().to_owned(),
                soft_wrap: false,
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body_rect);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane: TextPane::Inspector,
        bounds: body_rect,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == TextPane::Inspector
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }

    // High-visibility style-adapted batch progress bar with running shimmer at bottom of inspector
    if inner.height >= 3 {
        let bar_y = inner.bottom().saturating_sub(1);
        let total = progress.steps.len().max(1);
        let done_count = progress
            .steps
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    StepView::Done(_) | StepView::Failed(_) | StepView::Cancelled(_)
                )
            })
            .count();
        let pct = (done_count as f64 / total as f64) * 100.0;
        let elapsed = app.now.saturating_duration_since(app.started);
        let reduced = app.reduced_motion();
        let pct_label = format!(" {pct:>3.0}%");
        let bar_w = width.saturating_sub(pct_label.width()).max(4);
        let active_shimmer = if app.worker.is_some() {
            fx::shimmer_index(elapsed, bar_w, reduced)
        } else {
            None
        };
        let mut bar_spans =
            tokens::gauge_spans_fx(pct, bar_w, theme, elapsed, reduced, active_shimmer);
        bar_spans.push(Span::styled(
            pct_label,
            Style::default()
                .fg(fx::meter_pct_color(pct, theme, elapsed, reduced))
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(bar_spans)),
            Rect::new(inner.x, bar_y, inner.width, 1),
        );
    }
}
