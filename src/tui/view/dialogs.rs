//! Modal layers: parameter form, confirmation, error notice, system overview and key help,
//! plus the transient toast. Dialogs size to their content and recede the screen behind them.
use super::{
    LABEL_GAP, TAG_GAP, clip_spans, command_lines, dashboard, ellipsis, impact_lines, list,
    list_offset, pad, popup, related_tag, selection, theme, wrap_words,
};
use crate::{
    i18n::I18n,
    ops::Choice,
    theme::{Level, Role, Theme},
    tui::{
        App, Confirmation, Form, FormMode, GROUPS, motion,
        palette::{Hit, ResultRow, Results},
        related,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, ListItem, ListState, Padding, Paragraph},
};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
#[path = "form_tests.rs"]
mod form_tests;

const FORM_WIDTH: u16 = 92;
const PROSE_WIDTH: u16 = 84;
const OVERVIEW_WIDTH: u16 = 100;
const HELP_WIDTH: u16 = 80;
const LOADING_WIDTH: u16 = 56;
const BUTTON_GAP: u16 = 2;
/// List rows the picker shows at most; longer lists scroll.
const PICKER_ROWS: u16 = 12;
/// Result rows the Command_Palette shows; more scroll.
const PALETTE_ROWS: u16 = 12;

/// Content area of a dialog frame drawn over `rect`.
fn inner(rect: Rect) -> Rect {
    theme::dialog_frame().inner(rect)
}
fn open(t: &Theme, f: &mut Frame, rect: Rect) {
    let buffer = f.buffer_mut();
    theme::scrim(t, buffer, rect);
    // A wide glyph just left of the dialog would cover its left border, since a terminal
    // draws both its cells; blank it, as a terminal does once its second half is overwritten.
    if rect.x > buffer.area.x {
        for y in rect.top()..rect.bottom() {
            let cell = &mut buffer[(rect.x - 1, y)];
            if cell.symbol().width() > 1 {
                cell.set_symbol(" ");
            }
        }
    }
    f.render_widget(Clear, rect);
    theme::shadow(t, f, rect);
}

fn notice_popup(area: Rect, notice: &str) -> (Rect, Vec<String>) {
    let width = PROSE_WIDTH.min(area.width.saturating_sub(2));
    let lines = wrap_words(notice, width.saturating_sub(6) as usize);
    (popup(area, width, lines.len() as u16 + 5), lines)
}

fn active_popup(app: &App, area: Rect) -> Option<Rect> {
    Some(match app.overlay_kind() {
        1 => {
            ConfirmationGeometry::new(area, app.confirmation.as_ref().unwrap(), &app.ctx.i18n).popup
        }
        2 => popup(area, LOADING_WIDTH, 6),
        3 => {
            let form = app.form.as_ref().unwrap();
            let picker = form.picker.as_ref().unwrap();
            let choices = form.choices(form.index);
            PickerGeometry::new(
                area,
                choices.len(),
                picker.cursor,
                picker.matches(choices).len(),
            )
            .popup
        }
        4 => FormGeometry::new(area, app.form.as_ref().unwrap(), app).popup,
        5 => notice_popup(area, app.notice.as_ref().unwrap()).0,
        6 => popup(area, HELP_WIDTH, 22),
        7 => CommandPaletteGeometry::new(area, app, &app.palette_results()).popup,
        8 => OverviewGeometry::new(area, app).popup,
        9 => super::popup_full(area),
        _ => return None,
    })
}

pub(super) fn fade(app: &App, frame: &mut Frame) {
    let Some(opened) = app.dialog_opened else {
        return;
    };
    let opacity = motion::dialog_opacity(app.now.saturating_duration_since(opened));
    let background = theme::color(&app.theme.active, Role::Background);
    let Some(rect) = active_popup(app, frame.area()) else {
        return;
    };
    for row in rect.top()..rect.bottom() {
        for column in rect.left()..rect.right() {
            let cell = &mut frame.buffer_mut()[(column, row)];
            if matches!(
                (background, cell.fg),
                (
                    ratatui::style::Color::Rgb(..),
                    ratatui::style::Color::Rgb(..)
                )
            ) {
                cell.fg = theme::mix(background, cell.fg, opacity);
            }
            if matches!(
                (background, cell.bg),
                (
                    ratatui::style::Color::Rgb(..),
                    ratatui::style::Color::Rgb(..)
                )
            ) {
                cell.bg = theme::mix(background, cell.bg, opacity);
            }
        }
    }
}
fn hovered(app: &App, rect: Rect) -> bool {
    app.hover.is_some_and(|point| rect.contains(point))
}
fn hints(t: &Theme, f: &mut Frame, area: Rect, list: &[theme::Hint<'_>], pinned: theme::Hint<'_>) {
    f.render_widget(
        Paragraph::new(theme::key_hints(t, list, Some(pinned), area.width as usize)),
        area,
    );
}
fn button_label(text: &str) -> String {
    format!("[ {text} ]")
}
/// Hit areas match the drawn labels exactly, so a click lands where the button is seen.
fn button_rects(row: Rect, first: &str, second: &str) -> [Rect; 2] {
    let first_width = (button_label(first).width() as u16).min(row.width);
    let second_x = (row.x + first_width + BUTTON_GAP).min(row.right());
    let second_width =
        (button_label(second).width() as u16).min(row.right().saturating_sub(second_x));
    [
        Rect::new(row.x, row.y, first_width, 1),
        Rect::new(second_x, row.y, second_width, 1),
    ]
}
/// Selected buttons fill with their role; hovered ones underline to show the click target.
fn button(
    t: &Theme,
    f: &mut Frame,
    rect: Rect,
    text: &str,
    (idle, active): (Role, Role),
    state: (bool, bool),
) {
    let (selected, hover) = state;
    let restricted = theme::unframed(t) || t.patterns.glyphs == crate::theme::GlyphSet::Ascii;
    let idle = if restricted && matches!(idle, Role::Success | Role::Warning | Role::Danger) {
        Role::Text1
    } else {
        idle
    };
    let active = if restricted && matches!(active, Role::Success | Role::Warning | Role::Danger) {
        Role::Accent
    } else {
        active
    };
    let mut style = if selected {
        Style::default()
            .fg(theme::color(t, Role::OnAccent))
            .bg(theme::color(t, active))
            .add_modifier(Modifier::BOLD)
    } else if hover {
        theme::fg(t, active).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        theme::fg(t, idle)
    };
    if selected && t.patterns.glyphs == crate::theme::GlyphSet::Ascii {
        style = style.add_modifier(Modifier::REVERSED);
    }
    f.render_widget(
        Paragraph::new(Span::styled(button_label(text), style)),
        rect,
    );
}
/// Inputs are underlined: in `role` while active, in the border color otherwise.
fn field_style(t: &Theme, active: bool, role: Role) -> Style {
    Style::default()
        .add_modifier(Modifier::UNDERLINED)
        .underline_color(theme::color(t, if active { role } else { Role::Border }))
}
fn primary_key(form: &Form) -> &'static str {
    if form.mode == FormMode::Optimization {
        "ui.save_selection"
    } else if form.spec.reversible {
        "ui.confirm_execute"
    } else {
        "ui.continue"
    }
}

