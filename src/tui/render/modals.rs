//! Centered elevated modal dialogs with backdrop scrim, drop shadow, and smooth reveal:
//! Command Palette, Parameter Form (with Chips), Filterable Picker, Loading, Confirmation,
//! Notice, and Help.

use super::{
    clip_ascii_safe, highlight_spans,
    tokens::{self, Glyph},
    wrap_words,
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
    let specs = actions::specs();
    let results = palette::search(
        &pal.query.text,
        &specs,
        i,
        &app.category_order,
        &app.records,
    );
    let geom = CommandPaletteGeometry::compute(area, &results, pal.cursor);

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
        let list_h = geom.list.height as usize;
        let hi_style = Style::default()
            .fg(tokens::selected_accent(theme))
            .add_modifier(Modifier::BOLD);

        for (offset, row) in results
            .rows
            .iter()
            .skip(geom.scroll)
            .take(list_h)
            .enumerate()
        {
            let abs_idx = geom.scroll + offset;
            let y = geom.list.y + offset as u16;
            let row_rect = Rect::new(geom.list.x, y, geom.list.width, 1);

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

    // Bottom palette hints
    let hint_y = geom.modal.bottom().saturating_sub(2);
    if hint_y > geom.list.y {
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
    if hint_y > geom.buttons[0].y {
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
    let geom = PickerGeometry::compute(area, matches.len(), picker.cursor);

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

    for (offset, choice) in matches
        .iter()
        .skip(geom.scroll)
        .take(geom.list.height as usize)
        .enumerate()
    {
        let idx = geom.scroll + offset;
        let y = geom.list.y + offset as u16;
        let row_rect = Rect::new(geom.list.x, y, geom.list.width, 1);
        let active = idx == picker.cursor;
        let row_bg = if active {
            tokens::color(theme, Role::Selection)
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
                        .bg(row_bg),
                );
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
    let geom = ConfirmationGeometry::compute(area, confirm, i);

    fx::apply_shadow(frame.buffer_mut(), geom.modal, area, theme);
    let accent_role = if confirm.token == "YES" {
        Role::Warning
    } else {
        Role::Danger
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
    if confirm.token == "YES" {
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
    } else {
        let prompt_y = geom.modal.bottom().saturating_sub(4);
        let input_y = geom.modal.bottom().saturating_sub(3);
        let prompt = i.format("ui.type_confirm", &[("token", &confirm.token)]);
        buf.set_stringn(
            geom.modal.x + 2,
            prompt_y,
            clip_ascii_safe(
                &prompt,
                geom.modal.width.saturating_sub(4) as usize,
                theme.flags.ascii,
            ),
            geom.modal.width.saturating_sub(4) as usize,
            tokens::fg(theme, Role::Danger)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        let input_bg = if theme.flags.paint_bg {
            tokens::color(theme, Role::Surface)
        } else {
            bg
        };
        for col in (geom.modal.x + 2)..geom.modal.right().saturating_sub(2) {
            buf[(col, input_y)].set_symbol(" ").set_bg(input_bg);
        }
        let (vis, cx) = confirm
            .input
            .visible(geom.modal.width.saturating_sub(6) as usize);
        buf.set_stringn(
            geom.modal.x + 3,
            input_y,
            &vis,
            geom.modal.width.saturating_sub(6) as usize,
            tokens::text(theme, Level::Primary).bg(input_bg),
        );
        let cursor_col = (geom.modal.x + 3 + cx).min(geom.modal.right().saturating_sub(3));
        buf[(cursor_col, input_y)].set_style(
            Style::default()
                .fg(tokens::color(theme, Role::OnAccent))
                .bg(tokens::color(theme, Role::Danger)),
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

pub(super) fn draw_help(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let modal = layout::centered(area, 72, 22);
    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card(frame, modal, theme, Role::Elevated, Role::BorderFocus, true);
    let bg = if theme.flags.paint_bg {
        tokens::color(theme, Role::Elevated)
    } else {
        Color::Reset
    };
    frame.buffer_mut().set_stringn(
        modal.x + 2,
        modal.y,
        format!(" {} ", i.text("key.help")),
        modal.width.saturating_sub(4) as usize,
        tokens::fg(theme, Role::Accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let pairs: Vec<(&str, &str)> = if app.is_docker_view() {
        vec![
            ("<- / -> | 1-6", i.text("docker.help.tabs")),
            ("j k / Space / / / a / o", i.text("docker.help.list")),
            ("Enter / s / u / p / e / d / P", i.text("docker.help.ops")),
            ("i / l / m / v / J / Shift+S", i.text("docker.help.inspect")),
            ("f / t / w / c / Shift+M / y", i.text("docker.help.logs")),
            (
                "z / [ / Ctrl+K / Shift+T / Shift+L",
                i.text("docker.help.focus"),
            ),
            ("r / F2 / Shift+G", i.text("key.full_log")),
            ("Esc / q / Ctrl+C", i.text("key.quit")),
        ]
    } else {
        vec![
            ("↑↓ / j k", i.text("key.move")),
            ("←→ / PgUp·PgDn", i.text("key.section")),
            ("Tab / 1-9 / H / O", i.text("key.pan")),
            ("[", i.text("cmd.sidebar")),
            ("/ / Ctrl+K", i.text("help.palette")),
            ("Enter", i.text("key.open")),
            ("Space / X / A", i.text("group.optimization")),
            ("C / Y", i.text("help.copy")),
            ("V", i.text("help.plain_view")),
            ("Alt+↑ / Alt+↓", i.text("key.reorder")),
            ("T  (B / M)", i.text("help.style")),
            ("P", i.text("cmd.github_proxy")),
            ("G / F2", i.text("key.full_log")),
            ("R / N", i.text("key.public")),
            ("L", i.text("key.language")),
            ("Esc / Q / Ctrl+C", i.text("key.quit")),
        ]
    };

    let body = Rect::new(
        modal.x + 2,
        modal.y + 2,
        modal.width.saturating_sub(4),
        modal.height.saturating_sub(4),
    );
    let inner_w = body.width as usize;
    let key_w = if inner_w < 50 { 16usize } else { 22usize };
    let lines: Vec<Line<'static>> = pairs
        .iter()
        .map(|(k, desc)| {
            let k_clip = clip_ascii_safe(k, key_w, theme.flags.ascii);
            let k_pad = key_w.saturating_sub(k_clip.width());
            let prefix = if inner_w < 50 { " " } else { "  " };
            let desc_budget = inner_w.saturating_sub(prefix.len() + key_w + 1);
            Line::from(vec![
                Span::styled(
                    format!("{prefix}{k_clip}{} ", " ".repeat(k_pad)),
                    tokens::fg(theme, Role::Accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    clip_ascii_safe(desc, desc_budget, theme.flags.ascii),
                    tokens::text(theme, Level::Primary),
                ),
            ])
        })
        .collect();

    let max_scroll = lines.len().saturating_sub(body.height as usize) as u16;
    let scroll = app.help_scroll.min(max_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);

    let hint_y = modal.bottom().saturating_sub(2);
    let hint_w = modal.width.saturating_sub(4);
    let hint_pairs: Vec<(&str, &str)> = if max_scroll > 0 {
        vec![
            ("↑↓", i.text("key.scroll")),
            ("↵ / Esc / ?", i.text("key.close")),
        ]
    } else {
        vec![("↵ / Esc / ?", i.text("key.close"))]
    };
    let hints = tokens::fit_keycaps(theme, &hint_pairs, hint_w as usize);
    frame.render_widget(
        Paragraph::new(Line::from(hints)),
        Rect::new(modal.x + 2, hint_y, hint_w, 1),
    );

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
