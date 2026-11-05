//! Modal layers: parameter form, confirmation, error notice, system overview and key help,
//! plus the transient toast. Dialogs size to their content and recede the screen behind them.
use super::{clip, dashboard, list, list_offset, pad, popup, theme, wrap_words};
use crate::{
    i18n::I18n,
    ops::Choice,
    tui::{App, Confirmation, Form, FormMode, Tone, motion},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, ListItem, ListState, Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

const FORM_WIDTH: u16 = 92;
const PROSE_WIDTH: u16 = 84;
const OVERVIEW_WIDTH: u16 = 100;
const HELP_WIDTH: u16 = 80;
const LOADING_WIDTH: u16 = 56;
const BUTTON_GAP: u16 = 2;
/// List rows the picker shows at most; longer lists scroll.
const PICKER_ROWS: u16 = 12;

/// Content area of a dialog frame drawn over `rect`.
fn inner(rect: Rect) -> Rect {
    theme::dialog(Line::default(), Tone::Plain).inner(rect)
}
fn open(f: &mut Frame, rect: Rect) {
    theme::scrim(f.buffer_mut(), rect);
    f.render_widget(Clear, rect);
}
fn hovered(app: &App, rect: Rect) -> bool {
    app.hover.is_some_and(|point| rect.contains(point))
}
fn hints(f: &mut Frame, area: Rect, list: &[theme::Hint<'_>], pinned: theme::Hint<'_>) {
    f.render_widget(
        Paragraph::new(theme::key_hints(list, Some(pinned), area.width as usize)),
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
/// Selected buttons fill with their tone; hovered ones underline to show the click target.
fn button(
    f: &mut Frame,
    rect: Rect,
    text: &str,
    (idle, active): (Tone, Tone),
    state: (bool, bool),
) {
    let (selected, hover) = state;
    let style = if selected {
        Style::default()
            .fg(theme::ON_TONE)
            .bg(theme::color(active))
            .add_modifier(Modifier::BOLD)
    } else if hover {
        theme::fg(active).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        theme::fg(idle)
    };
    f.render_widget(
        Paragraph::new(Span::styled(button_label(text), style)),
        rect,
    );
}
fn field_style(active: bool, tone: Tone) -> Style {
    Style::default()
        .add_modifier(Modifier::UNDERLINED)
        .underline_color(if active {
            theme::color(tone)
        } else {
            theme::BORDER
        })
}
fn primary_key(form: &Form) -> &'static str {
    if form.mode == FormMode::Optimization {
        "ui.save_selection"
    } else {
        "ui.continue"
    }
}

/// Form layout shared by drawing and mouse hit tests.
pub(in crate::tui) struct FormGeometry {
    pub popup: Rect,
    /// Subtitle, fields, error, buttons and key hints.
    pub sections: [Rect; 5],
    pub first: usize,
    pub visible: usize,
    pub buttons: [Rect; 2],
}
impl FormGeometry {
    pub fn new(area: Rect, form: &Form, i: &I18n) -> Self {
        // Borders and top padding, subtitle, gap, fields (label, input and a spacer between
        // fields), error (two rows), buttons, gap and key hints.
        let popup = popup(area, FORM_WIDTH, form.values.len() as u16 * 3 + 9);
        let [subtitle, _, fields, error, buttons, _, hints] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner(popup));
        // Each field takes three rows except the last, which needs no spacer.
        let visible = ((fields.height as usize + 1) / 3).max(1);
        let first = list_offset(
            form.index.min(form.values.len().saturating_sub(1)),
            form.values.len(),
            visible,
        );
        let primary = i.text(primary_key(form));
        Self {
            popup,
            sections: [subtitle, fields, error, buttons, hints],
            first,
            visible,
            buttons: button_rects(buttons, primary, i.text("ui.cancel")),
        }
    }
    pub fn field_at(&self, point: Position, count: usize) -> Option<usize> {
        if !self.sections[1].contains(point) {
            return None;
        }
        let row = point.y.saturating_sub(self.sections[1].y) as usize / 3;
        (row < self.visible && self.first + row < count).then_some(self.first + row)
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
    let geometry = FormGeometry::new(f.area(), form, i);
    let [subtitle, fields, error, _, hint_row] = geometry.sections;
    open(f, geometry.popup);
    f.render_widget(
        theme::dialog(
            theme::title(i.text(&form.spec.label()), Tone::Accent),
            Tone::Accent,
        ),
        geometry.popup,
    );
    f.render_widget(
        Paragraph::new(i.text(if form.mode == FormMode::Optimization {
            "ui.form_batch_title"
        } else {
            "ui.form_single_title"
        }))
        .style(theme::muted()),
        subtitle,
    );
    let n = form.values.len();
    for index in geometry.first..(geometry.first + geometry.visible).min(n) {
        let y = fields.y + (index - geometry.first) as u16 * 3;
        let label = Rect::new(fields.x, y, fields.width, 1);
        let input = Rect::new(fields.x + 2, y + 1, fields.width.saturating_sub(2), 1);
        let field = &form.spec.fields[index];
        let active = form.index == index;
        let mut spans = vec![
            Span::styled(if active { "› " } else { "  " }, theme::fg(Tone::Accent)),
            Span::styled(
                i.text(&field.label()).to_owned(),
                if active {
                    theme::fg(Tone::Accent).add_modifier(Modifier::BOLD)
                } else {
                    theme::bold()
                },
            ),
        ];
        if field.required {
            spans.push(Span::styled(" *", theme::fg(Tone::Danger)));
        }
        spans.push(Span::styled(format!("  {}/{n}", index + 1), theme::muted()));
        f.render_widget(Paragraph::new(Line::from(spans)), label);
        let value = &form.values[index];
        if form.listed(index) {
            // A listed value names an existing item, shown with its context.
            let tone = if active { Tone::Accent } else { Tone::Info };
            let choices = form.choices(index);
            let chosen = choices.iter().position(|choice| choice.value == value.text);
            let mut line = vec![
                Span::styled("‹ ", theme::fg(tone)),
                Span::styled(
                    value.text.clone(),
                    theme::fg(tone).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ›", theme::fg(tone)),
                Span::styled(
                    format!("  {}/{}", chosen.map_or(0, |at| at + 1), choices.len()),
                    theme::muted(),
                ),
            ];
            if let Some(detail) = chosen
                .map(|at| choices[at].detail.as_str())
                .filter(|detail| !detail.is_empty())
            {
                line.push(Span::styled(format!("  {detail}"), theme::muted()));
            }
            f.render_widget(Paragraph::new(Line::from(line)), input);
        } else if field.editable() {
            let width = input.width.saturating_sub(1) as usize;
            let (text, cursor) = value.visible(width);
            f.render_widget(
                Paragraph::new(Span::styled(
                    pad(&text, width),
                    field_style(active, Tone::Accent),
                )),
                input,
            );
            if active {
                f.set_cursor_position((input.x + cursor.min(width as u16), input.y));
            }
        } else {
            let tone = if active { Tone::Accent } else { Tone::Info };
            let position = field
                .choices
                .iter()
                .position(|choice| *choice == value.text)
                .map_or(0, |at| at + 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("‹ ", theme::fg(tone)),
                    Span::styled(
                        value.text.clone(),
                        theme::fg(tone).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ›", theme::fg(tone)),
                    Span::styled(
                        format!("  {position}/{}", field.choices.len()),
                        theme::muted(),
                    ),
                ])),
                input,
            );
        }
    }
    if let Some(message) = &form.error {
        let lines = wrap_words(&format!("✗ {message}"), error.width as usize)
            .into_iter()
            .map(|row| Line::styled(row, theme::fg(Tone::Danger)))
            .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(lines), error);
    }
    let accent = (Tone::Accent, Tone::Accent);
    button(
        f,
        geometry.buttons[0],
        i.text(primary_key(form)),
        accent,
        (form.index == n, hovered(app, geometry.buttons[0])),
    );
    button(
        f,
        geometry.buttons[1],
        i.text("ui.cancel"),
        (Tone::Plain, Tone::Accent),
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
    hints(f, hint_row, &keys, ("Esc", i.text("ui.cancel")));
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
fn choice_line(choice: &Choice, width: usize, current: &str) -> Line<'static> {
    let tag = if choice.current {
        format!("  {current}")
    } else {
        String::new()
    };
    let value = clip(&choice.value, width.saturating_sub(tag.width()));
    let room = width.saturating_sub(value.width() + tag.width());
    let detail = if choice.detail.is_empty() || room < 4 {
        String::new()
    } else {
        clip(&format!("  {}", choice.detail), room)
    };
    let gap = " ".repeat(room.saturating_sub(detail.width()));
    Line::from(vec![
        Span::raw(value),
        Span::styled(detail, theme::muted()),
        Span::raw(gap),
        Span::styled(tag, theme::fg(Tone::Success)),
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
    let choices = form.choices(form.index);
    let matches = picker.matches(choices);
    let geometry = PickerGeometry::new(f.area(), choices.len(), picker.cursor, matches.len());
    let [filter, rows, hint_row] = geometry.sections;
    open(f, geometry.popup);
    let field = &form.spec.fields[form.index];
    let heading = format!(
        "{} · {}",
        i.text(&form.spec.label()),
        i.text(&field.label())
    );
    f.render_widget(
        theme::dialog(theme::title(heading, Tone::Accent), Tone::Accent),
        geometry.popup,
    );
    // Filter row: prompt, input, and how many items match.
    let count = format!("  {}/{}", matches.len(), choices.len());
    let width = (filter.width as usize).saturating_sub(2 + count.width() + 1);
    let (text, cursor) = picker.filter.visible(width);
    let shown = if text.is_empty() {
        Span::styled(
            pad(&clip(i.text("ui.filter_hint"), width), width),
            theme::muted(),
        )
    } else {
        Span::styled(pad(&text, width), field_style(true, Tone::Accent))
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", theme::fg(Tone::Accent)),
            shown,
            Span::styled(count, theme::muted()),
        ])),
        filter,
    );
    f.set_cursor_position((filter.x + 2 + cursor.min(width as u16), filter.y));
    if matches.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(
                i.text("ui.no_match").to_owned(),
                theme::muted(),
            )),
            rows,
        );
    } else {
        // The highlight symbol takes two cells of each row.
        let width = (rows.width as usize).saturating_sub(2);
        let current = i.text("ui.current");
        let items = matches
            .iter()
            .map(|choice| ListItem::new(choice_line(choice, width, current)))
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_selected(Some(picker.cursor))
            .with_offset(geometry.offset);
        f.render_stateful_widget(list(items, true), rows, &mut state);
        theme::scrollbar(
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
        f,
        hint_row,
        &[("↑↓", i.text("key.move")), ("Enter", i.text("key.pick"))],
        ("Esc", back),
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
    let rect = popup(f.area(), LOADING_WIDTH, 6);
    let [text, _, hint_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner(rect));
    open(f, rect);
    f.render_widget(
        theme::dialog(
            theme::title(i.text(&loading.spec.label()), Tone::Accent),
            Tone::Accent,
        ),
        rect,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", app.spin()), theme::fg(Tone::Accent)),
            Span::raw(i.text("ui.loading_choices").to_owned()),
        ])),
        text,
    );
    hints(f, hint_row, &[], ("Esc", i.text("ui.cancel")));
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
    let geometry = ConfirmationGeometry::new(f.area(), confirm, i);
    let [text, controls, hint_row] = geometry.sections;
    // Typed tokens guard destructive or hard-to-reverse operations.
    let typed = confirm.token != "YES";
    let tone = if typed { Tone::Danger } else { Tone::Warning };
    open(f, geometry.popup);
    f.render_widget(
        theme::dialog(theme::icon_title("!", i.text("ui.confirm"), tone), tone),
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
                Span::styled(before.to_owned(), theme::muted()),
                Span::styled(
                    confirm.token.clone(),
                    theme::fg(tone).add_modifier(Modifier::BOLD),
                ),
                Span::styled(after.to_owned(), theme::muted()),
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
                Span::styled("› ", theme::fg(tone)),
                Span::styled(pad(&value, width), field_style(true, tone)),
                Span::styled(
                    if matched { " ✓" } else { "" },
                    theme::fg(Tone::Success).add_modifier(Modifier::BOLD),
                ),
            ])),
            input,
        );
        f.set_cursor_position((input.x + 2 + cursor.min(width as u16), input.y));
        list.push(("Enter", i.text("key.confirm")));
    } else {
        button(
            f,
            geometry.buttons[0],
            i.text("ui.cancel"),
            (Tone::Plain, Tone::Accent),
            (!confirm.accept, hovered(app, geometry.buttons[0])),
        );
        button(
            f,
            geometry.buttons[1],
            i.text("ui.confirm_execute"),
            (Tone::Warning, Tone::Warning),
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
    hints(f, hint_row, &list, ("Esc", i.text("ui.cancel")));
}

/// Errors that need acknowledgement: red frame and glyph, sized to the message.
pub(super) fn draw_notice(app: &App, f: &mut Frame) {
    let Some(notice) = &app.notice else {
        return;
    };
    let i = &app.ctx.i18n;
    let width = PROSE_WIDTH.min(f.area().width.saturating_sub(2));
    let lines = wrap_words(notice, width.saturating_sub(6) as usize);
    let rect = popup(f.area(), width, lines.len() as u16 + 5);
    let [text, _, hint_row] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner(rect));
    open(f, rect);
    f.render_widget(
        theme::dialog(
            theme::icon_title("✗", i.text("ui.notice"), Tone::Danger),
            Tone::Danger,
        ),
        rect,
    );
    f.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()),
        text,
    );
    hints(f, hint_row, &[], ("Enter/Esc", i.text("key.back")));
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);
/// Complete key reference in two columns; the footer shows only the keys for the moment.
const HELP: [HelpSection; 4] = [
    (
        "help.navigate",
        &[
            ("↑↓ / j k", "key.move"),
            ("Tab", "key.focus"),
            ("←→", "key.pan"),
            ("Home/End", "key.ends"),
            ("Alt+↑↓", "key.reorder"),
        ],
    ),
    (
        "help.run",
        &[
            ("Enter", "key.open"),
            ("O", "key.optimize"),
            ("Space", "key.select"),
            ("F5", "key.review"),
            ("Esc", "key.back"),
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
];
fn help_column(i: &I18n, sections: &[HelpSection], width: usize) -> Vec<Line<'static>> {
    let key_width = HELP
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|(key, _)| key.width()))
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for (heading, rows) in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(
            i.text(heading).to_owned(),
            theme::fg(Tone::Accent).add_modifier(Modifier::BOLD),
        ));
        for (key, label) in *rows {
            lines.push(Line::from(vec![
                Span::styled(pad(key, key_width + 2), theme::bold()),
                Span::styled(
                    clip(i.text(label), width.saturating_sub(key_width + 2)),
                    theme::muted(),
                ),
            ]));
        }
    }
    lines
}
pub(super) fn draw_help(app: &App, f: &mut Frame) {
    if !app.help {
        return;
    }
    let i = &app.ctx.i18n;
    let width = HELP_WIDTH.min(f.area().width.saturating_sub(2));
    let column_width = (width.saturating_sub(6 + 2) / 2) as usize;
    let left = help_column(i, &HELP[..2], column_width);
    let right = help_column(i, &HELP[2..], column_width);
    let note = wrap_words(i.text("key.note"), width.saturating_sub(6) as usize);
    let rows = left.len().max(right.len()) as u16;
    // Borders and top padding (3), columns, gap, note, gap and key hints.
    let rect = popup(f.area(), width, rows + note.len() as u16 + 6);
    let [columns, _, note_row, _, hint_row] = Layout::vertical([
        Constraint::Length(rows),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner(rect));
    let [left_area, right_area] = Layout::horizontal([Constraint::Fill(1); 2])
        .spacing(2)
        .areas(columns);
    open(f, rect);
    f.render_widget(
        theme::dialog(
            theme::title(i.text("help.title"), Tone::Accent),
            Tone::Accent,
        ),
        rect,
    );
    f.render_widget(Paragraph::new(left), left_area);
    f.render_widget(Paragraph::new(right), right_area);
    f.render_widget(
        Paragraph::new(note.into_iter().map(Line::from).collect::<Vec<_>>()).style(theme::muted()),
        note_row,
    );
    hints(f, hint_row, &[], ("Esc", i.text("key.close")));
}

/// Overview layout shared by drawing and scroll clamping.
pub(in crate::tui) struct OverviewGeometry {
    popup: Rect,
    body: Rect,
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
    let geometry = OverviewGeometry::new(f.area(), app);
    let scroll = app.overview_scroll.min(geometry.max_scroll());
    open(f, geometry.popup);
    let heading = theme::title(i.text("dash.title"), Tone::Accent);
    let room = dashboard::title_room(geometry.popup.width, &heading);
    let mut frame = theme::dialog(heading, Tone::Accent);
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
        f,
        theme::track(geometry.popup, geometry.body),
        total,
        geometry.body.height as usize,
        scroll as usize,
    );
    hints(
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
    let tone = theme::mix(theme::BORDER, theme::color(toast.tone), opacity);
    let text_color = if opacity < 0.5 {
        theme::BORDER
    } else {
        theme::TEXT
    };
    let room = footer.width.saturating_sub(12) as usize;
    let text = clip(&toast.text, room);
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
                format!("{} ", theme::icon(toast.tone)),
                Style::default().fg(tone).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(text_color)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(tone))
                .padding(Padding::horizontal(1)),
        ),
        rect,
    );
}