fn subtitle_key(form: &Form) -> &'static str {
    if form.mode == FormMode::Optimization {
        "ui.form_batch_title"
    } else if form.spec.reversible {
        "ui.form_reviewed_subtitle"
    } else {
        "ui.form_single_title"
    }
}

fn form_header(app: &App, form: &Form, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let mut lines = wrap_words(app.ctx.i18n.text(subtitle_key(form)), width)
        .into_iter()
        .map(|line| Line::styled(line, theme::muted(theme)))
        .collect::<Vec<_>>();
    if let Some(state) = related::related(&form.spec, &app.snapshot, &app.ctx.i18n, app.now)
        .and_then(|related| related.state.into_iter().next())
    {
        lines.push(Line::from(clip_spans(state.spans, width, ellipsis(theme))));
    }
    lines.extend(impact_lines(app, &form.spec, width));
    lines
}

struct ChipGeometry {
    choices: Vec<(usize, Rect)>,
    previous: Option<Rect>,
    next: Option<Rect>,
}

impl ChipGeometry {
    fn new(area: Rect, choices: &[&str], selected: usize) -> Self {
        let widths = choices
            .iter()
            .map(|choice| choice.width() + 2)
            .collect::<Vec<_>>();
        let fits = |first: usize, end: usize| {
            widths[first..end].iter().sum::<usize>()
                + (end - first).saturating_sub(1)
                + usize::from(first > 0) * 2
                + usize::from(end < choices.len()) * 2
                <= area.width as usize
        };
        let mut first = selected;
        let mut end = selected + 1;
        while first > 0 && fits(first - 1, end) {
            first -= 1;
        }
        while end < choices.len() && fits(first, end + 1) {
            end += 1;
        }
        let previous = (first > 0).then(|| Rect::new(area.x, area.y, 1, area.height));
        let next = (end < choices.len())
            .then(|| Rect::new(area.right().saturating_sub(1), area.y, 1, area.height));
        let mut column = area.x + if previous.is_some() { 2 } else { 0 };
        let choices = (first..end)
            .map(|index| {
                let rect =
                    Rect::new(column, area.y, widths[index] as u16, area.height).intersection(area);
                column = column.saturating_add(widths[index] as u16 + 1);
                (index, rect)
            })
            .collect();
        Self {
            choices,
            previous,
            next,
        }
    }
}

struct FieldGeometry {
    index: usize,
    area: Rect,
    label: Rect,
    input: Rect,
    description: Option<Rect>,
    chips: Option<ChipGeometry>,
}

