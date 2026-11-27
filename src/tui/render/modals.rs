//! Centered elevated modal dialogs with backdrop scrim, drop shadow, and smooth reveal:
//! Command Palette, Parameter Form (with Chips), Filterable Picker, Loading, Confirmation,
//! Notice, and Help.

use super::{
    clip_ascii_safe, highlight_spans,
    tokens::{self, Glyph},
    wrap_cells, wrap_words,
};
use crate::{
    actions,
    theme::{Level, Role},
    tui::{
        fx,
        layout::{
            self, CommandPaletteGeometry, ConfirmationGeometry, FormGeometry, PickerGeometry,
        },
        palette::{self, PaletteItem, PaletteRow},
        preview,
        selection::{self, SelectablePane, SelectableRow, TextPane},
        state::{App, Form, FormMode, Loading},
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

pub(super) fn draw_palette(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let Some(pal) = &app.command_palette else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let specs = actions::specs_slice();
    let results = palette::search(&pal.query.text, specs, i, &app.category_order, &app.records);
    let geom =
        CommandPaletteGeometry::compute(area, &results, pal.cursor, app.resolved_density(area));

    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    tokens::draw_surface_card_fx(
        frame,
        geom.modal,
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

    // Title & result counter
    let title = format!(" {} ", i.text("ui.palette_title"));
    let count_str = format!(" {} / {} ", results.count(), results.total_items);
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        &title,
        geom.modal.width.saturating_sub(14) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    tokens::put_right(
        buf,
        Rect::new(
            geom.modal.x + 2,
            geom.modal.y,
            geom.modal.width.saturating_sub(4),
            1,
        ),
        &count_str,
        tokens::text(theme, Level::Tertiary).bg(bg),
    );

    // Search input row
    let input_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Surface)
    } else {
        bg
    };
    for col in geom.input.left()..geom.input.right() {
        buf[(col, geom.input.y)].set_symbol(" ").set_bg(input_bg);
    }
    let prompt = format!(" {} ", tokens::glyph(theme, Glyph::Chevron));
    buf.set_stringn(
        geom.input.x,
        geom.input.y,
        &prompt,
        3,
        tokens::fg(theme, Role::Accent)
            .bg(input_bg)
            .add_modifier(Modifier::BOLD),
    );
    let field_w = geom.input.width.saturating_sub(4) as usize;
    if pal.query.text.is_empty() {
        buf.set_stringn(
            geom.input.x + 3,
            geom.input.y,
            clip_ascii_safe(i.text("ui.palette_hint"), field_w, theme.flags.ascii),
            field_w,
            tokens::text(theme, Level::Tertiary).bg(input_bg),
        );
    } else {
        let (vis, cursor_x) = pal.query.visible(field_w);
        buf.set_stringn(
            geom.input.x + 3,
            geom.input.y,
            &vis,
            field_w,
            tokens::text(theme, Level::Primary).bg(input_bg),
        );
        let cx = (geom.input.x + 3 + cursor_x).min(geom.input.right().saturating_sub(1));
        buf[(cx, geom.input.y)].set_style(
            Style::default()
                .fg(tokens::color(theme, Role::OnAccent))
                .bg(tokens::color(theme, Role::Accent)),
        );
    }

    tokens::draw_rule(
        buf,
        geom.modal.x + 2,
        geom.input.y + 1,
        geom.modal.width.saturating_sub(4),
        theme,
        bg,
    );

    // Results list
    if results.count() == 0 {
        if geom.list.height >= 1 {
            frame.buffer_mut().set_stringn(
                geom.list.x + 2,
                geom.list.y + u16::from(geom.list.height >= 2),
                clip_ascii_safe(
                    i.text("palette.empty"),
                    geom.list.width.saturating_sub(4) as usize,
                    theme.flags.ascii,
                ),
                geom.list.width.saturating_sub(4) as usize,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
    } else {
        let sel_row_idx = results.row_of(pal.cursor);
        let hi_style = Style::default()
            .fg(tokens::selected_accent(theme))
            .add_modifier(Modifier::BOLD);

        for &(row_rect, abs_idx) in &geom.row_rects {
            let Some(row) = results.rows.get(abs_idx) else {
                continue;
            };
            let y = row_rect.y;

            match row {
                PaletteRow::Header(title) => {
                    frame.buffer_mut().set_stringn(
                        row_rect.x + 1,
                        y,
                        clip_ascii_safe(
                            title,
                            row_rect.width.saturating_sub(2) as usize,
                            theme.flags.ascii,
                        ),
                        row_rect.width.saturating_sub(2) as usize,
                        tokens::text(theme, Level::Tertiary)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                PaletteRow::Item(item) => {
                    let active = abs_idx == sel_row_idx;
                    let hovered = app.hover.is_some_and(|p| row_rect.contains(p));
                    let row_bg = if active {
                        tokens::color(theme, Role::Selection)
                    } else if hovered && theme.flags.paint_bg {
                        tokens::color(theme, Role::SurfaceAlt)
                    } else {
                        bg
                    };
                    {
                        let buf = frame.buffer_mut();
                        for ry in row_rect.top()..row_rect.bottom() {
                            for col in row_rect.left()..row_rect.right() {
                                buf[(col, ry)].set_symbol(" ").set_bg(row_bg);
                            }
                            if active {
                                buf[(row_rect.x, ry)]
                                    .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                                    .set_style(
                                        Style::default()
                                            .fg(tokens::selected_accent(theme))
                                            .bg(row_bg)
                                            .add_modifier(Modifier::BOLD),
                                    );
                            }
                        }
                    }

                    let base_fg = if active {
                        tokens::selected_fg(theme)
                    } else {
                        tokens::color(theme, Role::Text1)
                    };
                    let base_style = Style::default().fg(base_fg).bg(row_bg);
                    let match_style = hi_style.bg(row_bg);

                    match item {
                        PaletteItem::Command {
                            command,
                            label_ranges,
                        } => {
                            let label = i.text(command.label_key());
                            let mut spans = vec![Span::styled(
                                format!("{} ", tokens::glyph(theme, Glyph::Chevron)),
                                Style::default()
                                    .fg(tokens::selected_accent(theme))
                                    .bg(row_bg),
                            )];
                            spans.extend(highlight_spans(
                                label,
                                label_ranges,
                                base_style,
                                match_style,
                            ));
                            let right_key = format!("[{}]", command.shortcut());
                            let left_w =
                                row_rect.width.saturating_sub(right_key.width() as u16 + 4);
                            frame.render_widget(
                                Paragraph::new(Line::from(spans)),
                                Rect::new(row_rect.x + 2, y, left_w, 1),
                            );
                            tokens::put_right(
                                frame.buffer_mut(),
                                Rect::new(row_rect.x, y, row_rect.width.saturating_sub(1), 1),
                                &right_key,
                                Style::default()
                                    .fg(tokens::color(theme, Role::Accent))
                                    .bg(row_bg),
                            );
                        }
                        PaletteItem::Action {
                            spec,
                            label_ranges,
                            id_ranges,
                            ..
                        } => {
                            let label = i.text(&spec.label());
                            let mut spans =
                                highlight_spans(label, label_ranges, base_style, match_style);
                            if !spec.fields.is_empty() {
                                spans.push(Span::styled(
                                    if theme.flags.ascii { "..." } else { "…" },
                                    base_style,
                                ));
                            }
                            if let Some(id_rng) = id_ranges {
                                spans.push(Span::styled("  ", base_style));
                                let id_base = Style::default()
                                    .fg(tokens::color(theme, Role::Text3))
                                    .bg(row_bg);
                                spans.extend(highlight_spans(
                                    spec.id,
                                    id_rng,
                                    id_base,
                                    match_style,
                                ));
                            }
                            let right_meta = if spec.root && !app.root {
                                format!(
                                    "root  {}",
                                    if active {
                                        tokens::glyph(theme, Glyph::Enter)
                                    } else {
                                        ""
                                    }
                                )
                            } else if active {
                                tokens::glyph(theme, Glyph::Enter).to_owned()
                            } else {
                                String::new()
                            };
                            let right_w = if right_meta.is_empty() {
                                0
                            } else {
                                right_meta.width() as u16 + 2
                            };
                            let left_w = row_rect.width.saturating_sub(right_w + 3);
                            frame.render_widget(
                                Paragraph::new(Line::from(spans)),
                                Rect::new(row_rect.x + 2, y, left_w, 1),
                            );
                            if !right_meta.is_empty() {
                                tokens::put_right(
                                    frame.buffer_mut(),
                                    Rect::new(row_rect.x, y, row_rect.width.saturating_sub(1), 1),
                                    &right_meta,
                                    Style::default()
                                        .fg(if spec.root && !app.root {
                                            tokens::color(theme, Role::Warning)
                                        } else {
                                            tokens::selected_accent(theme)
                                        })
                                        .bg(row_bg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    if geom.prev_btn.is_some() || geom.next_btn.is_some() {
        tokens::draw_pager_buttons(
            frame.buffer_mut(),
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

    // Bottom palette hints
    let hint_y = geom.modal.bottom().saturating_sub(2);
    let content_bottom = geom
        .prev_btn
        .or(geom.next_btn)
        .map_or(geom.list.bottom(), Rect::bottom);
    if hint_y >= content_bottom && hint_y > geom.list.y {
        let hint_w = geom.modal.width.saturating_sub(4);
        let hints = tokens::fit_keycaps(
            theme,
            &[
                ("↑↓", i.text("key.move")),
                ("↵", i.text("key.accept")),
                ("Tab", i.text("key.jump")),
                ("Ctrl+U", i.text("key.clear")),
                ("Esc", i.text("key.close")),
            ],
            hint_w as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(geom.modal.x + 2, hint_y, hint_w, 1),
        );
    }

    fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
}

pub(super) fn draw_form(app: &App, frame: &mut Frame, area: Rect, form: &Form, alpha: f64) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let geom = FormGeometry::compute(area, form, app);

    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    tokens::draw_surface_card_fx(
        frame,
        geom.modal,
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

    let title = format!(" {} ", i.text(&form.spec.label()));
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        &title,
        geom.modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let sub_key = if form.mode == FormMode::Optimization {
        "ui.form_batch_title"
    } else {
        "ui.form_single_title"
    };
    let sub_y = if geom
        .fields
        .iter()
        .find(|r| r.height > 0)
        .is_some_and(|first| first.y <= geom.modal.y + 2)
    {
        geom.modal.y + 1
    } else {
        geom.modal.y + 2
    };
    buf.set_stringn(
        geom.modal.x + 2,
        sub_y,
        clip_ascii_safe(
            i.text(sub_key),
            geom.modal.width.saturating_sub(4) as usize,
            theme.flags.ascii,
        ),
        geom.modal.width.saturating_sub(4) as usize,
        tokens::text(theme, Level::Secondary).bg(bg),
    );

    // Fields
    for (idx, (field, field_rect)) in form.spec.fields.iter().zip(&geom.fields).enumerate() {
        if field_rect.height == 0 || field_rect.width < 4 {
            continue;
        }
        let active = form.index == idx;
        let req = if field.required { " *" } else { "" };
        let label = format!("{}{req}", i.text(&field.label()));
        let label_fg = if active {
            tokens::color(theme, Role::Accent)
        } else {
            tokens::color(theme, Role::Text2)
        };
        if active {
            buf[(field_rect.x, field_rect.y)]
                .set_symbol(tokens::glyph(theme, Glyph::ActiveBar))
                .set_style(
                    Style::default()
                        .fg(label_fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                );
        }
        buf.set_stringn(
            field_rect.x + 2,
            field_rect.y,
            clip_ascii_safe(
                &label,
                field_rect.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            field_rect.width.saturating_sub(4) as usize,
            Style::default()
                .fg(label_fg)
                .bg(bg)
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );

        let val_y = field_rect.y + 1;
        if val_y >= geom.modal.bottom().saturating_sub(2) {
            continue;
        }
        if form.chip_field(idx) {
            for (fi, ci, chip_rect) in &geom.chips {
                if *fi != idx {
                    continue;
                }
                let choice = field.choices[*ci];
                let selected = form.values[idx].text == choice;
                let hovered = app.hover.is_some_and(|p| chip_rect.contains(p));
                if chip_rect.height >= 2 {
                    tokens::draw_touch_button(
                        buf,
                        *chip_rect,
                        choice,
                        None,
                        if selected {
                            tokens::TouchBtnKind::Primary
                        } else {
                            tokens::TouchBtnKind::Secondary
                        },
                        hovered,
                        theme,
                        bg,
                    );
                    continue;
                }
                let (c_fg, c_bg) = if selected {
                    (
                        tokens::color(theme, Role::OnAccent),
                        tokens::color(theme, Role::Accent),
                    )
                } else if hovered {
                    (
                        tokens::color(theme, Role::Text1),
                        tokens::color(theme, Role::Selection),
                    )
                } else {
                    (
                        tokens::color(theme, Role::Text2),
                        if theme.flags.paint_bg {
                            tokens::color(theme, Role::SurfaceAlt)
                        } else {
                            bg
                        },
                    )
                };
                buf.set_stringn(
                    chip_rect.x,
                    chip_rect.y,
                    format!(" {choice} "),
                    chip_rect.width as usize,
                    Style::default()
                        .fg(c_fg)
                        .bg(c_bg)
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
            }
        } else {
            let input_bg = if active && theme.flags.paint_bg {
                tokens::color(theme, Role::Surface)
            } else if theme.flags.paint_bg {
                tokens::color(theme, Role::Sidebar)
            } else {
                bg
            };
            let box_x = field_rect.x + 2;
            let box_w = field_rect.width.saturating_sub(3);
            for col in box_x..(box_x + box_w).min(geom.modal.right().saturating_sub(1)) {
                buf[(col, val_y)].set_symbol(" ").set_bg(input_bg);
            }
            let (vis, cx) = form.values[idx].visible(box_w.saturating_sub(2) as usize);
            let display = if vis.is_empty() && form.listed(idx) {
                i.text("ui.not_set").to_owned()
            } else {
                vis
            };
            buf.set_stringn(
                box_x + 1,
                val_y,
                &display,
                box_w.saturating_sub(2) as usize,
                tokens::text(theme, Level::Primary).bg(input_bg),
            );
            if active && !form.listed(idx) && box_w > 0 {
                let cursor_col = (box_x + 1 + cx)
                    .min(box_x + box_w.saturating_sub(1))
                    .min(geom.modal.right().saturating_sub(2));
                if cursor_col < buf.area.right() && val_y < buf.area.bottom() {
                    buf[(cursor_col, val_y)].set_style(
                        Style::default()
                            .fg(tokens::color(theme, Role::OnAccent))
                            .bg(tokens::color(theme, Role::Accent)),
                    );
                }
            }
            if form.preset_chips(idx) {
                for (fi, ci, chip_rect) in &geom.chips {
                    if *fi != idx {
                        continue;
                    }
                    let choice = field.choices[*ci];
                    let selected = form.values[idx].text.eq_ignore_ascii_case(choice);
                    let hovered = app.hover.is_some_and(|p| chip_rect.contains(p));
                    if chip_rect.height >= 2 {
                        tokens::draw_touch_button(
                            buf,
                            *chip_rect,
                            choice,
                            None,
                            if selected {
                                tokens::TouchBtnKind::Primary
                            } else {
                                tokens::TouchBtnKind::Secondary
                            },
                            hovered,
                            theme,
                            bg,
                        );
                        continue;
                    }
                    let (c_fg, c_bg) = if selected {
                        (
                            tokens::color(theme, Role::OnAccent),
                            tokens::color(theme, Role::Accent),
                        )
                    } else if hovered {
                        (
                            tokens::color(theme, Role::Text1),
                            tokens::color(theme, Role::Selection),
                        )
                    } else {
                        (
                            tokens::color(theme, Role::Text2),
                            if theme.flags.paint_bg {
                                tokens::color(theme, Role::SurfaceAlt)
                            } else {
                                bg
                            },
                        )
                    };
                    buf.set_stringn(
                        chip_rect.x,
                        chip_rect.y,
                        format!(" {choice} "),
                        chip_rect.width as usize,
                        Style::default()
                            .fg(c_fg)
                            .bg(c_bg)
                            .add_modifier(if selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    );
                }
            }
        }
    }

    // Embedded live terminal prompt preview when configuring `terminal.apply`
    if let Some(preview_rect) = geom.terminal_preview {
        let params = form.parameters();
        let defaults = crate::ops::terminal::Options::default();
        let live_opts = crate::ops::terminal::Options {
            shell: params.get("shell").cloned().unwrap_or(defaults.shell),
            style: params.get("style").cloned().unwrap_or(defaults.style),
            color: params.get("color").cloned().unwrap_or(defaults.color),
            time: params.get("time").cloned().unwrap_or(defaults.time),
            git: params.get("git").cloned().unwrap_or(defaults.git),
        };
        if preview_rect.x > geom.modal.x + 4 {
            let div_x = preview_rect.x.saturating_sub(2);
            let div_sym = if theme.flags.ascii { "|" } else { "│" };
            for y in preview_rect.top()..preview_rect.bottom() {
                buf[(div_x, y)]
                    .set_symbol(div_sym)
                    .set_style(tokens::fg(theme, Role::Rule).bg(bg));
            }
        }
        super::terminal_preview::draw_form_live_preview(app, buf, preview_rect, &live_opts, bg);
    }

    // Embedded live SOCKS5 / SOCKS5h connection & security card for `clash.socks.configure`
    if let Some(preview_rect) = geom.socks_preview {
        let params = form.parameters();
        if preview_rect.x > geom.modal.x + 4 {
            let div_x = preview_rect.x.saturating_sub(2);
            let div_sym = if theme.flags.ascii { "|" } else { "│" };
            for y in preview_rect.top()..preview_rect.bottom() {
                buf[(div_x, y)]
                    .set_symbol(div_sym)
                    .set_style(tokens::fg(theme, Role::Rule).bg(bg));
            }
        }
        draw_socks_live_preview(app, buf, preview_rect, &params, bg);
    }

    // Error line or live CLI preview right above buttons
    let preview_y = geom.buttons[0].y.saturating_sub(2);
    if let Some(err) = &form.error {
        buf.set_stringn(
            geom.modal.x + 2,
            preview_y,
            clip_ascii_safe(
                err,
                geom.modal.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            geom.modal.width.saturating_sub(4) as usize,
            tokens::fg(theme, Role::Danger)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
    } else if preview_y > geom.modal.y + 4 {
        let words = preview::single_words(&form.spec, &form.parameters());
        let cmd = words
            .iter()
            .map(|w| w.shown().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let style = if preview::incomplete(&words) {
            tokens::text(theme, Level::Tertiary).bg(bg)
        } else {
            tokens::fg(theme, Role::Accent).bg(bg)
        };
        buf.set_stringn(
            geom.modal.x + 2,
            preview_y,
            clip_ascii_safe(
                &cmd,
                geom.modal.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            geom.modal.width.saturating_sub(4) as usize,
            style,
        );
    }

    // Buttons
    let n = form.values.len();
    let primary_label = if form.mode == FormMode::Optimization {
        i.text("ui.save_selection")
    } else {
        i.text("ui.continue")
    };
    draw_button(
        app,
        buf,
        geom.buttons[0],
        primary_label,
        form.index == n,
        true,
        bg,
    );
    draw_button(
        app,
        buf,
        geom.buttons[1],
        i.text("ui.cancel"),
        form.index == n + 1,
        false,
        bg,
    );

    // Hint bar
    let hint_y = geom.modal.bottom().saturating_sub(2);
    if hint_y >= geom.buttons[0].bottom() && hint_y > geom.buttons[0].y {
        let hint_w = geom.modal.width.saturating_sub(4);
        let hints = tokens::fit_keycaps(
            theme,
            &[
                ("Tab/↑↓", i.text("key.field")),
                ("←→", i.text("key.choice")),
                ("Ctrl+↵", i.text("key.submit")),
                ("Esc", i.text("ui.cancel")),
            ],
            hint_w as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(geom.modal.x + 2, hint_y, hint_w, 1),
        );
    }

    fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
}

pub(super) fn draw_picker(app: &App, frame: &mut Frame, area: Rect, form: &Form, alpha: f64) {
    let Some(picker) = &form.picker else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let choices = form.choices(form.index);
    let matches = picker.matches(choices);
    let geom = PickerGeometry::compute(
        area,
        matches.len(),
        picker.cursor,
        app.resolved_density(area),
    );

    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    tokens::draw_surface_card_fx(
        frame,
        geom.modal,
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

    let field_label = i.text(&form.spec.fields[form.index].label());
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        format!(" {field_label} "),
        geom.modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    // Filter box
    let filter_y = geom.modal.y + 2;
    let field_w = geom.modal.width.saturating_sub(8) as usize;
    let (vis, cx) = picker.filter.visible(field_w);
    buf.set_stringn(
        geom.modal.x + 2,
        filter_y,
        format!("{} ", tokens::glyph(theme, Glyph::Chevron)),
        2,
        tokens::fg(theme, Role::Accent).bg(bg),
    );
    buf.set_stringn(
        geom.modal.x + 4,
        filter_y,
        &vis,
        field_w,
        tokens::text(theme, Level::Primary).bg(bg),
    );
    let cursor_col = (geom.modal.x + 4 + cx).min(geom.modal.right().saturating_sub(2));
    buf[(cursor_col, filter_y)].set_style(
        Style::default()
            .fg(tokens::color(theme, Role::OnAccent))
            .bg(tokens::color(theme, Role::Accent)),
    );

    tokens::draw_rule(
        buf,
        geom.modal.x + 2,
        filter_y + 1,
        geom.modal.width.saturating_sub(4),
        theme,
        bg,
    );

    let rh = geom.row_height.max(1);
    let vis = (geom.list.height / rh) as usize;
    for (offset, choice) in matches.iter().skip(geom.scroll).take(vis).enumerate() {
        let idx = geom.scroll + offset;
        let y = geom.list.y + offset as u16 * rh;
        let row_rect = Rect::new(geom.list.x, y, geom.list.width, rh);
        let active = idx == picker.cursor;
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
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
                            .fg(tokens::selected_accent(theme))
                            .bg(row_bg),
                    );
            }
        }
        let mut text = choice.value.clone();
        if !choice.detail.is_empty() {
            text.push_str("  ");
            text.push_str(&choice.detail);
        }
        buf.set_stringn(
            row_rect.x + 2,
            y,
            clip_ascii_safe(
                &text,
                row_rect.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            row_rect.width.saturating_sub(4) as usize,
            Style::default()
                .fg(if active {
                    tokens::selected_fg(theme)
                } else {
                    tokens::color(theme, Role::Text1)
                })
                .bg(row_bg),
        );
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

    fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
}

pub(super) fn draw_loading(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    loading: &Loading,
    alpha: f64,
) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let modal = layout::centered(area, 52, 7);
    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card(frame, modal, theme, Role::Elevated, Role::BorderFocus, true);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    let buf = frame.buffer_mut();
    buf.set_stringn(
        modal.x + 2,
        modal.y,
        format!(" {} ", i.text(&loading.spec.label())),
        modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let msg = format!("{} {}", app.spin(), i.text("dash.loading"));
    buf.set_stringn(
        modal.x + 3,
        modal.y + 3,
        &msg,
        modal.width.saturating_sub(6) as usize,
        tokens::fg(theme, Role::Info).bg(bg),
    );
    fx::apply_fade(frame.buffer_mut(), modal, theme.palette.canvas, alpha);
}

pub(super) fn draw_confirmation(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let Some(confirm) = &app.confirmation else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let geom = ConfirmationGeometry::compute(area, confirm, i, app.resolved_density(area));

    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    let accent_role = if confirm.danger {
        Role::Danger
    } else {
        Role::Warning
    };
    tokens::draw_surface_card(frame, geom.modal, theme, Role::Elevated, accent_role, true);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    let buf = frame.buffer_mut();

    let title = format!(
        " {} {} ",
        tokens::glyph(theme, Glyph::Warning),
        i.text("ui.confirm")
    );
    buf.set_stringn(
        geom.modal.x + 2,
        geom.modal.y,
        &title,
        geom.modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, accent_role)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let wrapped = wrap_words(&confirm.text, geom.body.width as usize);
    let sel_rows: Vec<SelectableRow> = wrapped
        .iter()
        .skip(confirm.scroll as usize)
        .take(geom.body.height as usize)
        .map(|row| SelectableRow {
            text: row.clone(),
            soft_wrap: false,
        })
        .collect();
    let lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .map(|row| Line::from(Span::styled(row, tokens::text(theme, Level::Primary))))
        .collect();
    frame.render_widget(Paragraph::new(lines).scroll((confirm.scroll, 0)), geom.body);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane: TextPane::Confirm,
        bounds: geom.body,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == TextPane::Confirm
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }

    let buf = frame.buffer_mut();
    draw_button(
        app,
        buf,
        geom.buttons[0],
        i.text("ui.cancel"),
        !confirm.accept,
        false,
        bg,
    );
    draw_button(
        app,
        buf,
        geom.buttons[1],
        i.text("ui.confirm_execute"),
        confirm.accept,
        true,
        bg,
    );

    let hint_y = geom.modal.bottom().saturating_sub(2);
    if hint_y >= geom.buttons[0].bottom() && hint_y > geom.buttons[0].y {
        let hint_w = geom.modal.width.saturating_sub(4);
        let hints = tokens::fit_keycaps(
            theme,
            &[
                ("Y", i.text("key.confirm")),
                ("N/Esc", i.text("ui.cancel")),
                ("←→", i.text("key.choose")),
                ("↵", i.text("key.submit")),
            ],
            hint_w as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(geom.modal.x + 2, hint_y, hint_w, 1),
        );
    }

    fx::apply_fade(frame.buffer_mut(), geom.modal, theme.palette.canvas, alpha);
}

pub(super) fn draw_notice(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let Some(text) = &app.notice else {
        return;
    };
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let (modal, body) = layout::notice_modal_rects(area, text);
    let wrapped = wrap_words(text, body.width as usize);

    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card(frame, modal, theme, Role::Elevated, Role::Warning, true);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    frame.buffer_mut().set_stringn(
        modal.x + 2,
        modal.y,
        format!(
            " {} {} ",
            tokens::glyph(theme, Glyph::Warning),
            i.text("ui.notice")
        ),
        modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Warning)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let sel_rows: Vec<SelectableRow> = wrapped
        .iter()
        .take(body.height as usize)
        .map(|row| SelectableRow {
            text: row.clone(),
            soft_wrap: false,
        })
        .collect();
    let lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .map(|row| Line::from(Span::styled(row, tokens::text(theme, Level::Primary))))
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane: TextPane::Notice,
        bounds: body,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == TextPane::Notice
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }
    let hint_y = modal.bottom().saturating_sub(2);
    let hint_w = modal.width.saturating_sub(4);
    let hints = tokens::fit_keycaps(
        theme,
        &[
            ("c", i.text("key.copy")),
            ("V", i.text("key.plain_view")),
            ("↵ / Esc", i.text("key.close")),
        ],
        hint_w as usize,
    );
    frame.render_widget(
        Paragraph::new(Line::from(hints)),
        Rect::new(modal.x + 2, hint_y, hint_w, 1),
    );
    fx::apply_fade(frame.buffer_mut(), modal, theme.palette.canvas, alpha);
}

enum HelpRow<'a> {
    Section(&'a str),
    Item(&'a str, &'a str),
}

struct HelpLayout {
    modal: Rect,
    body: Rect,
    rule_y: Option<u16>,
    hint_y: u16,
    lines: Vec<Line<'static>>,
}

fn sanitize_help_key(key: &str, ascii: bool) -> String {
    if !ascii {
        return key.to_owned();
    }
    key.replace("↑↓←→", "Arrows")
        .replace("↑↓", "Up/Dn")
        .replace("←→", "Lt/Rt")
        .replace('↑', "Up")
        .replace('↓', "Dn")
        .replace('←', "Lt")
        .replace('→', "Rt")
        .replace('↵', "Enter")
        .replace('·', "/")
        .replace('–', "-")
}

fn sanitize_help_desc(desc: &str, ascii: bool) -> String {
    if !ascii {
        return desc.to_owned();
    }
    desc.replace('·', "/").replace('–', "-").replace('│', "|")
}

fn help_rows(app: &App) -> Vec<HelpRow<'_>> {
    let i = &app.ctx.i18n;
    if app.is_docker_view() {
        vec![
            HelpRow::Section(i.text("docker.help.sec_nav")),
            HelpRow::Item("←→ / 1-6", i.text("docker.help.tabs")),
            HelpRow::Item("↑↓ / j k / Tab", i.text("docker.help.list")),
            HelpRow::Item("/ / Space / a / o", i.text("docker.help.filter_sort")),
            HelpRow::Section(i.text("docker.help.sec_ops")),
            HelpRow::Item("Enter / s / u / p", i.text("docker.help.ops")),
            HelpRow::Item("e / Shift+E", i.text("docker.help.exec")),
            HelpRow::Item("d / Shift+D", i.text("docker.help.remove")),
            HelpRow::Item("Shift+P / c / g", i.text("docker.help.manage")),
            HelpRow::Section(i.text("docker.help.sec_inspect")),
            HelpRow::Item("i / l / m / v", i.text("docker.help.inspect")),
            HelpRow::Item("PgUp / PgDn", i.text("docker.help.scroll")),
            HelpRow::Item("Shift+J / Shift+S", i.text("docker.help.inspect_extra")),
            HelpRow::Item("f / t / w / c", i.text("docker.help.logs")),
            HelpRow::Item("y / Shift+V", i.text("docker.help.copy_plain")),
            HelpRow::Section(i.text("docker.help.sec_global")),
            HelpRow::Item("z / [ / r", i.text("docker.help.focus")),
            HelpRow::Item("Ctrl+K / Shift+T·L", i.text("docker.help.global")),
            HelpRow::Item("Shift+G / F2", i.text("docker.help.shell_logs")),
            HelpRow::Item("Esc / q / Ctrl+C", i.text("docker.help.exit")),
        ]
    } else if app.is_clash_view() {
        vec![
            HelpRow::Section(i.text("clash.help.sec_nav")),
            HelpRow::Item("Alt+1-6 / { }", i.text("clash.help.tabs")),
            HelpRow::Item("Tab / 1-9", i.text("clash.help.sidebar")),
            HelpRow::Item("↑↓←→ / j k", i.text("clash.help.move")),
            HelpRow::Item("/ / Esc", i.text("clash.help.filter")),
            HelpRow::Section(i.text("clash.help.sec_proxies")),
            HelpRow::Item(", . / g", i.text("clash.help.groups")),
            HelpRow::Item("Enter / f / x", i.text("clash.help.select")),
            HelpRow::Item("t / p / d", i.text("clash.help.delay")),
            HelpRow::Item("o / O / h / c", i.text("clash.help.sort")),
            HelpRow::Section(i.text("clash.help.sec_manage")),
            HelpRow::Item("v / g t y / Space", i.text("clash.help.traffic")),
            HelpRow::Item("m / p / s R / i c N", i.text("clash.help.overview")),
            HelpRow::Item("u / U / a / i / d", i.text("clash.help.profiles")),
            HelpRow::Item("a / , . f / x X", i.text("clash.help.conns")),
            HelpRow::Item("P / l,y / Shift+S", i.text("clash.help.tools")),
            HelpRow::Section(i.text("docker.help.sec_global")),
            HelpRow::Item("z / [ / r", i.text("docker.help.focus")),
            HelpRow::Item("Ctrl+K / Shift+T·L", i.text("docker.help.global")),
            HelpRow::Item("Shift+G / F2", i.text("docker.help.shell_logs")),
            HelpRow::Item("Esc / q / Ctrl+C", i.text("docker.help.exit")),
        ]
    } else {
        vec![
            HelpRow::Item("↑↓ / j k", i.text("key.move")),
            HelpRow::Item("←→ / PgUp·PgDn", i.text("key.section")),
            HelpRow::Item("Tab / 1-9 / H / O", i.text("key.pan")),
            HelpRow::Item("[", i.text("cmd.sidebar")),
            HelpRow::Item("/ / Ctrl+K", i.text("help.palette")),
            HelpRow::Item("Enter", i.text("key.open")),
            HelpRow::Item("Space / X / A", i.text("group.optimization")),
            HelpRow::Item("C / Y", i.text("help.copy")),
            HelpRow::Item("V", i.text("help.plain_view")),
            HelpRow::Item("Alt+↑ / Alt+↓", i.text("key.reorder")),
            HelpRow::Item("T  (B / M)", i.text("help.style")),
            HelpRow::Item("P", i.text("cmd.github_proxy")),
            HelpRow::Item("Shift+S", i.text("cmd.socks_secret")),
            HelpRow::Item("G / F2", i.text("key.full_log")),
            HelpRow::Item("R / N", i.text("key.public")),
            HelpRow::Item("L", i.text("key.language")),
            HelpRow::Item("Esc / Q / Ctrl+C", i.text("key.quit")),
        ]
    }
}

fn build_help_lines(
    app: &App,
    rows: &[HelpRow<'_>],
    inner_w: usize,
    section_gaps: bool,
) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let prefix = if inner_w < 44 { "" } else { " " };
    let gap = if inner_w < 58 { " " } else { "  " };
    let natural_key_w = rows
        .iter()
        .filter_map(|row| match row {
            HelpRow::Item(k, _) => Some(sanitize_help_key(k, ascii).width()),
            HelpRow::Section(_) => None,
        })
        .max()
        .unwrap_or(12);
    let max_key_w = if inner_w < 44 {
        14usize
    } else {
        (inner_w * 36 / 100).max(18)
    };
    let key_w = natural_key_w.min(max_key_w);
    let key_style = tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD);
    let desc_style = tokens::text(theme, Level::Primary);
    let sec_style = tokens::text(theme, Level::Tertiary).add_modifier(Modifier::BOLD);
    let rule_style = tokens::fg(theme, Role::Rule);
    let rule_sym = tokens::glyph(theme, Glyph::Rule);

    let mut lines = Vec::new();
    for row in rows {
        match row {
            HelpRow::Section(title) => {
                if section_gaps && !lines.is_empty() {
                    lines.push(Line::default());
                }
                let title_clean = sanitize_help_desc(title, ascii);
                let title_clip =
                    clip_ascii_safe(&title_clean, inner_w.saturating_sub(prefix.len()), ascii);
                let used = prefix.len() + title_clip.width();
                let tail_pad = usize::from(inner_w >= 44);
                let rule_len = inner_w.saturating_sub(used + 1 + tail_pad);
                let mut spans = vec![Span::styled(format!("{prefix}{title_clip}"), sec_style)];
                if rule_len >= 2 {
                    spans.push(Span::styled(
                        format!(" {}", rule_sym.repeat(rule_len)),
                        rule_style,
                    ));
                }
                lines.push(Line::from(spans));
            }
            HelpRow::Item(k, desc) => {
                let k_text = sanitize_help_key(k, ascii);
                let d_text = sanitize_help_desc(desc, ascii);
                let k_len = k_text.width();
                // Stack long keys onto their own line on ultra-narrow viewports instead of clipping.
                if k_len > key_w || inner_w < prefix.len() + key_w + gap.len() + 12 {
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{k_text}"),
                        key_style,
                    )));
                    let indent = format!("{prefix}  ");
                    let wrap_w = inner_w.saturating_sub(indent.len()).max(8);
                    for part in wrap_words(&d_text, wrap_w) {
                        lines.push(Line::from(vec![
                            Span::raw(indent.clone()),
                            Span::styled(part, desc_style),
                        ]));
                    }
                } else {
                    let pad = " ".repeat(key_w.saturating_sub(k_len));
                    let desc_budget = inner_w
                        .saturating_sub(prefix.len() + key_w + gap.len())
                        .max(8);
                    let wrapped = wrap_words(&d_text, desc_budget);
                    let cont_indent = " ".repeat(prefix.len() + key_w + gap.len());
                    for (idx, part) in wrapped.into_iter().enumerate() {
                        if idx == 0 {
                            lines.push(Line::from(vec![
                                Span::styled(format!("{prefix}{k_text}{pad}{gap}"), key_style),
                                Span::styled(part, desc_style),
                            ]));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw(cont_indent.clone()),
                                Span::styled(part, desc_style),
                            ]));
                        }
                    }
                }
            }
        }
    }
    lines
}

fn help_layout(app: &App, area: Rect) -> HelpLayout {
    let rows = help_rows(app);
    let max_modal_h = area.height.saturating_sub(2);
    let probe = layout::centered(area, 86, max_modal_h.max(9));
    let inner_w = probe.width.saturating_sub(4).max(1) as usize;
    let chrome = if max_modal_h >= 9 { 5u16 } else { 4u16 };
    let max_body_h = max_modal_h.saturating_sub(chrome) as usize;

    // Include relaxed blank lines between sections when everything still fits on screen.
    let relaxed = build_help_lines(app, &rows, inner_w, true);
    let lines = if relaxed.len() <= max_body_h {
        relaxed
    } else {
        build_help_lines(app, &rows, inner_w, false)
    };

    let desired_h = (lines.len() as u16).saturating_add(chrome);
    let modal_h = desired_h.clamp(9.min(max_modal_h), max_modal_h.max(1));
    let modal = layout::centered(area, 86, modal_h);
    let rule_y = (modal.height >= 9).then(|| modal.bottom().saturating_sub(3));
    let body_h = modal
        .height
        .saturating_sub(if rule_y.is_some() { 5 } else { 4 });
    let body = Rect::new(
        modal.x + 2,
        modal.y + 2,
        modal.width.saturating_sub(4),
        body_h,
    );
    let hint_y = modal.bottom().saturating_sub(2);

    HelpLayout {
        modal,
        body,
        rule_y,
        hint_y,
        lines,
    }
}

pub(in crate::tui) fn help_max_scroll(app: &App, area: Rect) -> u16 {
    let geom = help_layout(app, area);
    geom.lines.len().saturating_sub(geom.body.height as usize) as u16
}

pub(super) fn draw_help(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let ascii = theme.flags.ascii;
    let geom = help_layout(app, area);
    let modal = geom.modal;

    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card(frame, modal, theme, Role::Elevated, Role::BorderFocus, true);
    let bg = tokens::color(theme, Role::Elevated);
    let title_key = if app.is_docker_view() {
        "docker.help.title"
    } else if app.is_clash_view() {
        "clash.help.title"
    } else {
        "help.title"
    };
    let title = format!(" {} ", i.text(title_key));
    frame.buffer_mut().set_stringn(
        modal.x + 2,
        modal.y,
        clip_ascii_safe(&title, modal.width.saturating_sub(4) as usize, ascii),
        modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let total_lines = geom.lines.len();
    let max_scroll = total_lines.saturating_sub(geom.body.height as usize) as u16;
    let scroll = app.help_scroll.min(max_scroll);
    frame.render_widget(Paragraph::new(geom.lines).scroll((scroll, 0)), geom.body);

    if let Some(rule_y) = geom.rule_y {
        tokens::draw_rule(
            frame.buffer_mut(),
            modal.x + 2,
            rule_y,
            modal.width.saturating_sub(4),
            theme,
            bg,
        );
    }

    let hint_w = modal.width.saturating_sub(4);
    let hint_pairs: Vec<(&str, &str)> = if max_scroll > 0 {
        vec![
            ("↑↓", i.text("key.scroll")),
            ("↵ / Esc / ?", i.text("key.close")),
        ]
    } else {
        vec![("↵ / Esc / ?", i.text("key.close"))]
    };
    if max_scroll > 0 {
        let first = scroll as usize + 1;
        let last = (scroll as usize + geom.body.height as usize).min(total_lines);
        let dash = if ascii { "-" } else { "–" };
        let pos = format!("{first}{dash}{last} / {total_lines}");
        let pos_w = pos.width() as u16;
        let hint_budget = hint_w.saturating_sub(pos_w + 2);
        let hints = tokens::fit_keycaps(theme, &hint_pairs, hint_budget as usize);
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(modal.x + 2, geom.hint_y, hint_budget, 1),
        );
        tokens::put_right(
            frame.buffer_mut(),
            Rect::new(modal.x + 2, geom.hint_y, hint_w, 1),
            &pos,
            tokens::text(theme, Level::Tertiary).bg(bg),
        );
    } else {
        let hints = tokens::fit_keycaps(theme, &hint_pairs, hint_w as usize);
        frame.render_widget(
            Paragraph::new(Line::from(hints)),
            Rect::new(modal.x + 2, geom.hint_y, hint_w, 1),
        );
    }

    fx::apply_fade(frame.buffer_mut(), modal, theme.palette.canvas, alpha);
}

fn draw_button(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    label: &str,
    focused: bool,
    primary: bool,
    base_bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    if rect.height >= 2 {
        let kind = if focused {
            tokens::TouchBtnKind::Primary
        } else if primary {
            tokens::TouchBtnKind::Active
        } else {
            tokens::TouchBtnKind::Secondary
        };
        tokens::draw_touch_button(buf, rect, label, None, kind, hovered, theme, base_bg);
        return;
    }
    let (fg_col, bg_col) = if focused {
        (
            tokens::color(theme, Role::OnAccent),
            tokens::color(theme, if primary { Role::Accent } else { Role::Text2 }),
        )
    } else if hovered {
        (
            tokens::color(theme, Role::Text1),
            tokens::color(theme, Role::Selection),
        )
    } else {
        (
            tokens::color(theme, if primary { Role::Accent } else { Role::Text2 }),
            if theme.flags.paint_bg {
                tokens::color(theme, Role::SurfaceAlt)
            } else {
                base_bg
            },
        )
    };
    let text = if theme.flags.ascii {
        format!("[ {label} ]")
    } else {
        format!("  {label}  ")
    };
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&text, rect.width as usize, theme.flags.ascii),
        rect.width as usize,
        Style::default()
            .fg(fg_col)
            .bg(bg_col)
            .add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );
}

fn draw_socks_live_preview(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    params: &crate::actions::Parameters,
    modal_bg: Color,
) {
    if rect.width < 18 || rect.height < 6 {
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;

    let card_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Sidebar)
    } else {
        modal_bg
    };
    let header_bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::SurfaceAlt)
    } else {
        card_bg
    };
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            buf[(x, y)].set_symbol(" ").set_bg(card_bg);
        }
    }
    for x in rect.left()..rect.right() {
        buf[(x, rect.y)].set_symbol(" ").set_bg(header_bg);
    }

    let inner_x = rect.x + 1;
    let inner_w = rect.width.saturating_sub(2) as usize;
    let bottom = rect.bottom();

    let header = format!(
        "{} {}",
        if ascii { "*" } else { "◆" },
        i.text("ui.socks_preview_title")
    );
    buf.set_stringn(
        inner_x,
        rect.y,
        clip_ascii_safe(&header, inner_w, ascii),
        inner_w,
        tokens::fg(theme, Role::Accent)
            .bg(header_bg)
            .add_modifier(Modifier::BOLD),
    );

    let mut y = rect.y + 2;
    if y >= bottom {
        return;
    }

    let enabled = params.get("socks_enabled").map(String::as_str) != Some("off");
    if !enabled {
        let msg = format!(
            "{} {}",
            if ascii { "o" } else { "○" },
            i.text("ui.socks_preview_disabled")
        );
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&msg, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Secondary).bg(card_bg),
        );
        return;
    }

    let port_raw = params.get("socks_port").map_or("10808", String::as_str);
    let listen_raw = params.get("socks_listen").map_or("0.0.0.0", String::as_str);
    let udp = params.get("socks_udp").map(String::as_str) != Some("off");
    let outbound = params.get("socks_out").map_or("direct", String::as_str);
    let auth = params.get("socks_auth").map_or("auto", String::as_str);
    let user = params
        .get("socks_user")
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("cqs");

    let port_res = crate::ops::clash::socks::validate_port(port_raw);
    let listen_res = crate::ops::clash::socks::normalize_listen(listen_raw);

    let (endpoint, host, is_loopback, addr_valid) = match (&port_res, &listen_res) {
        (Ok(port), Ok((norm_listen, ip))) => {
            let bracketed = crate::ops::clash::socks::format_host_bracketed(norm_listen);
            let public_v4 = match &app.snapshot.public.ipv4 {
                crate::dashboard::Reading::Value(v) => Some(v.as_str()),
                _ => None,
            };
            let target_host = if ip.is_unspecified() {
                public_v4
                    .map(crate::ops::clash::socks::format_host_bracketed)
                    .unwrap_or_else(|| "<SERVER_IP>".into())
            } else {
                bracketed.clone()
            };
            (
                format!("{bracketed}:{port}"),
                format!("{target_host}:{port}"),
                crate::ops::clash::socks::is_loopback_ip(ip),
                true,
            )
        }
        _ => (
            format!("{listen_raw}:{port_raw}"),
            format!("<SERVER_IP>:{port_raw}"),
            false,
            false,
        ),
    };

    // 1. Traffic topology line
    let arrow = if ascii { "->" } else { "──▶" };
    let proto = if udp { "TCP+UDP" } else { "TCP" };
    let out_label = if outbound == "direct" {
        i.text("ui.socks_preview_out_direct")
    } else {
        i.text("ui.socks_preview_out_rule")
    };
    buf.set_stringn(
        inner_x,
        y,
        clip_ascii_safe(i.text("ui.socks_preview_flow"), inner_w, ascii),
        inner_w,
        tokens::text(theme, Level::Tertiary).bg(card_bg),
    );
    y += 1;
    if y < bottom {
        let flow = format!("{endpoint} ({proto}) {arrow} {out_label}");
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&flow, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Primary)
                .bg(card_bg)
                .add_modifier(Modifier::BOLD),
        );
        y += 1;
    }

    // 2. Security & authentication status badge
    if y < bottom {
        let (badge_text, badge_role) = if !addr_valid {
            (
                format!(
                    "{} {}",
                    if ascii { "!" } else { "✗" },
                    i.text("ui.socks_preview_bad_addr")
                ),
                Role::Danger,
            )
        } else if auth == "off" && !is_loopback {
            (
                format!(
                    "{} {}",
                    if ascii { "!" } else { "✗" },
                    i.text("ui.socks_preview_auth_err")
                ),
                Role::Danger,
            )
        } else if auth == "off" {
            (
                format!(
                    "{} {}",
                    if ascii { "*" } else { "●" },
                    i.text("ui.socks_preview_auth_loopback")
                ),
                Role::Info,
            )
        } else {
            (
                format!(
                    "{} {}",
                    if ascii { "+" } else { "✓" },
                    i.format("ui.socks_preview_auth_ok", &[("user", user)])
                ),
                Role::Success,
            )
        };
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&badge_text, inner_w, ascii),
            inner_w,
            tokens::fg(theme, badge_role)
                .bg(card_bg)
                .add_modifier(Modifier::BOLD),
        );
        y += 2;
    }

    let user_valid = crate::ops::clash::socks::validate_username(user).is_ok();
    let safe_user = if user_valid { user } else { "<user>" };
    let cred_prefix = if auth == "off" {
        String::new()
    } else if auth == "keep" {
        format!("{safe_user}:<saved>@")
    } else {
        format!("{safe_user}:<auto>@")
    };
    let socks5h_uri = format!("socks5h://{cred_prefix}{host}");
    let socks5_uri = format!("socks5://{cred_prefix}{host}");

    // 3. SOCKS5h URI (remote DNS)
    if y + 1 < bottom {
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(i.text("ui.socks_preview_socks5h"), inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Tertiary).bg(card_bg),
        );
        y += 1;
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&socks5h_uri, inner_w, ascii),
            inner_w,
            tokens::fg(theme, Role::Accent)
                .bg(card_bg)
                .add_modifier(Modifier::BOLD),
        );
        y += 2;
    }

    // 4. SOCKS5 URI (client local DNS)
    if y + 1 < bottom {
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(i.text("ui.socks_preview_socks5"), inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Tertiary).bg(card_bg),
        );
        y += 1;
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&socks5_uri, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Primary).bg(card_bg),
        );
        y += 2;
    }

    // 5. Quick curl test & 0600 credential card path
    if y + 1 < bottom {
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(i.text("ui.socks_preview_curl"), inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Tertiary).bg(card_bg),
        );
        y += 1;
        let curl_cmd = format!("curl -x '{socks5h_uri}' https://ifconfig.me");
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&curl_cmd, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Secondary).bg(card_bg),
        );
        y += 2;
    }

    if auth != "off" && y + 1 < bottom {
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(i.text("ui.socks_preview_cred_file"), inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Tertiary).bg(card_bg),
        );
        y += 1;
        let cred_path = crate::ops::clash::socks::auth_path(&app.ctx)
            .display()
            .to_string();
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&cred_path, inner_w, ascii),
            inner_w,
            tokens::text(theme, Level::Secondary).bg(card_bg),
        );
        y += 2;
    }

    if y < bottom && app.cached_socks_config().is_some_and(|c| c.enabled) {
        let hint = format!(
            "{} {}",
            if ascii { ">" } else { "▸" },
            i.text("ui.socks_preview_secret_hint")
        );
        buf.set_stringn(
            inner_x,
            y,
            clip_ascii_safe(&hint, inner_w, ascii),
            inner_w,
            tokens::fg(theme, Role::Info).bg(card_bg),
        );
    }
}

