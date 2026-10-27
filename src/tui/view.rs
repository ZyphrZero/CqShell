use super::{App, Focus, Form, FormMode, Phase};
use crate::{
    dashboard::{Reading, Usage},
    i18n::I18n,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CANVAS: Color = Color::Rgb(9, 14, 22);
const PANEL: Color = Color::Rgb(17, 27, 39);
const PANEL_ALT: Color = Color::Rgb(24, 38, 53);
const BORDER: Color = Color::Rgb(61, 82, 101);
const BORDER_ACTIVE: Color = Color::Rgb(69, 205, 182);
const TEXT: Color = Color::Rgb(229, 237, 243);
const MUTED: Color = Color::Rgb(145, 162, 176);
const TEAL: Color = Color::Rgb(69, 205, 182);
const BLUE: Color = Color::Rgb(112, 170, 255);
const AMBER: Color = Color::Rgb(255, 190, 94);
const VIOLET: Color = Color::Rgb(181, 145, 255);
const GREEN: Color = Color::Rgb(93, 216, 136);
const RED: Color = Color::Rgb(255, 117, 133);

pub(super) struct Regions {
    pub header: Rect,
    pub environment: Rect,
    pub dashboard: Rect,
    pub categories: Rect,
    pub actions: Rect,
    pub details: Rect,
    pub logs: Rect,
    pub footer: Rect,
}
impl Regions {
    pub fn new(area: Rect, app: &App) -> Self {
        let log_height =
            if area.height >= 34 && app.logs_open && (app.worker.is_some() || !app.logs.is_empty())
            {
                // Share the space below the dashboard with logs, keeping 10 menu rows.
                let content_height = area.height.saturating_sub(15);
                ((u32::from(content_height) * 3 / 5) as u16).min(content_height.saturating_sub(10))
            } else {
                0
            };
        let vertical = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(10),
            Constraint::Min(6),
            Constraint::Length(log_height),
            Constraint::Length(3),
        ])
        .split(area);
        let body =
            Layout::horizontal([Constraint::Length(22), Constraint::Min(30)]).split(vertical[3]);
        let (actions, details) = if area.width >= 112 {
            let columns =
                Layout::horizontal([Constraint::Length(44), Constraint::Min(25)]).split(body[1]);
            (columns[0], columns[1])
        } else {
            let rows = Layout::vertical([
                Constraint::Min(4),
                Constraint::Length(if body[1].height >= 13 { 5 } else { 3 }),
            ])
            .split(body[1]);
            (rows[0], rows[1])
        };
        Self {
            header: vertical[0],
            environment: vertical[1],
            dashboard: vertical[2],
            categories: body[0],
            actions,
            details,
            logs: vertical[4],
            footer: vertical[5],
        }
    }
}
fn block(title: impl Into<String>, active: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title.into())
        .title_style(
            Style::default()
                .fg(if active { TEAL } else { TEXT })
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(PANEL))
        .border_style(Style::default().fg(if active { BORDER_ACTIVE } else { BORDER }))
}
fn highlight(active: bool) -> Style {
    highlight_with(active, TEAL)
}
fn highlight_with(active: bool, accent: Color) -> Style {
    if active {
        Style::default()
            .fg(CANVAS)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT).bg(PANEL_ALT)
    }
}
fn phase_color(phase: Phase) -> Color {
    match phase {
        Phase::Completed => GREEN,
        Phase::Failed => RED,
        Phase::Cancelled | Phase::Cancelling => AMBER,
        Phase::Running => BLUE,
        Phase::Idle => MUTED,
    }
}
fn metric_color(index: usize) -> Color {
    [TEAL, BLUE, AMBER, VIOLET][index % 4]
}
pub(super) fn list_offset(selected: usize, count: usize, height: usize) -> usize {
    selected
        .saturating_sub(height.saturating_sub(1))
        .min(count.saturating_sub(height))
}
fn clipped(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = c.width().unwrap_or(0);
        if used + n > width.saturating_sub(1) {
            break;
        }
        result.push(c);
        used += n;
    }
    result.push('…');
    result
}
fn text<T: ToString>(value: &Reading<T>, i: &I18n) -> String {
    match value {
        Reading::Value(v) => v.to_string(),
        Reading::Loading => i.text("dash.loading").into(),
        Reading::NotQueried => i.text("dash.not_queried").into(),
        Reading::NotApplicable => i.text("dash.na").into(),
        Reading::Unavailable(_) => i.text("dash.unknown").into(),
    }
}
fn boolean(value: &Reading<bool>, i: &I18n, yes: &str, no: &str) -> String {
    match value {
        Reading::Value(v) => i.text(if *v { yes } else { no }).into(),
        _ => text(value, i),
    }
}
fn bytes(value: u64) -> String {
    let gib = value as f64 / 1073741824.0;
    if gib >= 1.0 {
        format!("{gib:.1} GiB")
    } else {
        format!("{:.0} MiB", value as f64 / 1048576.0)
    }
}
fn meter(percent: f64, width: usize) -> String {
    let filled = (percent.clamp(0.0, 100.0) / 100.0 * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled))
    )
}
fn usage(value: &Reading<Usage>, i: &I18n) -> String {
    match value {
        Reading::Value(v) => format!(
            "{} {} / {} ({:.0}%)",
            meter(v.percent(), 4),
            bytes(v.used),
            bytes(v.total),
            v.percent()
        ),
        _ => match value {
            Reading::Loading => i.text("dash.loading").into(),
            Reading::NotApplicable => i.text("dash.na").into(),
            _ => i.text("dash.unknown").into(),
        },
    }
}
fn uptime(value: &Reading<u64>, i: &I18n) -> String {
    match value {
        Reading::Value(seconds) => format!(
            "{}d {:02}h {:02}m",
            seconds / 86400,
            seconds % 86400 / 3600,
            seconds % 3600 / 60
        ),
        _ => text(value, i),
    }
}
fn cpu_usage(app: &App) -> String {
    match app.snapshot.local.cpu_usage {
        Reading::Value(value) => format!("{} {value:.1}%", meter(value as f64, 4)),
        _ => text(&app.snapshot.local.cpu_usage, &app.ctx.i18n),
    }
}
fn kernel_status(value: &Reading<String>, i: &I18n) -> String {
    match value {
        Reading::Value(version) => i.format("dash.kernel_running", &[("version", version)]),
        _ => text(value, i),
    }
}
fn metric_value_color(key: &str, app: &App) -> Color {
    match key {
        "dash.cpu_usage" => RED,
        "dash.memory" => VIOLET,
        "dash.disk" => AMBER,
        "dash.swap" => BLUE,
        "dash.kernel_status" if matches!(&app.snapshot.local.kernel, Reading::Value(_)) => GREEN,
        "dash.terminal_proxy" => match &app.snapshot.local.terminal_proxy {
            Reading::Value(true) => GREEN,
            Reading::Value(false) => MUTED,
            _ => TEXT,
        },
        "dash.ipv4" | "dash.ipv6" => TEAL,
        _ => TEXT,
    }
}
fn bbr(app: &App) -> String {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let enabled = match &data.congestion {
        Reading::Value(v) => i
            .text(if v == "bbr" {
                "status.enabled"
            } else {
                "status.disabled"
            })
            .into(),
        _ => text(&data.congestion, i),
    };
    let available = boolean(&data.bbr_available, i, "dash.available", "dash.unavailable");
    let version = if matches!(
        data.bbr_available,
        Reading::Value(false) | Reading::NotApplicable
    ) {
        String::new()
    } else {
        format!(" / {}", text(&data.bbr_version, i))
    };
    format!("{enabled} · {available}{version}")
}
fn network_summary(app: &App) -> String {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let mut parts = Vec::new();
    for value in [&data.congestion, &data.qdisc] {
        if !matches!(value, Reading::NotApplicable) {
            parts.push(text(value, i));
        }
    }
    if !matches!(data.bbr_available, Reading::NotApplicable)
        || !matches!(data.bbr_version, Reading::NotApplicable)
    {
        parts.push(bbr(app));
    }
    if parts.is_empty() {
        i.text("dash.na").into()
    } else {
        parts.join(" / ")
    }
}
type Metric = (&'static str, String);
fn metric_rows(app: &App) -> Vec<(Metric, Metric)> {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let public = &app.snapshot.public;
    vec![
        (
            ("dash.host", text(&data.host, i)),
            ("dash.os", text(&data.os, i)),
        ),
        (
            ("dash.kernel_status", kernel_status(&data.kernel, i)),
            (
                "dash.arch",
                format!("{} · {}", data.arch, uptime(&data.uptime, i)),
            ),
        ),
        (
            ("dash.cpu", text(&data.cpu, i)),
            ("dash.cpu_usage", cpu_usage(app)),
        ),
        (
            ("dash.memory", usage(&data.memory, i)),
            (
                "dash.disk",
                format!("{} {}", data.disk_name, usage(&data.disk, i)),
            ),
        ),
        (
            ("dash.swap", usage(&data.swap, i)),
            ("dash.timezone", text(&data.timezone, i)),
        ),
        (
            (
                "dash.terminal_proxy",
                boolean(&data.terminal_proxy, i, "status.enabled", "status.disabled"),
            ),
            ("dash.isp", text(&public.isp, i)),
        ),
        (
            ("dash.ipv4", text(&public.ipv4, i)),
            ("dash.ipv6", text(&public.ipv6, i)),
        ),
        (
            ("dash.network", network_summary(app)),
            (
                "dash.tcp",
                boolean(
                    &data.tcp_configured,
                    i,
                    "dash.configured",
                    "dash.not_configured",
                ),
            ),
        ),
    ]
}
fn draw_dashboard(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let age = app
        .snapshot
        .local
        .updated
        .map(|at| format!("{}s", at.elapsed().as_secs()))
        .unwrap_or_else(|| i.text("dash.loading").into());
    let frame = block(
        format!(
            " {} · {} ",
            i.text("dash.title"),
            i.format("dash.age", &[("age", &age)])
        ),
        false,
    );
    let inside = frame.inner(area);
    f.render_widget(frame, area);
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inside);
    let rows = metric_rows(app);
    for (column, rect) in columns.iter().enumerate() {
        let lines = rows
            .iter()
            .enumerate()
            .map(|(row_index, row)| {
                let (key, value) = if column == 0 { &row.0 } else { &row.1 };
                let label = format!(" {}: ", i.text(key));
                Line::from(vec![
                    Span::styled(
                        label.clone(),
                        Style::default()
                            .fg(metric_color(row_index))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        clipped(
                            value,
                            (rect.width as usize).saturating_sub(label.width() + 1),
                        ),
                        Style::default().fg(metric_value_color(key, app)),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(lines).style(Style::default().bg(PANEL)),
            *rect,
        );
    }
}
fn action_description(app: &App) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let spec = &app.items()[app.selected()];
    let mut lines = vec![
        Line::styled(
            i.text(&spec.label()).to_owned(),
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    if let Err(error) = app.available(spec) {
        lines.push(Line::styled(
            i.text("ui.unavailable").to_owned(),
            Style::default().fg(AMBER),
        ));
        lines.push(Line::from(i.error(&error)));
        lines.push(Line::default());
    }
    lines.push(Line::from(i.text(&spec.description()).to_owned()));
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "{} · {}",
            i.text(if spec.changes {
                "ui.mutating"
            } else {
                "ui.readonly"
            }),
            i.text(if spec.root { "ui.root" } else { "ui.user" })
        ),
        Style::default().fg(MUTED),
    ));
    if !spec.fields.is_empty() {
        lines.push(Line::default());
        let parameters = app
            .parameters
            .get(spec.id)
            .cloned()
            .unwrap_or_else(|| spec.defaults());
        for field in &spec.fields {
            let value = &parameters[field.name];
            lines.push(Line::from(format!(
                "{}: {}",
                i.text(&field.label()),
                if value.is_empty() {
                    i.text("ui.not_set")
                } else {
                    value
                }
            )));
        }
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        i.text(if app.optimization() {
            "ui.optimization_details"
        } else {
            "ui.action_details"
        })
        .to_owned(),
        Style::default().fg(TEAL),
    ));
    lines
}
fn draw_lists(app: &App, f: &mut Frame, regions: &Regions) {
    let i = &app.ctx.i18n;
    let items = app
        .category_order
        .iter()
        .enumerate()
        .map(|(display_index, group_index)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    "● ",
                    Style::default()
                        .fg(metric_color(display_index))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    i.text(&format!("group.{}", super::GROUPS[*group_index])),
                    Style::default().fg(TEXT),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let category_position = app.category_position();
    let mut state = ListState::default().with_selected(Some(category_position));
    *state.offset_mut() = list_offset(
        category_position,
        app.category_order.len(),
        regions.categories.height.saturating_sub(2) as usize,
    );
    f.render_stateful_widget(
        List::new(items)
            .block(block(
                i.text("ui.categories"),
                app.focus == Focus::Categories,
            ))
            .highlight_style(highlight_with(app.focus == Focus::Categories, AMBER))
            .highlight_symbol("› "),
        regions.categories,
        &mut state,
    );
    let specs = app.items();
    let list = specs
        .iter()
        .map(|spec| {
            let available = app.available(spec).is_ok();
            let label = i.text(&spec.label()).to_owned();
            if app.optimization() {
                let marked = app.marked.contains(spec.id);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if marked { "[x] " } else { "[ ] " },
                        Style::default().fg(if marked { AMBER } else { MUTED }),
                    ),
                    Span::styled(
                        label,
                        Style::default().fg(if available { TEXT } else { MUTED }),
                    ),
                ]))
            } else {
                ListItem::new(label).style(Style::default().fg(if available {
                    TEXT
                } else {
                    MUTED
                }))
            }
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selected()));
    *state.offset_mut() = list_offset(
        app.selected(),
        specs.len(),
        regions.actions.height.saturating_sub(2) as usize,
    );
    let title = if app.optimization() {
        i.format(
            "ui.optimization_title",
            &[("count", &app.marked.len().to_string())],
        )
    } else {
        format!(
            "{}  {}/{}",
            i.text("ui.actions"),
            app.selected() + 1,
            specs.len()
        )
    };
    f.render_stateful_widget(
        List::new(list)
            .block(block(title, app.focus == Focus::Actions))
            .highlight_style(highlight(app.focus == Focus::Actions))
            .highlight_symbol("› "),
        regions.actions,
        &mut state,
    );
    f.render_widget(
        Paragraph::new(action_description(app))
            .wrap(Wrap { trim: true })
            .block(block(i.text("ui.details"), false)),
        regions.details,
    );
}
fn draw_logs(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let height = area.height.saturating_sub(2) as usize;
    let rows = log_rows(&app.logs, area.width.saturating_sub(2) as usize);
    let end = rows.len().saturating_sub(app.scroll as usize);
    let start = end.saturating_sub(height);
    let lines = if app.logs.is_empty() {
        vec![Line::from(i.text("ui.empty_log"))]
    } else {
        rows.iter()
            .skip(start)
            .take(end - start)
            .map(|l| Line::from(l.as_str()))
            .collect()
    };
    let title = format!(
        "{} · {} · {}",
        i.text("ui.logs"),
        app.task_title(),
        i.text(app.phase.key())
    );
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .block(block(title, app.phase != Phase::Idle)),
        area,
    );
}
pub(super) fn log_rows(logs: &std::collections::VecDeque<String>, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for line in logs {
        let mut row = String::new();
        let mut used = 0;
        for c in line.replace('\t', "    ").chars() {
            let cells = c.width().unwrap_or(0);
            if used + cells > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += cells;
        }
        rows.push(row);
    }
    rows
}
fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
pub(super) struct FormGeometry {
    pub popup: Rect,
    pub sections: [Rect; 5],
    pub first: usize,
    pub visible: usize,
    pub buttons: [Rect; 2],
}
impl FormGeometry {
    pub fn new(area: Rect, form: &Form, i: &I18n) -> Self {
        let popup = popup(area, 92, form.values.len() as u16 * 3 + 10);
        let inside = block("", true).inner(popup);
        let split = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inside);
        let sections = [split[0], split[1], split[2], split[3], split[4]];
        let visible = (sections[1].height as usize / 3).max(1);
        let first = list_offset(
            form.index.min(form.values.len() - 1),
            form.values.len(),
            visible,
        );
        let primary = i.text(if form.mode == FormMode::Optimization {
            "ui.save_selection"
        } else {
            "ui.continue"
        });
        let buttons = button_rects(sections[3], primary, i.text("ui.cancel"));
        Self {
            popup,
            sections,
            first,
            visible,
            buttons,
        }
    }
    pub fn field_at(&self, point: ratatui::layout::Position, count: usize) -> Option<usize> {
        if !self.sections[1].contains(point) {
            return None;
        }
        let row = point.y.saturating_sub(self.sections[1].y) as usize / 3;
        (row < self.visible && self.first + row < count).then_some(self.first + row)
    }
}
pub(super) struct ConfirmationGeometry {
    pub popup: Rect,
    pub sections: [Rect; 3],
    pub buttons: [Rect; 2],
}
impl ConfirmationGeometry {
    pub fn new(area: Rect, i: &I18n) -> Self {
        let popup = popup(area, 100, area.height.saturating_sub(2));
        let inside = block("", true).inner(popup);
        let split = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(inside);
        let sections = [split[0], split[1], split[2]];
        let buttons = button_rects(
            sections[1],
            i.text("ui.cancel"),
            i.text("ui.confirm_execute"),
        );
        Self {
            popup,
            sections,
            buttons,
        }
    }
}
fn button_rects(row: Rect, first: &str, second: &str) -> [Rect; 2] {
    let width = (first.width() as u16 + 8).min(row.width);
    [
        Rect::new(row.x, row.y, width, 1),
        Rect::new(
            row.x + width,
            row.y,
            (second.width() as u16 + 8).min(row.width.saturating_sub(width)),
            1,
        ),
    ]
}
fn draw_form(app: &App, f: &mut Frame) {
    let Some(form) = &app.form else {
        return;
    };
    let i = &app.ctx.i18n;
    let geometry = FormGeometry::new(f.area(), form, i);
    let rect = geometry.popup;
    f.render_widget(Clear, rect);
    let frame = block(format!(" {} ", i.text(&form.spec.label())), true);
    f.render_widget(frame, rect);
    let sections = geometry.sections;
    f.render_widget(
        Paragraph::new(i.text(if form.mode == FormMode::Optimization {
            "ui.form_batch_title"
        } else {
            "ui.form_single_title"
        }))
        .style(Style::default().fg(MUTED)),
        sections[0],
    );
    let n = form.values.len();
    let visible = geometry.visible;
    let first = geometry.first;
    for index in first..(first + visible).min(n) {
        let y = sections[1].y + (index - first) as u16 * 3;
        let label = Rect::new(sections[1].x + 1, y, sections[1].width.saturating_sub(2), 1);
        let input = Rect::new(label.x, y + 1, label.width, 1);
        let field = &form.spec.fields[index];
        let active = form.index == index;
        f.render_widget(
            Paragraph::new(format!(
                "{}{}  ({}/{})",
                if active { "› " } else { "  " },
                i.text(&field.label()),
                index + 1,
                n
            )),
            label,
        );
        if field.editable() {
            let (value, cursor) =
                form.values[index].visible(input.width.saturating_sub(2) as usize);
            f.render_widget(
                Paragraph::new(format!(" {value}")).style(if active {
                    highlight(true)
                } else {
                    Style::default().fg(TEXT).bg(PANEL_ALT)
                }),
                input,
            );
            if active {
                f.set_cursor_position((
                    input.x + 1 + cursor.min(input.width.saturating_sub(2)),
                    input.y,
                ));
            }
        } else {
            f.render_widget(
                Paragraph::new(format!(" ‹ {} ›", form.values[index].text)).style(if active {
                    highlight(true)
                } else {
                    Style::default().fg(BLUE)
                }),
                input,
            );
        }
    }
    if let Some(error) = &form.error {
        f.render_widget(
            Paragraph::new(error.as_str())
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(RED)),
            sections[2],
        );
    }
    let primary = i.text(if form.mode == FormMode::Optimization {
        "ui.save_selection"
    } else {
        "ui.continue"
    });
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  [ {primary} ]  "),
                if form.index == n {
                    highlight(true)
                } else {
                    Style::default().fg(TEAL)
                },
            ),
            Span::styled(
                format!("  [ {} ]  ", i.text("ui.cancel")),
                if form.index == n + 1 {
                    highlight(true)
                } else {
                    Style::default()
                },
            ),
        ])),
        sections[3],
    );
    f.render_widget(
        Paragraph::new(i.text("ui.form_hint")).wrap(Wrap { trim: true }),
        sections[4],
    );
}
fn draw_confirmation(app: &App, f: &mut Frame) {
    let Some(confirm) = &app.confirmation else {
        return;
    };
    let i = &app.ctx.i18n;
    let geometry = ConfirmationGeometry::new(f.area(), i);
    let rect = geometry.popup;
    f.render_widget(Clear, rect);
    let frame = block(i.text("ui.confirm"), true);
    f.render_widget(frame, rect);
    let sections = geometry.sections;
    f.render_widget(
        Paragraph::new(confirm.text.as_str())
            .wrap(Wrap { trim: false })
            .scroll((confirm.scroll, 0)),
        sections[0],
    );
    if confirm.token == "YES" {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("  [ {} ]  ", i.text("ui.cancel")),
                    highlight(!confirm.accept),
                ),
                Span::styled(
                    format!("  [ {} ]  ", i.text("ui.confirm_execute")),
                    highlight(confirm.accept),
                ),
            ])),
            sections[1],
        );
        f.render_widget(
            Paragraph::new(i.text("ui.yes_no_hint")).wrap(Wrap { trim: true }),
            sections[2],
        );
    } else {
        let (value, cursor) = confirm
            .input
            .visible(sections[1].width.saturating_sub(3) as usize);
        f.render_widget(
            Paragraph::new(format!(
                "{}\n› {value}",
                i.format("ui.type_confirm", &[("token", &confirm.token)])
            ))
            .wrap(Wrap { trim: true }),
            sections[1],
        );
        f.set_cursor_position((sections[1].x + 2 + cursor, sections[1].y + 1));
        f.render_widget(
            Paragraph::new(i.text("ui.confirm_hint")).wrap(Wrap { trim: true }),
            sections[2],
        );
    }
}
fn overview_lines(app: &App) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let mut lines = Vec::new();
    for row in metric_rows(app) {
        for (key, value) in [row.0, row.1] {
            lines.push(Line::from(format!("{}: {value}", i.text(key))));
        }
    }
    if let Some(at) = app.snapshot.public.updated {
        lines.push(Line::from(format!(
            "{}: {}s",
            i.text("dash.public_age"),
            at.elapsed().as_secs()
        )));
    }
    let data = &app.snapshot.local;
    let mut errors = Vec::new();
    for (key, value) in [
        ("dash.host", &data.host),
        ("dash.os", &data.os),
        ("dash.kernel_status", &data.kernel),
        ("dash.cpu", &data.cpu),
        ("dash.timezone", &data.timezone),
        ("dash.network", &data.congestion),
        ("dash.qdisc", &data.qdisc),
        ("dash.ipv4", &app.snapshot.public.ipv4),
        ("dash.ipv6", &app.snapshot.public.ipv6),
        ("dash.isp", &app.snapshot.public.isp),
    ] {
        if let Reading::Unavailable(error) = value {
            errors.push(format!("{}: {error}", i.text(key)));
        }
    }
    if !errors.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            i.text("dash.diagnostics").to_owned(),
            Style::default().fg(AMBER),
        ));
        lines.extend(errors.into_iter().map(Line::from));
    }
    lines
}
pub(super) fn draw(app: &App, f: &mut Frame) {
    let i = &app.ctx.i18n;
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(CANVAS)), area);
    if area.width < 80 || area.height < 24 {
        f.render_widget(
            Paragraph::new(i.text("ui.small"))
                .style(Style::default().fg(TEXT).bg(CANVAS))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let regions = Regions::new(area, app);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" CqShell {} ", env!("CARGO_PKG_VERSION")),
                Style::default()
                    .fg(TEAL)
                    .bg(PANEL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}  /  {}",
                    i.text(&format!("group.{}", super::GROUPS[app.group])),
                    i.locale()
                ),
                Style::default().fg(MUTED).bg(PANEL),
            ),
        ]))
        .style(Style::default().bg(PANEL)),
        regions.header,
    );
    let environment = if app.platform_error.is_some() {
        i.text("ui.preview_banner")
    } else if app.root {
        i.text("ui.root_banner")
    } else {
        i.text("ui.user_banner")
    };
    f.render_widget(
        Paragraph::new(format!("  {}", environment)).style(
            Style::default()
                .fg(if app.platform_error.is_some() {
                    AMBER
                } else {
                    MUTED
                })
                .bg(CANVAS),
        ),
        regions.environment,
    );
    draw_dashboard(app, f, regions.dashboard);
    draw_lists(app, f, &regions);
    if regions.logs.height > 0 {
        draw_logs(app, f, regions.logs);
    }
    let mut lines = vec![
        Line::from(i.text(if app.optimization() {
            "ui.optimization_hint"
        } else {
            "ui.hint"
        })),
        Line::from(i.text("ui.status_hint")),
    ];
    if app.phase != Phase::Idle {
        lines.push(Line::styled(
            format!(
                "{} · {}{}",
                app.task_title(),
                i.text(app.phase.key()),
                if app.worker.is_some() {
                    format!(" · {}", i.text("ui.busy_hint"))
                } else {
                    String::new()
                }
            ),
            Style::default().fg(phase_color(app.phase)),
        ));
    } else {
        lines.push(Line::styled(
            i.text(if app.optimization() {
                "ui.optimization_details"
            } else {
                "dash.refresh_hint"
            }),
            Style::default().fg(MUTED),
        ));
    }
    f.render_widget(
        Paragraph::new(lines).style(Style::default().fg(TEXT).bg(CANVAS)),
        regions.footer,
    );
    if app.overview {
        let rect = popup(area, 112, area.height.saturating_sub(2));
        f.render_widget(Clear, rect);
        let frame = block(i.text("dash.title"), true);
        let inside = frame.inner(rect);
        f.render_widget(frame, rect);
        let rows = Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).split(inside);
        f.render_widget(
            Paragraph::new(overview_lines(app))
                .wrap(Wrap { trim: false })
                .scroll((app.overview_scroll, 0)),
            rows[0],
        );
        f.render_widget(
            Paragraph::new(i.text("ui.overview_hint")).wrap(Wrap { trim: true }),
            rows[1],
        );
    }
    if app.log_modal {
        let rect = popup(
            area,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        f.render_widget(Clear, rect);
        draw_logs(app, f, rect);
    }
    if let Some(notice) = &app.notice {
        let rect = popup(area, 94, 14);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(format!("{notice}\n\n{}", i.text("ui.back_hint")))
                .wrap(Wrap { trim: true })
                .block(block(i.text("ui.notice"), true)),
            rect,
        );
    }
    draw_form(app, f);
    draw_confirmation(app, f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Paths, runtime::Context};
    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .replace(' ', "")
    }
    #[test]
    fn dashboard_is_permanent_and_checkboxes_belong_only_to_optimization() {
        let mut app = App::new(Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some("en-US"), None).unwrap(),
            false,
        ));
        for (width, height) in [(80, 24), (120, 40), (200, 55)] {
            let text = render(&app, width, height);
            assert!(text.contains("Host:"));
            assert!(text.contains("CPU:"));
            assert!(text.contains("Kernelstatus:"));
            assert!(text.contains("Terminalproxy:"));
            assert!(text.contains("IPv4:"));
            assert!(!text.contains("[x]"));
            assert!(!text.contains("[]"));
        }
        app.group = super::super::OPTIMIZATION_GROUP;
        let text = render(&app, 120, 40);
        assert!(text.contains("[]"));
        assert!(text.contains("Selected:0"));
    }
    #[test]
    fn network_summary_collapses_linux_only_fields_on_other_platforms() {
        let mut app = App::new(Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some("en-US"), None).unwrap(),
            false,
        ));
        app.snapshot.local.congestion = Reading::NotApplicable;
        app.snapshot.local.qdisc = Reading::NotApplicable;
        app.snapshot.local.bbr_available = Reading::NotApplicable;
        app.snapshot.local.bbr_version = Reading::NotApplicable;

        let summary = metric_rows(&app)
            .into_iter()
            .flat_map(|row| [row.0, row.1])
            .find_map(|(key, value)| (key == "dash.network").then_some(value))
            .unwrap();
        assert_eq!(summary, app.ctx.i18n.text("dash.na"));

        app.snapshot.local.congestion = Reading::Value("bbr".into());
        app.snapshot.local.qdisc = Reading::Value("fq".into());
        app.snapshot.local.bbr_available = Reading::Value(true);
        app.snapshot.local.bbr_version = Reading::Value("v3".into());
        assert_eq!(network_summary(&app), "bbr / fq / Enabled · available / v3");
    }
    #[test]
    fn logs_expand_with_the_terminal_without_crowding_navigation() {
        let mut app = App::new(Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some("en-US"), None).unwrap(),
            false,
        ));
        assert_eq!(Regions::new(Rect::new(0, 0, 160, 50), &app).logs.height, 0);
        app.logs_open = true;
        app.log("working");
        let large = Regions::new(Rect::new(0, 0, 160, 50), &app);
        assert!(large.logs.height >= 20);
        assert!(large.categories.height >= 10);
        for height in [34, 40, 50, 80] {
            let regions = Regions::new(Rect::new(0, 0, 160, height), &app);
            assert!(regions.logs.height >= 9);
            assert!(regions.categories.height >= 10);
            assert_eq!(regions.dashboard.height, 10);
            assert_eq!(regions.footer.height, 3);
        }
        assert_eq!(Regions::new(Rect::new(0, 0, 80, 24), &app).logs.height, 0);
        app.logs_open = false;
        assert_eq!(Regions::new(Rect::new(0, 0, 160, 50), &app).logs.height, 0);
    }
    #[test]
    fn long_logs_wrap_without_losing_unicode_or_tail_content() {
        let original = "路径/very/long/path/错误诊断";
        let rows = log_rows(&std::collections::VecDeque::from([original.into()]), 8);
        assert_eq!(rows.concat(), original);
        assert!(rows.iter().all(|row| row.width() <= 8));
    }
    #[test]
    fn all_parameter_forms_keep_submit_and_cancel_visible_at_minimum_size() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = App::new(Context::new(
                Paths::from_env().unwrap(),
                I18n::load(Some(locale), None).unwrap(),
                false,
            ));
            for spec in crate::actions::specs()
                .into_iter()
                .filter(|s| !s.fields.is_empty())
            {
                let values = spec
                    .fields
                    .iter()
                    .map(|f| super::super::InputBuffer::new(f.default.into()))
                    .collect::<Vec<_>>();
                let index = values.len();
                app.form = Some(super::super::Form {
                    spec,
                    values,
                    index,
                    mode: FormMode::Run,
                    error: None,
                });
                let screen = render(&app, 80, 24);
                assert!(screen.contains(&app.ctx.i18n.text("ui.continue").replace(' ', "")));
                assert!(screen.contains(&app.ctx.i18n.text("ui.cancel").replace(' ', "")));
            }
        }
    }
}