/// Form layout shared by drawing and mouse hit tests.
pub(in crate::tui) struct FormGeometry {
    pub popup: Rect,
    /// Header, fields, error, buttons and key hints.
    pub sections: [Rect; 5],
    pub command: Rect,
    fields: Vec<FieldGeometry>,
    pub buttons: [Rect; 2],
}
impl FormGeometry {
    pub fn new(area: Rect, form: &Form, app: &App) -> Self {
        let width = inner(popup(area, FORM_WIDTH, area.height)).width as usize;
        let header_height = form_header(app, form, width).len() as u16;
        let command_height = command_lines(app, &form.spec, &form.parameters(), width)
            .len()
            .min(3) as u16;
        let error_height = if form.error.is_some() { 2 } else { 0 };
        let heights = (0..form.values.len())
            .map(|index| if form.chip_field(index) { 3 } else { 2 })
            .collect::<Vec<u16>>();
        let field_height = heights.iter().sum::<u16>() + heights.len().saturating_sub(1) as u16;
        let popup = popup(
            area,
            FORM_WIDTH,
            header_height + field_height + command_height + error_height + 8,
        );
        let [header, _, fields, _, error, command, _, buttons, hints] = Layout::vertical([
            Constraint::Length(header_height),
            Constraint::Length(1),
            Constraint::Min(heights.iter().copied().max().unwrap_or(0)),
            Constraint::Length(1),
            Constraint::Length(error_height),
            Constraint::Length(command_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        let selected = form.index.min(heights.len().saturating_sub(1));
        let mut first = 0;
        while first < selected
            && heights[first..=selected].iter().sum::<u16>() + (selected - first) as u16
                > fields.height
        {
            first += 1;
        }
        let mut row = fields.y;
        let mut field_layouts = Vec::new();
        for (index, height) in heights.iter().copied().enumerate().skip(first) {
            if row + height > fields.bottom() {
                break;
            }
            let label = Rect::new(fields.x, row, fields.width, 1);
            let input = Rect::new(fields.x + 2, row + 1, fields.width.saturating_sub(2), 1);
            let description = form
                .chip_field(index)
                .then(|| Rect::new(input.x, row + 2, input.width, 1));
            let chips = if form.chip_field(index) {
                let choices = form.spec.fields[index].choices;
                let selected = choices
                    .iter()
                    .position(|choice| *choice == form.values[index].text)
                    .unwrap_or(0);
                Some(ChipGeometry::new(input, choices, selected))
            } else {
                None
            };
            field_layouts.push(FieldGeometry {
                index,
                area: Rect::new(fields.x, row, fields.width, height),
                label,
                input,
                description,
                chips,
            });
            row += height + 1;
        }
        let primary = app.ctx.i18n.text(primary_key(form));
        Self {
            popup,
            sections: [header, fields, error, buttons, hints],
            command,
            fields: field_layouts,
            buttons: button_rects(buttons, primary, app.ctx.i18n.text("ui.cancel")),
        }
    }
    pub fn field_at(&self, point: Position) -> Option<usize> {
        self.fields
            .iter()
            .find(|field| field.area.contains(point))
            .map(|field| field.index)
    }
    pub fn chip_at(&self, point: Position) -> Option<(usize, usize)> {
        self.fields.iter().find_map(|field| {
            field
                .chips
                .as_ref()?
                .choices
                .iter()
                .find(|(_, rect)| rect.contains(point))
                .map(|(choice, _)| (field.index, *choice))
        })
    }
}

pub(super) fn draw_form(app: &App, f: &mut Frame) {
    let Some(form) = &app.form else {
        return;
    };
    // An open list replaces its form rather than stacking on it; the list's title names
    // both the action and the field.
    if form.picker.is_some() {
        return;
    }
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let geometry = FormGeometry::new(f.area(), form, app);
    let [header, _, error, _, hint_row] = geometry.sections;
    open(t, f, geometry.popup);
    f.render_widget(
        theme::dialog(
            t,
            theme::title(t, i.text(&form.spec.label()), Role::Accent),
            Role::Accent,
        ),
        geometry.popup,
    );
    f.render_widget(
        Paragraph::new(form_header(app, form, header.width as usize)),
        header,
    );
    let n = form.values.len();
    for layout in &geometry.fields {
        let index = layout.index;
        let label = layout.label;
        let input = layout.input;
        let field = &form.spec.fields[index];
        let active = form.index == index;
        let mut spans = vec![
            Span::styled(
                if active {
                    format!("{} ", theme::glyph(t, theme::Glyph::Mark))
                } else {
                    "  ".into()
                },
                theme::fg(t, Role::Accent),
            ),
            Span::styled(
                i.text(&field.label()).to_owned(),
                if active {
                    theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD)
                } else {
                    theme::bold()
                },
            ),
        ];
        if field.required {
            spans.push(Span::styled(" *", theme::fg(t, Role::Danger)));
        }
        spans.push(Span::styled(
            format!("  {}/{n}", index + 1),
            theme::muted(t),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), label);
        let value = &form.values[index];
        if form.listed(index) {
            // A listed value names an existing item, shown with its context.
            let role = if active { Role::Accent } else { Role::Info };
            let choices = form.choices(index);
            let chosen = choices.iter().position(|choice| choice.value == value.text);
            let mut line = vec![
                Span::styled(
                    format!("{} ", theme::glyph(t, theme::Glyph::Previous)),
                    theme::fg(t, role),
                ),
                Span::styled(
                    value.text.clone(),
                    theme::fg(t, role).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", theme::glyph(t, theme::Glyph::Next)),
                    theme::fg(t, role),
                ),
                Span::styled(
                    format!("  {}/{}", chosen.map_or(0, |at| at + 1), choices.len()),
                    theme::muted(t),
                ),
            ];
            if let Some(detail) = chosen
                .map(|at| choices[at].detail.as_str())
                .filter(|detail| !detail.is_empty())
            {
                line.push(Span::styled(format!("  {detail}"), theme::muted(t)));
            }
            f.render_widget(Paragraph::new(Line::from(line)), input);
        } else if field.editable() {
            let width = input.width.saturating_sub(1) as usize;
            let (text, cursor) = value.visible(width);
            f.render_widget(
                Paragraph::new(Span::styled(
                    pad(&text, width),
                    field_style(t, active, Role::Accent),
                )),
                input,
            );
            if active {
                f.set_cursor_position((input.x + cursor.min(width as u16), input.y));
            }
        } else if let Some(chips) = &layout.chips {
            for &(choice, rect) in &chips.choices {
                let selected = field.choices[choice] == value.text;
                let mut style = if selected {
                    theme::fg(t, Role::OnAccent).bg(theme::color(t, Role::Accent))
                } else {
                    theme::text(t, Level::Secondary)
                };
                if hovered(app, rect) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                f.render_widget(
                    Paragraph::new(Span::styled(format!(" {} ", field.choices[choice]), style)),
                    rect,
                );
            }
            for (rect, glyph) in [
                (chips.previous, theme::Glyph::Previous),
                (chips.next, theme::Glyph::Next),
            ] {
                if let Some(rect) = rect {
                    f.render_widget(
                        Paragraph::new(Span::styled(theme::glyph(t, glyph), theme::muted(t))),
                        rect,
                    );
                }
            }
            if let Some(rect) = layout.description {
                let key = format!("option.{}.{}.{}", form.spec.id, field.name, value.text);
                let description = super::clip_with(i.text(&key), rect.width as usize, ellipsis(t));
                f.render_widget(
                    Paragraph::new(Span::styled(description, theme::muted(t))),
                    rect,
                );
            }
        }
    }
    if let Some(message) = &form.error {
        let lines = wrap_words(
            &format!("{} {message}", theme::glyph(t, theme::Glyph::Failure)),
            error.width as usize,
        )
        .into_iter()
        .map(|row| Line::styled(row, theme::tone(t, crate::tui::Tone::Danger)))
        .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(lines), error);
    }
    f.render_widget(
        Paragraph::new(command_lines(
            app,
            &form.spec,
            &form.parameters(),
            geometry.command.width as usize,
        )),
        geometry.command,
    );
    let accent = (Role::Accent, Role::Accent);
    button(
        t,
        f,
        geometry.buttons[0],
        i.text(primary_key(form)),
        accent,
        (form.index == n, hovered(app, geometry.buttons[0])),
    );
    button(
        t,
        f,
        geometry.buttons[1],
        i.text("ui.cancel"),
        (Role::Text1, Role::Accent),
        (form.index == n + 1, hovered(app, geometry.buttons[1])),
    );
    let mut keys = Vec::new();
    if form.index < n && form.listed(form.index) {
        keys.push(("Enter", i.text("key.list")));
    }
    keys.extend([
        ("Tab/↑↓", i.text("key.field")),
        ("←→", i.text("key.choice")),
        ("Ctrl+Enter", i.text("key.submit")),
    ]);
    hints(t, f, hint_row, &keys, ("Esc", i.text("ui.cancel")));
}

/// Picker layout shared by drawing, paging and mouse hit tests. Its height follows the
/// whole list, not the filtered matches, so the dialog stays still while typing.
pub(in crate::tui) struct PickerGeometry {
    pub popup: Rect,
    /// Filter, list and key hints.
    pub sections: [Rect; 3],
    pub offset: usize,
}
impl PickerGeometry {
    pub fn new(area: Rect, total: usize, cursor: usize, shown: usize) -> Self {
        let rows = (total.max(1) as u16).min(PICKER_ROWS);
        // Borders and top padding (3), filter, gap, rows, gap and key hints.
        let popup = popup(area, FORM_WIDTH, rows + 7);
        let [filter, _, rows, _, hints] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        Self {
            popup,
            sections: [filter, rows, hints],
            offset: list_offset(cursor, shown, rows.height as usize),
        }
    }
    pub fn row_at(&self, point: Position, shown: usize) -> Option<usize> {
        if !self.sections[1].contains(point) {
            return None;
        }
        let row = self.offset + usize::from(point.y - self.sections[1].y);
        (row < shown).then_some(row)
    }
}
/// One row: the value, its context, and a tag on the item currently in effect.
fn choice_line(t: &Theme, choice: &Choice, width: usize, current: &str) -> Line<'static> {
    let tag = if choice.current {
        format!("  {current}")
    } else {
        String::new()
    };
    let value = super::clip_with(
        &choice.value,
        width.saturating_sub(tag.width()),
        ellipsis(t),
    );
    let room = width.saturating_sub(value.width() + tag.width());
    let detail = if choice.detail.is_empty() || room < 4 {
        String::new()
    } else {
        super::clip_with(&format!("  {}", choice.detail), room, ellipsis(t))
    };
    let gap = " ".repeat(room.saturating_sub(detail.width()));
    Line::from(vec![
        Span::raw(value),
        Span::styled(detail, theme::muted(t)),
        Span::raw(gap),
        Span::styled(tag, theme::tone(t, crate::tui::Tone::Success)),
    ])
}
pub(super) fn draw_picker(app: &App, f: &mut Frame) {
    let Some(form) = &app.form else {
        return;
    };
    let Some(picker) = &form.picker else {
        return;
    };
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let choices = form.choices(form.index);
    let matches = picker.matches(choices);
    let geometry = PickerGeometry::new(f.area(), choices.len(), picker.cursor, matches.len());
    let [filter, rows, hint_row] = geometry.sections;
    open(t, f, geometry.popup);
    let field = &form.spec.fields[form.index];
    let heading = format!(
        "{} · {}",
        i.text(&form.spec.label()),
        i.text(&field.label())
    );
    f.render_widget(
        theme::dialog(t, theme::title(t, heading, Role::Accent), Role::Accent),
        geometry.popup,
    );
    // Filter row: prompt, input, and how many items match.
    let count = format!("  {}/{}", matches.len(), choices.len());
    let width = (filter.width as usize).saturating_sub(2 + count.width() + 1);
    let (text, cursor) = picker.filter.visible(width);
    let shown = if text.is_empty() {
        Span::styled(
            pad(
                &super::clip_with(i.text("ui.filter_hint"), width, ellipsis(t)),
                width,
            ),
            theme::muted(t),
        )
    } else {
        Span::styled(pad(&text, width), field_style(t, true, Role::Accent))
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} ", theme::glyph(t, theme::Glyph::Mark)),
                theme::fg(t, Role::Accent),
            ),
            shown,
            Span::styled(count, theme::muted(t)),
        ])),
        filter,
    );
    f.set_cursor_position((filter.x + 2 + cursor.min(width as u16), filter.y));
    if matches.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(
                i.text("ui.no_match").to_owned(),
                theme::muted(t),
            )),
            rows,
        );
    } else {
        // The highlight symbol takes two cells of each row.
        let width = (rows.width as usize).saturating_sub(2);
        let current = i.text("ui.current");
        let items = matches
            .iter()
            .map(|choice| ListItem::new(choice_line(t, choice, width, current)))
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_selected(Some(picker.cursor))
            .with_offset(geometry.offset);
        f.render_stateful_widget(
            list(t, items, selection(t, Role::DialogSelection)),
            rows,
            &mut state,
        );
        theme::scrollbar(
            t,
            f,
            theme::track(geometry.popup, rows),
            matches.len(),
            rows.height as usize,
            geometry.offset,
        );
    }
    // Leaving a list that opened with its form leaves the form too.
    let back = if picker.with_form {
        i.text("ui.cancel")
    } else {
        i.text("key.back")
    };
    hints(
        t,
        f,
        hint_row,
        &[("↑↓", i.text("key.move")), ("Enter", i.text("key.pick"))],
        ("Esc", back),
    );
}