fn build_socks_secret_lines(
    app: &App,
    modal: &super::super::state::SocksSecretModal,
    inner_w: usize,
) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;
    let creds = &modal.creds;
    let inner_w = inner_w.max(1);

    let warn_style = tokens::fg(theme, Role::Warning).add_modifier(Modifier::BOLD);
    let label_style = tokens::text(theme, Level::Tertiary);
    let primary_style = tokens::text(theme, Level::Primary);
    let primary_bold = primary_style.add_modifier(Modifier::BOLD);
    let accent_bold = tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD);
    let muted_style = tokens::text(theme, Level::Secondary);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in wrap_words(i.text("ui.socks_secret_notice"), inner_w) {
        lines.push(Line::from(Span::styled(row, warn_style)));
    }
    lines.push(Line::default());

    let sep = if ascii { " | " } else { " · " };
    let endpoint = format!(
        "{}:{}",
        crate::ops::clash::socks::format_host_bracketed(&creds.listen),
        creds.port
    );
    let proto = if creds.udp { "TCP+UDP" } else { "TCP" };
    let mode = if creds.outbound == "direct" {
        "DIRECT"
    } else {
        "RULE"
    };
    let listen_summary = format!("{endpoint}{sep}{proto}{sep}{mode}");
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", i.text("ui.socks_secret_listen")),
            label_style,
        ),
        Span::styled(
            clip_ascii_safe(
                &listen_summary,
                inner_w.saturating_sub(i.text("ui.socks_secret_listen").width() + 2),
                ascii,
            ),
            primary_bold,
        ),
    ]));

    // Never display the raw SOCKS5 password on screen; always render a fixed-width mask.
    if creds.has_auth {
        let user = creds.username.as_deref().unwrap_or("cqs");
        let masked_pass = "************************";
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", i.text("ui.socks_secret_user")), label_style),
            Span::styled(user.to_owned(), primary_bold),
        ]));
        let pass_prefix = format!("{}: ", i.text("ui.socks_secret_pass"));
        if pass_prefix.width() + masked_pass.width() <= inner_w {
            lines.push(Line::from(vec![
                Span::styled(pass_prefix, label_style),
                Span::styled(masked_pass.to_owned(), muted_style),
            ]));
        } else {
            lines.push(Line::from(Span::styled(pass_prefix, label_style)));
            for chunk in wrap_cells(masked_pass, inner_w) {
                lines.push(Line::from(Span::styled(chunk, muted_style)));
            }
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", i.text("ui.socks_secret_auth")), label_style),
            Span::styled(
                i.text("ui.socks_preview_auth_loopback").to_owned(),
                tokens::fg(theme, Role::Info),
            ),
        ]));
    }

    let uri_h = creds.masked_uri("socks5h");
    let uri_5 = creds.masked_uri("socks5");
    let curl = creds.masked_curl_cmd();

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        i.text("ui.socks_secret_socks5h").to_owned(),
        label_style,
    )));
    for chunk in wrap_cells(&uri_h, inner_w) {
        lines.push(Line::from(Span::styled(chunk, accent_bold)));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        i.text("ui.socks_secret_socks5").to_owned(),
        label_style,
    )));
    for chunk in wrap_cells(&uri_5, inner_w) {
        lines.push(Line::from(Span::styled(chunk, primary_style)));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        i.text("ui.socks_secret_curl").to_owned(),
        label_style,
    )));
    for chunk in wrap_cells(&curl, inner_w) {
        lines.push(Line::from(Span::styled(chunk, muted_style)));
    }

    if creds.has_auth {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            i.text("ui.socks_secret_file").to_owned(),
            label_style,
        )));
        for chunk in wrap_cells(&creds.auth_file.display().to_string(), inner_w) {
            lines.push(Line::from(Span::styled(chunk, label_style)));
        }
    }

    lines
}

