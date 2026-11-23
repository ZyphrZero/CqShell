//! Bottom expandable execution log drawer (`G`) and full-screen log viewer modal (`F2`).

use super::{
    clip_ascii_safe,
    tokens::{self},
    wrap_cells,
};
use crate::{
    theme::{Level, Role},
    tui::{
        fx, layout,
        selection::{self, SelectablePane, SelectableRow, TextPane},
        state::{App, format_mm_ss},
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

pub(in crate::tui) fn wrapped_line_count(app: &App, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    app.logs
        .iter()
        .map(|line| wrap_cells(&line.text, width).len().max(1))
        .sum()
}

pub(super) fn draw_drawer(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    body: Rect,
    copy_btn: Option<Rect>,
    plain_btn: Option<Rect>,
    expand_btn: Option<Rect>,
) {
    if area.height < 3 || area.width < 12 {
        return;
    }
    let theme = &app.theme.active;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    tokens::draw_surface_card_fx(
        frame,
        area,
        theme,
        Role::Sidebar,
        Role::Border,
        false,
        elapsed,
        reduced,
    );
    let bg = tokens::card_title_bg(theme, Role::Sidebar);
    draw_log_header(app, frame, area, bg, false, copy_btn, plain_btn, expand_btn);
    draw_log_lines(app, frame, body, TextPane::LogDrawer);
}

pub(super) fn draw_modal(app: &App, frame: &mut Frame, area: Rect, alpha: f64) {
    let theme = &app.theme.active;
    let elapsed = app.now.saturating_duration_since(app.started);
    let reduced = app.reduced_motion();
    let (modal, body) = layout::full_log_modal(area);
    let btns = layout::full_log_modal_buttons(modal, app);
    fx::apply_shadow(frame.buffer_mut(), modal, area, theme);
    tokens::draw_surface_card_fx(
        frame,
        modal,
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
    draw_log_header(app, frame, modal, bg, true, None, None, None);
    draw_log_lines(app, frame, body, TextPane::LogModal);

    // Bottom modal hints (left) + clickable Copy Logs & Borderless Plain View pills (right)
    let i = &app.ctx.i18n;
    let hint_y = modal.bottom().saturating_sub(2);
    if hint_y > body.y {
        let left_w = btns.copy.x.saturating_sub(modal.x + 3);
        if left_w >= 8 {
            let hints = tokens::fit_keycaps(
                theme,
                &[
                    ("↑↓/PgUp/PgDn", i.text("key.scroll")),
                    ("Esc/F2", i.text("key.close")),
                ],
                left_w as usize,
            );
            frame.render_widget(
                Paragraph::new(Line::from(hints)),
                Rect::new(modal.x + 2, hint_y, left_w, 1),
            );
        }
        let buf = frame.buffer_mut();
        draw_log_action_pill(app, buf, btns.copy, "c", i.text("key.copy_logs"), false, bg);
        draw_log_action_pill(
            app,
            buf,
            btns.plain,
            "V",
            i.text("key.plain_view"),
            false,
            bg,
        );
    }

    fx::apply_fade(frame.buffer_mut(), modal, theme.palette.canvas, alpha);
}

#[allow(clippy::too_many_arguments)]
fn draw_log_header(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    bg: Color,
    modal: bool,
    copy_btn: Option<Rect>,
    select_btn: Option<Rect>,
    expand_btn: Option<Rect>,
) {
    let theme = &app.theme.active;
    let i = &app.ctx.i18n;
    let elapsed = app.now.saturating_duration_since(app.started);
    let phase_sym = tokens::phase_icon(theme, app.phase, elapsed);

    let title_text = if let Some(progress) = &app.progress
        && let Some(curr) = progress.current()
        && let Some(action_id) = app.task_ids.get(curr)
    {
        let dur = format_mm_ss(app.now.saturating_duration_since(progress.started));
        i.format(
            "ui.log_step_title",
            &[
                ("index", &(curr + 1).to_string()),
                ("total", &progress.steps.len().to_string()),
                ("action", i.text(&format!("action.{action_id}"))),
                ("elapsed", &dur),
            ],
        )
    } else if !app.task_ids.is_empty() {
        let sep = tokens::glyph(theme, tokens::Glyph::Separator);
        let dur = app
            .single_elapsed()
            .map(|d| format!(" {sep} {}", format_mm_ss(d)))
            .unwrap_or_default();
        format!("{} {sep} {}{dur}", i.text("ui.logs"), app.task_title())
    } else {
        i.text("ui.logs").to_owned()
    };

    let full_title = format!(" {phase_sym} {title_text} ");
    let count_hint = format!(" {} ", app.logs.len());
    let y = area.y;
    let buf = frame.buffer_mut();

    let right_start = if !modal {
        copy_btn
            .or(select_btn)
            .or(expand_btn)
            .map(|r| r.x)
            .unwrap_or_else(|| area.right().saturating_sub(count_hint.width() as u16 + 2))
    } else {
        area.right().saturating_sub(count_hint.width() as u16 + 2)
    };
    let max_title = right_start.saturating_sub(area.x + 3) as usize;
    buf.set_stringn(
        area.x + 2,
        y,
        clip_ascii_safe(&full_title, max_title, theme.flags.ascii),
        max_title,
        tokens::tone_style(theme, app.phase.tone())
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    tokens::put_right(
        buf,
        Rect::new(area.x + 2, y, area.width.saturating_sub(4), 1),
        &count_hint,
        tokens::text(theme, Level::Tertiary).bg(bg),
    );

    if !modal {
        if let Some(r) = copy_btn {
            draw_log_action_pill(app, buf, r, "y", i.text("key.copy"), false, bg);
        }
        if let Some(r) = select_btn {
            draw_log_action_pill(app, buf, r, "V", i.text("key.plain_view"), false, bg);
        }
        if let Some(r) = expand_btn {
            draw_log_action_pill(app, buf, r, "F2", "", false, bg);
        }
    }
}

fn draw_log_action_pill(
    app: &App,
    buf: &mut ratatui::buffer::Buffer,
    rect: Rect,
    key: &str,
    label: &str,
    active: bool,
    base_bg: Color,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let theme = &app.theme.active;
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    let text = if theme.flags.ascii {
        if label.is_empty() {
            format!("[{key}]")
        } else {
            format!("[{key} {label}]")
        }
    } else if label.is_empty() {
        format!(" {key} ")
    } else {
        format!(" {key} {label} ")
    };

    if theme.flags.ascii {
        let mut style = tokens::text(theme, Level::Secondary);
        if active {
            style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        buf.set_stringn(
            rect.x,
            rect.y,
            clip_ascii_safe(&text, rect.width as usize, true),
            rect.width as usize,
            style,
        );
        return;
    }

    let (fg_col, bg_col) = if active {
        (
            tokens::color(theme, Role::OnAccent),
            tokens::color(theme, Role::Warning),
        )
    } else if hovered {
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
    let key_prefix = format!(" {key} ");
    let key_fg = if active {
        fg_col
    } else {
        tokens::color(theme, Role::Accent)
    };
    buf.set_stringn(
        rect.x,
        rect.y,
        clip_ascii_safe(&key_prefix, rect.width as usize, false),
        rect.width as usize,
        ratatui::style::Style::default()
            .fg(key_fg)
            .bg(bg_col)
            .add_modifier(Modifier::BOLD),
    );
    let prefix_w = key_prefix.width() as u16;
    if !label.is_empty() && rect.width > prefix_w {
        let rest_w = rect.width.saturating_sub(prefix_w) as usize;
        buf.set_stringn(
            rect.x + prefix_w,
            rect.y,
            clip_ascii_safe(&format!("{label} "), rest_w, false),
            rest_w,
            ratatui::style::Style::default()
                .fg(fg_col)
                .bg(bg_col)
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
    }
}

fn draw_log_lines(app: &App, frame: &mut Frame, body: Rect, pane: TextPane) {
    if body.height == 0 || body.width == 0 {
        return;
    }
    let theme = &app.theme.active;
    if app.logs.is_empty() {
        let msg = app.ctx.i18n.text("ui.empty_log");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg.to_owned(),
                tokens::text(theme, Level::Tertiary),
            ))),
            body,
        );
        return;
    }

    let width = body.width as usize;
    let mut wrapped: Vec<(Line<'static>, SelectableRow)> = Vec::new();
    for entry in &app.logs {
        let mut style = tokens::tone_style(theme, entry.tone);
        if entry.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        for (idx, part) in wrap_cells(&entry.text, width).into_iter().enumerate() {
            let sel_row = SelectableRow {
                text: part.clone(),
                soft_wrap: idx > 0,
            };
            wrapped.push((Line::from(Span::styled(part, style)), sel_row));
        }
    }

    let height = body.height as usize;
    let max_scroll = wrapped.len().saturating_sub(height);
    let from_bottom = (app.log_scroll as usize).min(max_scroll);
    let top_index = max_scroll.saturating_sub(from_bottom);
    let mut slice = Vec::with_capacity(height);
    let mut sel_rows = Vec::with_capacity(height);
    for (line, sel_row) in wrapped.into_iter().skip(top_index).take(height) {
        slice.push(line);
        sel_rows.push(sel_row);
    }
    frame.render_widget(Paragraph::new(slice), body);
    app.selectable_panes.borrow_mut().push(SelectablePane {
        pane,
        bounds: body,
        rows: sel_rows,
    });
    if let Some(sel) = app.drag_select
        && sel.pane == pane
    {
        selection::highlight_selection(frame.buffer_mut(), &sel, theme);
    }
}