/// The button on the highlighted result row, "执行 ↵" (Requirement 8.4), padded into a chip.
fn exec_label(t: &Theme, i: &I18n) -> String {
    format!(
        " {} {} ",
        i.text("key.accept"),
        theme::glyph(t, theme::Glyph::Enter)
    )
}
/// Command_Palette layout shared by drawing and mouse hit tests (Requirements 17.6, 17.10).
/// Its height is fixed, so the dialog stays still while the query changes.
pub(in crate::tui) struct CommandPaletteGeometry {
    pub popup: Rect,
    /// The query row.
    pub input: Rect,
    /// The result rows.
    pub rows: Rect,
    pub hints: Rect,
    /// The first of `Results::rows` drawn.
    pub offset: usize,
    /// The number of `Results::rows`.
    count: usize,
    /// The highlighted row's "执行 ↵" while that row is drawn.
    exec: Option<Rect>,
}
impl CommandPaletteGeometry {
    pub fn new(area: Rect, app: &App, results: &Results) -> Self {
        // Borders and top padding (3), query, gap, result rows, gap and key hints.
        let popup = popup(area, FORM_WIDTH, PALETTE_ROWS + 7);
        let [input, _, rows, _, hints] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        let count = results.rows.len();
        let height = rows.height as usize;
        let highlighted = app
            .command_palette
            .as_ref()
            .and_then(|palette| results.row_of(palette.cursor));
        let offset = list_offset(highlighted.unwrap_or(0), count, height);
        // The button ends the row, after the two cells of the highlight symbol.
        let width = (exec_label(&app.theme.active, &app.ctx.i18n).width() as u16).min(rows.width);
        let exec = highlighted
            .filter(|row| (offset..offset + height).contains(row))
            .map(|row| {
                Rect::new(
                    rows.right() - width,
                    rows.y + (row - offset) as u16,
                    width,
                    1,
                )
            });
        Self {
            popup,
            input,
            rows,
            hints,
            offset,
            count,
            exec,
        }
    }
    /// Index into `Results::rows` of the row drawn under `point`.
    pub fn row_at(&self, point: Position) -> Option<usize> {
        if !self.rows.contains(point) {
            return None;
        }
        let row = self.offset + usize::from(point.y - self.rows.y);
        (row < self.count).then_some(row)
    }
    /// Whether `point` is on the highlighted row's "执行 ↵".
    pub fn exec_at(&self, point: Position) -> bool {
        self.exec.is_some_and(|rect| rect.contains(point))
    }
}
/// `text` as spans: the parts `ranges` mark in `matched`, the rest in `base`.
fn marked(text: &str, ranges: &[Range<usize>], base: Style, matched: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut at = 0;
    for range in ranges {
        if range.start > at {
            spans.push(Span::styled(text[at..range.start].to_owned(), base));
        }
        spans.push(Span::styled(text[range.clone()].to_owned(), matched));
        at = range.end;
    }
    if at < text.len() {
        spans.push(Span::styled(text[at..].to_owned(), base));
    }
    spans
}
/// A result row `width` cells wide (Requirement 8.4): the name with its matches in accent,
/// "…" for a Form_Action and "↗" for one that hands over the terminal, and for a hit by ID
/// alone the ID in text3 with its matches in accent; then, right-aligned before the last
/// `reserve` cells, which the highlighted row's "执行 ↵" covers, the Related_Reading's tag
/// and `root` for an Action that needs root. A row too narrow drops the tag first, then
/// clips the name. An Action that cannot run is drawn in text3 with its matches in bold
/// (Requirement 8.13). The selection colors replace the accent on the highlighted row, so
/// its matches are also underlined.
fn palette_line(
    app: &App,
    hit: &Hit,
    width: usize,
    reserve: usize,
    highlighted: bool,
) -> Line<'static> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let spec = &hit.spec;
    let dim = theme::text(t, Level::Tertiary);
    let available = app.available(spec).is_ok();
    let (base, mut matched, tag_style, root_style) = if available {
        (
            Style::default(),
            theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
            theme::muted(t),
            theme::fg(t, Role::Warning),
        )
    } else {
        (dim, dim.add_modifier(Modifier::BOLD), dim, dim)
    };
    if highlighted {
        matched = matched.add_modifier(Modifier::UNDERLINED);
    }
    let mut name = marked(i.text(&spec.label()), &hit.label_ranges, base, matched);
    let mut marks = String::new();
    if !spec.fields.is_empty() {
        marks.push_str(theme::glyph(t, theme::Glyph::Ellipsis));
    }
    if spec.external {
        marks.push(' ');
        marks.push_str(theme::glyph(t, theme::Glyph::External));
    }
    if !marks.is_empty() {
        name.push(Span::styled(marks, base));
    }
    if let Some(ranges) = &hit.id_ranges {
        name.push(Span::raw(LABEL_GAP));
        name.extend(marked(spec.id, ranges, dim, matched));
    }
    let cells = |spans: &[Span<'static>]| spans.iter().map(Span::width).sum::<usize>();
    let root = spec.root.then(|| Span::styled("root", root_style));
    let root_width = root.as_ref().map_or(0, Span::width);
    // The tag and `root`, with a gap between them when both show.
    let right = |tag: &[Span<'static>]| {
        let tag = cells(tag);
        tag + root_width
            + if tag > 0 && root_width > 0 {
                TAG_GAP
            } else {
                0
            }
    };
    let room = width.saturating_sub(reserve);
    let mut tag = related_tag(app, spec, tag_style);
    if cells(&name) + TAG_GAP + right(&tag) > room {
        tag.clear();
    }
    let right_width = right(&tag);
    let lead = if right_width > 0 {
        right_width + TAG_GAP
    } else {
        0
    };
    let mut spans = clip_spans(name, room.saturating_sub(lead), ellipsis(t));
    if right_width > 0 {
        let used = cells(&spans);
        spans.push(Span::raw(
            " ".repeat(room.saturating_sub(used + right_width)),
        ));
        let both = !tag.is_empty() && root.is_some();
        spans.extend(tag);
        if both {
            spans.push(Span::raw(" ".repeat(TAG_GAP)));
        }
        spans.extend(root);
    }
    Line::from(spans)
}
/// Command_Palette (design component 5): the query, then the result rows under their
/// category names, or what to type while there are none; the title counts the matches
/// against every Action (Requirement 8.5).
pub(super) fn draw_palette(app: &App, f: &mut Frame) {
    let Some(palette) = &app.command_palette else {
        return;
    };
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let results = app.palette_results();
    let geometry = CommandPaletteGeometry::new(f.area(), app, &results);
    open(t, f, geometry.popup);
    let count = format!(" {} / {} ", results.count(), crate::actions::specs().len());
    f.render_widget(
        theme::dialog(
            t,
            theme::title(t, i.text("ui.palette_title"), Role::Accent),
            Role::Accent,
        )
        .title(Line::styled(count, theme::muted(t)).right_aligned()),
        geometry.popup,
    );
    let input = geometry.input;
    let width = (input.width as usize).saturating_sub(2 + 1);
    let (text, cursor) = palette.query.visible(width);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} ", theme::glyph(t, theme::Glyph::Mark)),
                theme::fg(t, Role::Accent),
            ),
            Span::styled(pad(&text, width), field_style(t, true, Role::Accent)),
        ])),
        input,
    );
    f.set_cursor_position((input.x + 2 + cursor.min(width as u16), input.y));
    let rows = geometry.rows;
    if results.rows.is_empty() {
        let key = if palette.query.text.is_empty() {
            "ui.palette_hint"
        } else {
            "ui.no_match"
        };
        f.render_widget(
            Paragraph::new(Line::styled(i.text(key).to_owned(), theme::muted(t))),
            rows,
        );
    } else {
        let exec = exec_label(t, i);
        let highlighted = results.row_of(palette.cursor);
        // The highlight symbol takes two cells of each row; a blank precedes the button.
        let width = (rows.width as usize).saturating_sub(2);
        let reserve = exec.width() + 1;
        let items = results
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                ListItem::new(match row {
                    ResultRow::Group(group) => Line::styled(
                        i.text(&format!("group.{}", GROUPS[*group])).to_owned(),
                        theme::muted(t).add_modifier(Modifier::BOLD),
                    ),
                    ResultRow::Hit(hit) => {
                        palette_line(app, hit, width, reserve, highlighted == Some(index))
                    }
                })
            })
            .collect::<Vec<_>>();
        // A highlighted Action that cannot run keeps its tertiary text on the fill.
        let usable = results
            .hit(palette.cursor)
            .is_some_and(|hit| app.available(&hit.spec).is_ok());
        let highlight = if usable {
            selection(t, Role::DialogSelection)
        } else {
            Style::default().bg(theme::color(t, Role::DialogSelection))
        };
        let mut state = ListState::default()
            .with_selected(highlighted)
            .with_offset(geometry.offset);
        f.render_stateful_widget(list(t, items, highlight), rows, &mut state);
        theme::scrollbar(
            t,
            f,
            theme::track(geometry.popup, rows),
            results.rows.len(),
            rows.height as usize,
            geometry.offset,
        );
        if let Some(rect) = geometry.exec {
            let mut style = if usable {
                Style::default()
                    .fg(theme::color(t, Role::OnAccent))
                    .bg(theme::color(t, Role::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                theme::text(t, Level::Tertiary)
            };
            if hovered(app, rect) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            f.render_widget(Paragraph::new(Span::styled(exec, style)), rect);
        }
    }
    hints(
        t,
        f,
        geometry.hints,
        &[("↑↓", i.text("key.move")), ("Enter", i.text("key.accept"))],
        ("Esc", i.text("key.close")),
    );
}