pub(in crate::tui) fn socks_secret_max_scroll(app: &App, area: Rect) -> u16 {
    let Some(modal) = &app.socks_secret else {
        return 0;
    };
    let geom = layout::SocksSecretGeometry::compute(
        area,
        &modal.creds,
        &app.ctx.i18n,
        app.theme.active.flags.ascii,
        app.resolved_density(area),
    );
    let lines = build_socks_secret_lines(app, modal, geom.body.width as usize);
    lines.len().saturating_sub(geom.body.height as usize) as u16
}

fn draw_secret_chip(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    key: &str,
    label: &str,
    primary: bool,
    base_bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    if rect.height >= 2 {
        let lbl = format!("[{key}] {label}");
        let kind = if primary {
            tokens::TouchBtnKind::Primary
        } else {
            tokens::TouchBtnKind::Secondary
        };
        tokens::draw_touch_button(buf, rect, &lbl, None, kind, hovered, theme, base_bg);
        return;
    }
    let (fg_col, bg_col) = if hovered {
        (
            tokens::color(theme, Role::OnAccent),
            tokens::color(theme, Role::Accent),
        )
    } else if primary {
        (
            tokens::color(theme, Role::Accent),
            if theme.flags.paint_bg {
                tokens::color(theme, Role::SurfaceAlt)
            } else {
                base_bg
            },
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
    let text = if ascii {
        format!("[{key} {label}]")
    } else {
        format!(" {key} {label} ")
    };
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&text, rect.width as usize, ascii),
        rect.width as usize,
        Style::default()
            .fg(fg_col)
            .bg(bg_col)
            .add_modifier(if primary || hovered {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    );
}

pub(super) fn draw_socks_secret(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let Some(modal_state) = &app.socks_secret else {
        return;
    };
    let theme = &app.theme.active;
    let ascii = theme.flags.ascii;
    let i = &app.ctx.i18n;
    let geom = layout::SocksSecretGeometry::compute(
        area,
        &modal_state.creds,
        i,
        ascii,
        app.resolved_density(area),
    );
    let modal = geom.modal;

    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card(frame, modal, theme, Role::Elevated, Role::Warning, true);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };

    let title = format!(
        " {} {} ",
        tokens::glyph(theme, Glyph::Warning),
        i.text("ui.socks_secret_title")
    );
    frame.buffer_mut().set_stringn(
        modal.x + 2,
        modal.y,
        clip_ascii_safe(&title, modal.width.saturating_sub(4) as usize, ascii),
        modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Warning)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let lines = build_socks_secret_lines(app, modal_state, geom.body.width as usize);
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(geom.body.height as usize) as u16;
    let scroll = modal_state.scroll.min(max_scroll);

    if max_scroll > 0 {
        let first = scroll as usize + 1;
        let last = (scroll as usize + geom.body.height as usize).min(total_lines);
        let pos = format!(" {first}-{last}/{total_lines} ");
        let pos_w = pos.width() as u16;
        if modal.width > pos_w + 8 {
            frame.buffer_mut().set_stringn(
                modal.right().saturating_sub(pos_w + 2),
                modal.y,
                &pos,
                pos_w as usize,
                tokens::text(theme, Level::Tertiary).bg(bg),
            );
        }
    }

    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), geom.body);

    if let Some(rule_y) = geom.rule_y {
        tokens::draw_rule(
            frame.buffer_mut(),
            modal.x + 2,
            rule_y,
            modal.width.saturating_sub(4),
            theme,
            bg,
        );
    }

    let buf = frame.buffer_mut();
    draw_secret_chip(app, buf, geom.btn_socks5h, "1", "SOCKS5h", true, bg);
    draw_secret_chip(app, buf, geom.btn_socks5, "2", "SOCKS5", false, bg);
    draw_secret_chip(
        app,
        buf,
        geom.btn_password,
        "3",
        i.text("ui.socks_secret_btn_pass"),
        false,
        bg,
    );
    draw_secret_chip(app, buf, geom.btn_curl, "4", "curl", false, bg);
    draw_secret_chip(
        app,
        buf,
        geom.btn_close,
        "Esc",
        i.text("key.close"),
        false,
        bg,
    );

    fx::apply_fade(frame.buffer_mut(), modal, theme.palette.canvas, alpha);
}