/// Shown only once a read takes noticeably long, so quick reads never flash a dialog.
pub(super) fn draw_loading(app: &App, f: &mut Frame) {
    let Some(loading) = &app.loading else {
        return;
    };
    if app.now.saturating_duration_since(loading.started) < motion::LOADING_DELAY {
        return;
    }
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let rect = popup(f.area(), LOADING_WIDTH, 6);
    let [text, _, hint_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner(rect));
    open(t, f, rect);
    f.render_widget(
        theme::dialog(
            t,
            theme::title(t, i.text(&loading.spec.label()), Role::Accent),
            Role::Accent,
        ),
        rect,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", app.spin()), theme::fg(t, Role::Accent)),
            Span::raw(i.text("ui.loading_choices").to_owned()),
        ])),
        text,
    );
    hints(t, f, hint_row, &[], ("Esc", i.text("ui.cancel")));
}

/// Confirmation layout: the dialog grows with its text up to the screen, then scrolls.
pub(in crate::tui) struct ConfirmationGeometry {
    pub popup: Rect,
    /// Text, controls and key hints.
    pub sections: [Rect; 3],
    pub buttons: [Rect; 2],
    lines: Vec<String>,
    /// Wrapped rows of the first paragraph, which states the question and renders bold.
    lead: usize,
}
impl ConfirmationGeometry {
    pub fn new(area: Rect, confirm: &Confirmation, i: &I18n) -> Self {
        let width = PROSE_WIDTH.min(area.width.saturating_sub(2));
        let text_width = width.saturating_sub(6) as usize;
        let (first, rest) = confirm
            .text
            .split_once('\n')
            .unwrap_or((confirm.text.as_str(), ""));
        let mut lines = wrap_words(first, text_width);
        let lead = lines.len();
        if !rest.is_empty() {
            lines.push(String::new());
            lines.extend(wrap_words(rest, text_width));
        }
        let controls = if confirm.token == "YES" { 1 } else { 2 };
        // Borders and top padding (3), text, gap, controls, gap and key hints.
        let height = lines.len() as u16 + controls + 6;
        let popup = popup(area, width, height);
        let [text, _, controls, _, hints] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(controls),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        Self {
            popup,
            sections: [text, controls, hints],
            buttons: button_rects(controls, i.text("ui.cancel"), i.text("ui.confirm_execute")),
            lines,
            lead,
        }
    }
    pub fn max_scroll(&self) -> u16 {
        self.lines
            .len()
            .saturating_sub(self.sections[0].height as usize)
            .min(u16::MAX as usize) as u16
    }
}

pub(super) fn draw_confirmation(app: &App, f: &mut Frame) {
    let Some(confirm) = &app.confirmation else {
        return;
    };
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let geometry = ConfirmationGeometry::new(f.area(), confirm, i);
    let [text, controls, hint_row] = geometry.sections;
    // Typed tokens guard destructive or hard-to-reverse operations.
    let typed = confirm.token != "YES";
    let role = if typed { Role::Danger } else { Role::Warning };
    open(t, f, geometry.popup);
    f.render_widget(
        theme::dialog(
            t,
            theme::icon_title(t, "!", i.text("ui.confirm"), role),
            role,
        ),
        geometry.popup,
    );
    let scroll = confirm.scroll.min(geometry.max_scroll()) as usize;
    let lines = geometry
        .lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(text.height as usize)
        .map(|(row, line)| {
            if row < geometry.lead {
                Line::styled(line.clone(), theme::bold())
            } else {
                Line::from(line.clone())
            }
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), text);
    theme::scrollbar(
        t,
        f,
        theme::track(geometry.popup, text),
        geometry.lines.len(),
        text.height as usize,
        scroll,
    );
    let mut list = Vec::new();
    if typed {
        let prompt = i.format("ui.type_confirm", &[("token", &confirm.token)]);
        let (before, after) = prompt.split_once(&confirm.token).unwrap_or((&prompt, ""));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(before.to_owned(), theme::muted(t)),
                Span::styled(
                    confirm.token.clone(),
                    theme::fg(t, role).add_modifier(Modifier::BOLD),
                ),
                Span::styled(after.to_owned(), theme::muted(t)),
            ])),
            Rect {
                height: 1,
                ..controls
            },
        );
        let input = Rect::new(controls.x, controls.y + 1, controls.width, 1);
        let matched = confirm.input.text == confirm.token;
        let width = input.width.saturating_sub(4) as usize;
        let (value, cursor) = confirm.input.visible(width);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{} ", theme::glyph(t, theme::Glyph::Mark)),
                    theme::fg(t, role),
                ),
                Span::styled(pad(&value, width), field_style(t, true, role)),
                Span::styled(
                    if matched {
                        format!(" {}", theme::glyph(t, theme::Glyph::Success))
                    } else {
                        String::new()
                    },
                    theme::fg(t, Role::Success).add_modifier(Modifier::BOLD),
                ),
            ])),
            input,
        );
        f.set_cursor_position((input.x + 2 + cursor.min(width as u16), input.y));
        list.push(("Enter", i.text("key.confirm")));
    } else {
        button(
            t,
            f,
            geometry.buttons[0],
            i.text("ui.cancel"),
            (Role::Text1, Role::Accent),
            (!confirm.accept, hovered(app, geometry.buttons[0])),
        );
        button(
            t,
            f,
            geometry.buttons[1],
            i.text("ui.confirm_execute"),
            (Role::Warning, Role::Warning),
            (confirm.accept, hovered(app, geometry.buttons[1])),
        );
        list.extend([
            ("←→", i.text("key.choose")),
            ("Enter", i.text("key.confirm")),
            ("Y", i.text("key.accept")),
        ]);
    }
    if geometry.max_scroll() > 0 {
        list.push(("PgUp/PgDn", i.text("key.scroll")));
    }
    hints(t, f, hint_row, &list, ("Esc", i.text("ui.cancel")));
}

/// Errors that need acknowledgement: red frame and glyph, sized to the message.
pub(super) fn draw_notice(app: &App, f: &mut Frame) {
    let Some(notice) = &app.notice else {
        return;
    };
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let (rect, lines) = notice_popup(f.area(), notice);
    let [text, _, hint_row] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner(rect));
    open(t, f, rect);
    f.render_widget(
        theme::dialog(
            t,
            theme::icon_title(
                t,
                theme::glyph(t, theme::Glyph::Failure),
                i.text("ui.notice"),
                Role::Danger,
            ),
            Role::Danger,
        ),
        rect,
    );
    f.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()),
        text,
    );
    hints(t, f, hint_row, &[], ("Enter/Esc", i.text("key.back")));
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);
/// Complete key reference in two columns; the footer shows only the keys for the moment.
const HELP: [HelpSection; 5] = [
    (
        "help.navigate",
        &[
            ("↑↓ / j k", "key.move"),
            ("←→/Tab/Shift+Tab", "key.pan"),
            ("Home/End", "key.ends"),
            ("1–9", "key.jump"),
            ("O / 0", "key.optimize"),
            ("Alt+↑↓", "key.reorder"),
        ],
    ),
    (
        "help.run",
        &[
            ("Enter", "key.open"),
            ("/", "key.search"),
            ("Space", "key.select"),
            ("X / F5", "key.run_checklist"),
            ("A", "key.recommended"),
        ],
    ),
    (
        "help.view",
        &[
            ("R", "key.refresh"),
            ("N", "key.public"),
            ("G", "key.logs"),
            ("F2", "key.full_log"),
            ("L", "key.language"),
            ("T", "key.style"),
        ],
    ),
    (
        "help.task",
        &[
            ("Esc", "key.cancel_task"),
            ("Q / Ctrl+C", "key.quit"),
            ("PgUp/PgDn", "key.scroll"),
        ],
    ),
    (
        "help.palette",
        &[
            ("a…z / ←→", "key.edit"),
            ("Home/End/Bksp/Del/Ctrl+U", "key.edit"),
            ("↑↓/PgUp/PgDn", "key.move"),
            ("Enter/Tab/Esc", "key.palette_actions"),
        ],
    ),
];
fn help_row(t: &Theme, key: &str, label: &str, width: usize) -> Line<'static> {
    let rendered_key = if t.patterns.glyphs == crate::theme::GlyphSet::Ascii {
        theme::keycap(t, key).content.into_owned()
    } else {
        key.to_owned()
    };
    let key = rendered_key.as_str();
    let key_width = key.width().max(12) + 1;
    Line::from(vec![
        Span::styled(pad(key, key_width), theme::bold()),
        Span::styled(
            super::clip_with(label, width.saturating_sub(key_width), ellipsis(t)),
            theme::muted(t),
        ),
    ])
}
fn help_column(t: &Theme, i: &I18n, sections: &[HelpSection], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (heading, rows) in sections {
        lines.push(Line::styled(
            i.text(heading).to_owned(),
            theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
        ));
        for (key, label) in *rows {
            lines.push(help_row(t, key, i.text(label), width));
        }
    }
    lines
}
pub(super) fn draw_help(app: &App, f: &mut Frame) {
    if !app.help {
        return;
    }
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let width = HELP_WIDTH.min(f.area().width.saturating_sub(2));
    let column_width = (width.saturating_sub(6) / 2) as usize;
    let left = help_column(t, i, &[HELP[0], HELP[1], HELP[4]], column_width);
    let mut right = help_column(t, i, &HELP[2..4], column_width);
    right.push(Line::styled(
        i.text("help.style").to_owned(),
        theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
    ));
    let (style, restore) = super::style_hints(app);
    right.extend(
        style
            .into_iter()
            .chain([restore])
            .map(|(key, label)| help_row(t, key, label, column_width)),
    );
    let rect = popup(f.area(), width, 22);
    let [columns, hint_row] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
        .areas(rect.inner(super::Margin::new(2, 1)));
    let [left_area, right_area] = Layout::horizontal([Constraint::Fill(1); 2])
        .spacing(2)
        .areas(columns);
    open(t, f, rect);
    f.render_widget(
        theme::dialog(
            t,
            theme::title(t, i.text("help.title"), Role::Accent),
            Role::Accent,
        ),
        rect,
    );
    f.render_widget(Paragraph::new(left), left_area);
    f.render_widget(Paragraph::new(right), right_area);
    hints(t, f, hint_row, &[], ("Esc", i.text("key.close")));
}

/// Overview layout shared by drawing and scroll clamping.
pub(in crate::tui) struct OverviewGeometry {
    pub popup: Rect,
    pub body: Rect,
    hints: Rect,
    lines: Vec<Line<'static>>,
}
impl OverviewGeometry {
    pub fn new(area: Rect, app: &App) -> Self {
        let width = OVERVIEW_WIDTH.min(area.width.saturating_sub(2));
        let lines = dashboard::overview(app, width.saturating_sub(6) as usize);
        let popup = popup(area, width, lines.len() as u16 + 5);
        let [body, _, hints] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        Self {
            popup,
            body,
            hints,
            lines,
        }
    }
    pub fn page(&self) -> u16 {
        self.body.height.saturating_sub(1).max(1)
    }
    pub fn max_scroll(&self) -> u16 {
        self.lines
            .len()
            .saturating_sub(self.body.height as usize)
            .min(u16::MAX as usize) as u16
    }
}
pub(super) fn draw_overview(app: &App, f: &mut Frame) {
    if !app.overview {
        return;
    }
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let geometry = OverviewGeometry::new(f.area(), app);
    let scroll = app.overview_scroll.min(geometry.max_scroll());
    open(t, f, geometry.popup);
    let heading = theme::title(t, i.text("dash.title"), Role::Accent);
    let room = dashboard::title_room(geometry.popup.width, &heading);
    let mut frame = theme::dialog(t, heading, Role::Accent);
    if let Some(line) = dashboard::freshness(app, room) {
        frame = frame.title(line);
    }
    f.render_widget(frame, geometry.popup);
    let total = geometry.lines.len();
    f.render_widget(
        Paragraph::new(geometry.lines).scroll((scroll, 0)),
        geometry.body,
    );
    theme::scrollbar(
        t,
        f,
        theme::track(geometry.popup, geometry.body),
        total,
        geometry.body.height as usize,
        scroll as usize,
    );
    hints(
        t,
        f,
        geometry.hints,
        &[
            ("R", i.text("key.refresh")),
            ("N", i.text("key.public")),
            ("PgUp/PgDn", i.text("key.scroll")),
        ],
        ("Esc", i.text("key.back")),
    );
}

/// Transient, non-blocking feedback above the footer's right edge; it fades into the border
/// color before expiring and never takes input.
pub(super) fn draw_toast(app: &App, f: &mut Frame, footer: Rect) {
    let Some(toast) = &app.toast else {
        return;
    };
    let opacity = motion::toast_opacity(app.now.saturating_duration_since(toast.shown));
    if opacity <= 0.0 {
        return;
    }
    let t = &app.theme.active;
    // The frame settles into an ordinary border as the toast fades.
    let border = theme::color(t, Role::Border);
    let tone = theme::mix(border, theme::color(t, theme::role(toast.tone)), opacity);
    let text_color = if opacity < 0.5 {
        border
    } else {
        theme::color(t, Role::Text1)
    };
    let room = footer.width.saturating_sub(12) as usize;
    let text = super::clip_with(&toast.text, room, ellipsis(t));
    let width = text.width() as u16 + 6;
    let rect = Rect::new(
        footer.right().saturating_sub(width + 1),
        footer.y.saturating_sub(3),
        width,
        3,
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} ", theme::icon(t, toast.tone)),
                Style::default().fg(tone).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(text_color)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(theme::border_set(t))
                .border_style(
                    if theme::unframed(t) || t.patterns.glyphs == crate::theme::GlyphSet::Ascii {
                        theme::fg(t, Role::Border)
                    } else {
                        Style::default().fg(tone)
                    },
                )
                .style(Style::default().bg(theme::color(t, Role::Dialog)))
                .padding(Padding::horizontal(1)),
        ),
        rect,
    );
}
