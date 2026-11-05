//! Responsive drawing and mouse geometry. Layout and hierarchy live here; tokens in `theme`,
//! the status panel in `dashboard`, modal layers in `dialogs` and task output in `logs`.
mod dashboard;
mod dialogs;
mod logs;
mod theme;

use super::{App, Focus, Phase, Tone};
use crate::actions::Spec;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Borders, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) use dashboard::{gauge_targets, pending};
pub(super) use dialogs::{ConfirmationGeometry, FormGeometry, OverviewGeometry, PickerGeometry};
pub(super) use logs::{log_body, log_row_count};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const CATEGORY_WIDTH: u16 = 20;
const ACTION_WIDTH: u16 = 44;
/// Actions and details sit side by side from this width; below it they share one panel.
const SIDE_BY_SIDE: u16 = 112;
const MIN_BODY: u16 = 12;
const MIN_LOGS: u16 = 8;

pub(super) fn fits(area: Rect) -> bool {
    area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT
}
pub(super) fn list_offset(selected: usize, count: usize, height: usize) -> usize {
    selected
        .saturating_sub(height.saturating_sub(1))
        .min(count.saturating_sub(height))
}
/// Pad to display cells; wide glyphs count twice.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}
/// Clip to display cells with an ellipsis.
fn clip(text: &str, width: usize) -> String {
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
        if used + n > width - 1 {
            break;
        }
        result.push(c);
        used += n;
    }
    result.push('…');
    result
}
/// Hard wrap by display cells, keeping every character (logs and long values).
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push(c);
        used += cells;
    }
    rows.push(row);
    rows
}
/// Word wrap for prose: Latin words move whole to the next line, while text containing wide
/// (CJK) characters may break between any two characters, as Chinese typesetting allows.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for paragraph in text.split('\n') {
        let mut row = String::new();
        for word in paragraph.split(' ') {
            let gap = usize::from(!row.is_empty());
            if row.width() + gap + word.width() <= width {
                if gap == 1 {
                    row.push(' ');
                }
                row.push_str(word);
                continue;
            }
            let breakable = word.chars().any(|c| c.width().unwrap_or(0) > 1);
            let first = word.chars().next().map_or(0, |c| c.width().unwrap_or(0));
            if breakable && row.width() + gap + first <= width {
                if gap == 1 {
                    row.push(' ');
                }
            } else if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            for c in word.chars() {
                if row.width() + c.width().unwrap_or(0) > width && !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                row.push(c);
            }
        }
        rows.push(row);
    }
    rows
}
/// Centered rect clamped to the area with a one-cell margin.
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
/// The full-screen log view keeps a thin margin so it still reads as a layer.
fn popup_full(area: Rect) -> Rect {
    popup(
        area,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
}

/// Screen regions shared by drawing and mouse hit tests. `*_rows` are the list interiors.
pub(super) struct Regions {
    pub header: Rect,
    pub dashboard: Rect,
    pub categories: Rect,
    pub category_rows: Rect,
    pub actions: Rect,
    pub action_rows: Rect,
    pub details: Rect,
    pub logs: Rect,
    pub footer: Rect,
    /// Narrow screens join actions and details into one panel split by a rule.
    pub stacked: bool,
}
impl Regions {
    pub fn new(area: Rect, app: &App) -> Self {
        let status_height = dashboard::HEIGHT;
        let content = area.height.saturating_sub(status_height + 2);
        // Logs share the space under the dashboard only while navigation keeps MIN_BODY rows.
        let wanted = app.logs_open && (app.worker.is_some() || !app.logs.is_empty());
        let share = ((u32::from(content) * 3 / 5) as u16).min(content.saturating_sub(MIN_BODY));
        let log_height = if wanted && share >= MIN_LOGS {
            share
        } else {
            0
        };
        let [header, status, body, logs, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(status_height),
            Constraint::Min(6),
            Constraint::Length(log_height),
            Constraint::Length(1),
        ])
        .areas(area);
        let [categories, main] =
            Layout::horizontal([Constraint::Length(CATEGORY_WIDTH), Constraint::Min(30)])
                .areas(body);
        let stacked = area.width < SIDE_BY_SIDE;
        let (actions, details, action_rows) = if stacked {
            let rows = (main.height.saturating_sub(6) / 2).clamp(3, 8);
            let [actions, details] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(rows + 2)]).areas(main);
            let inner = Rect::new(
                actions.x + 1,
                actions.y + 1,
                actions.width.saturating_sub(2),
                actions.height.saturating_sub(1),
            );
            (actions, details, inner)
        } else {
            let [actions, details] =
                Layout::horizontal([Constraint::Length(ACTION_WIDTH), Constraint::Min(25)])
                    .areas(main);
            (actions, details, actions.inner(Margin::new(1, 1)))
        };
        Self {
            header,
            dashboard: status,
            categories,
            category_rows: categories.inner(Margin::new(1, 1)),
            actions,
            action_rows,
            details,
            logs,
            footer,
            stacked,
        }
    }
}

/// Any layer that owns input; hover and background highlights pause while one is open.
fn dialog_open(app: &App) -> bool {
    app.confirmation.is_some()
        || app.form.is_some()
        || app.loading.is_some()
        || app.notice.is_some()
        || app.overview
        || app.log_modal
        || app.help
}
/// List row under the pointer, for hover feedback on clickable rows.
fn hover_row(app: &App, rows: Rect, offset: usize, count: usize) -> Option<usize> {
    let point = app.hover.filter(|_| !dialog_open(app))?;
    if !rows.contains(point) {
        return None;
    }
    let index = offset + usize::from(point.y - rows.y);
    (index < count).then_some(index)
}

fn draw_header(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let (mode, banner, tone) = if app.platform_error.is_some() {
        ("ui.mode_preview", "ui.preview_banner", Tone::Warning)
    } else if app.root {
        ("ui.mode_root", "ui.root_banner", Tone::Danger)
    } else {
        ("ui.mode_user", "ui.user_banner", Tone::Info)
    };
    let language = i
        .languages()
        .into_iter()
        .find(|(id, _)| *id == i.locale())
        .map_or(i.locale(), |(_, name)| name);
    let right = format!("{language} ");
    let mut spans = vec![
        Span::styled(
            " CqShell",
            theme::fg(Tone::Accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" v{}  ", env!("CARGO_PKG_VERSION")), theme::muted()),
        theme::badge(i.text(mode), tone),
    ];
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    let banner = format!("  {}", i.text(banner));
    // The explanation is dropped, not clipped, when it does not fit beside the language.
    if used + banner.width() + right.width() + 2 <= area.width as usize {
        spans.push(Span::styled(banner, theme::muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    f.render_widget(
        Paragraph::new(Line::styled(right, theme::muted()).right_aligned()),
        area,
    );
}

/// Focused lists show a filled bar; an unfocused list keeps its place with bold accent text.
fn selection(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(theme::ON_TONE)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        theme::fg(Tone::Accent).add_modifier(Modifier::BOLD)
    }
}
fn list(items: Vec<ListItem<'static>>, focused: bool) -> List<'static> {
    List::new(items)
        .highlight_style(selection(focused))
        .highlight_symbol("› ")
        .highlight_spacing(HighlightSpacing::Always)
}
fn draw_categories(app: &App, f: &mut Frame, regions: &Regions) {
    let i = &app.ctx.i18n;
    let focused = app.focus == Focus::Categories;
    let rows = regions.category_rows;
    let count = app.category_order.len();
    let position = app.category_position();
    let offset = list_offset(position, count, rows.height as usize);
    let hover = hover_row(app, rows, offset, count);
    let items = app
        .category_order
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let mut spans = vec![Span::raw(
                i.text(&format!("group.{}", super::GROUPS[*group]))
                    .to_owned(),
            )];
            if *group == super::OPTIMIZATION_GROUP && !app.marked.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", app.marked.len()),
                    theme::fg(Tone::Accent).add_modifier(Modifier::BOLD),
                ));
            }
            let style = if hover == Some(index) {
                theme::fg(Tone::Accent)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();
    let mut state = ListState::default()
        .with_selected(Some(position))
        .with_offset(offset);
    f.render_stateful_widget(
        list(items, focused).block(theme::panel(i.text("ui.categories"), focused)),
        regions.categories,
        &mut state,
    );
    theme::scrollbar(
        f,
        theme::track(regions.categories, rows),
        count,
        rows.height as usize,
        offset,
    );
}
/// The running action spins in place; a finished single action keeps its outcome glyph.
fn task_marker(app: &App, spec: &Spec) -> Option<Span<'static>> {
    let runs = app.task_ids.iter().any(|id| id == spec.id);
    let live = matches!(app.phase, Phase::Running | Phase::Cancelling);
    let settled = matches!(
        app.phase,
        Phase::Completed | Phase::Failed | Phase::Cancelled
    );
    // A batch outcome belongs to the whole selection, so only single runs keep a glyph.
    if !runs || !(live || (settled && app.task_ids.len() == 1)) {
        return None;
    }
    Some(Span::styled(
        format!(" {}", theme::phase_icon(app.phase, app.elapsed())),
        theme::fg(app.phase.tone()),
    ))
}
fn draw_actions(app: &App, f: &mut Frame, regions: &Regions, specs: &[Spec]) {
    let i = &app.ctx.i18n;
    let focused = app.focus == Focus::Actions;
    let rows = regions.action_rows;
    let offset = list_offset(app.selected(), specs.len(), rows.height as usize);
    let hover = hover_row(app, rows, offset, specs.len());
    let items = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let available = app.available(spec).is_ok();
            let mut spans = Vec::new();
            if app.optimization() {
                let marked = app.marked.contains(spec.id);
                spans.push(Span::styled(
                    if marked { "[✓] " } else { "[ ] " },
                    theme::fg(if marked { Tone::Accent } else { Tone::Muted }),
                ));
            }
            let label = i.text(&spec.label()).to_owned();
            spans.push(if available {
                Span::raw(label)
            } else {
                Span::styled(label, theme::muted())
            });
            spans.extend(task_marker(app, spec));
            let style = if hover == Some(index) {
                theme::fg(Tone::Accent)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();
    let heading = if app.optimization() {
        i.format(
            "ui.optimization_title",
            &[("count", &app.marked.len().to_string())],
        )
    } else {
        i.text(&format!("group.{}", super::GROUPS[app.group]))
            .to_owned()
    };
    let mut block = theme::panel(heading, focused).title(
        Line::styled(
            format!(" {}/{} ", app.selected() + 1, specs.len()),
            theme::muted(),
        )
        .right_aligned(),
    );
    if regions.stacked {
        block = block.borders(Borders::TOP | Borders::LEFT | Borders::RIGHT);
    }
    let mut state = ListState::default()
        .with_selected(Some(app.selected()))
        .with_offset(offset);
    f.render_stateful_widget(
        list(items, focused).block(block),
        regions.actions,
        &mut state,
    );
    theme::scrollbar(
        f,
        theme::track(regions.actions, rows),
        specs.len(),
        rows.height as usize,
        offset,
    );
}
/// Impact and privilege on one line, toned by risk.
fn meta_line(app: &App, spec: &Spec) -> Line<'static> {
    let i = &app.ctx.i18n;
    let (impact, impact_tone) = if spec.changes {
        ("ui.mutating", Tone::Warning)
    } else {
        ("ui.readonly", Tone::Success)
    };
    let (access, access_tone) = if spec.root {
        ("ui.root", Tone::Warning)
    } else {
        ("ui.user", Tone::Muted)
    };
    Line::from(vec![
        Span::styled(format!("● {}", i.text(impact)), theme::fg(impact_tone)),
        Span::styled(" · ", theme::muted()),
        Span::styled(i.text(access).to_owned(), theme::fg(access_tone)),
    ])
}
fn unavailable_line(app: &App, spec: &Spec) -> Option<Line<'static>> {
    let error = app.available(spec).err()?;
    Some(Line::styled(
        format!("! {}", app.ctx.i18n.error(&error)),
        theme::fg(Tone::Warning),
    ))
}
/// Under the action list there is room for the essentials only: why it is unavailable,
/// its impact, then as much description as fits.
fn compact_details(app: &App, spec: &Spec) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let mut lines = Vec::new();
    lines.extend(unavailable_line(app, spec));
    lines.push(meta_line(app, spec));
    lines.push(Line::from(i.text(&spec.description()).to_owned()));
    lines
}
/// The side panel reads top-down: what it is, whether it can run, what it does, its impact,
/// then the parameters it will use.
fn full_details(app: &App, spec: &Spec) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let mut lines = vec![Line::styled(
        i.text(&spec.label()).to_owned(),
        theme::bold(),
    )];
    if let Some(line) = unavailable_line(app, spec) {
        lines.push(Line::default());
        lines.push(line);
    }
    lines.push(Line::default());
    lines.push(Line::from(i.text(&spec.description()).to_owned()));
    lines.push(Line::default());
    lines.push(meta_line(app, spec));
    if spec.external {
        lines.push(Line::styled(
            i.text("ui.external").to_owned(),
            theme::muted(),
        ));
    }
    if !spec.fields.is_empty() {
        let parameters = app
            .parameters
            .get(spec.id)
            .cloned()
            .unwrap_or_else(|| spec.defaults());
        let width = spec
            .fields
            .iter()
            .map(|field| i.text(&field.label()).width())
            .max()
            .unwrap_or(0);
        lines.push(Line::default());
        lines.push(Line::styled(
            i.text("ui.parameters").to_owned(),
            theme::muted(),
        ));
        for field in &spec.fields {
            let value = &parameters[field.name];
            // A listed value is picked when the form opens, so it is not missing yet.
            let (text, tone) = if !value.is_empty() {
                (value.clone(), Tone::Plain)
            } else if field.source.is_some() {
                (i.text("ui.choose_on_open").to_owned(), Tone::Muted)
            } else if field.required {
                (i.text("ui.not_set").to_owned(), Tone::Warning)
            } else {
                ("—".to_owned(), Tone::Muted)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", pad(i.text(&field.label()), width)),
                    theme::muted(),
                ),
                Span::styled(text, theme::fg(tone)),
            ]));
        }
    }
    if app.optimization() {
        lines.push(Line::default());
        lines.push(Line::styled(
            i.text("ui.optimization_details").to_owned(),
            theme::fg(Tone::Info),
        ));
    }
    lines
}
fn draw_details(app: &App, f: &mut Frame, regions: &Regions, spec: &Spec) {
    let title = app.ctx.i18n.text("ui.details");
    let (block, lines) = if regions.stacked {
        // The rule continues the action panel's sides, so both read as one panel.
        let joined = border::Set {
            top_left: "├",
            top_right: "┤",
            ..border::ROUNDED
        };
        (
            theme::panel(title, false)
                .border_set(joined)
                .padding(Padding::horizontal(1)),
            compact_details(app, spec),
        )
    } else {
        (
            theme::panel(title, false).padding(Padding::horizontal(1)),
            full_details(app, spec),
        )
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        regions.details,
    );
}

/// Keys for the current moment, by priority; `?` keeps the full reference one key away.
fn footer_hints(app: &App) -> (Vec<theme::Hint<'_>>, theme::Hint<'_>) {
    let t = |key: &str| app.ctx.i18n.text(key);
    let help = ("?", t("key.help"));
    let back = ("Esc", t("key.back"));
    if app.help {
        return (Vec::new(), ("Esc", t("key.close")));
    }
    if app.worker.is_some() {
        let hints = vec![
            ("Esc", t("key.cancel_task")),
            ("F2", t("key.full_log")),
            ("G", t("key.logs")),
            ("Q", t("key.quit")),
        ];
        return (hints, help);
    }
    if app.log_modal {
        return (vec![("PgUp/PgDn", t("key.scroll"))], back);
    }
    if app.overview {
        let hints = vec![("R", t("key.refresh")), ("N", t("key.public"))];
        return (hints, back);
    }
    let hints = if app.optimization() {
        vec![
            ("Space", t("key.select")),
            ("Enter", t("key.configure")),
            ("F5", t("key.review")),
            ("Tab", t("key.focus")),
            ("Q", t("key.quit")),
        ]
    } else if app.focus == Focus::Categories {
        vec![
            ("↑↓", t("key.move")),
            ("Enter", t("key.open")),
            ("Alt+↑↓", t("key.reorder")),
            ("O", t("key.optimize")),
            ("Q", t("key.quit")),
        ]
    } else {
        vec![
            ("↑↓", t("key.move")),
            ("Enter", t("key.open")),
            ("Tab", t("key.focus")),
            ("O", t("key.optimize")),
            ("R", t("key.refresh")),
            ("Q", t("key.quit")),
        ]
    };
    (hints, help)
}
/// Task state for screens too short for the inline log panel: phase, task and the latest
/// output while it runs; the outcome stays until the next task starts.
fn footer_status(app: &App, logs_visible: bool, room: usize) -> Option<Line<'static>> {
    if app.phase == Phase::Idle || logs_visible {
        return None;
    }
    let i = &app.ctx.i18n;
    let tone = app.phase.tone();
    let mut text = format!(
        "{} {} · {}",
        theme::phase_icon(app.phase, app.elapsed()),
        i.text(app.phase.key()),
        app.task_title()
    );
    let latest = app
        .logs
        .back()
        .filter(|_| app.phase == Phase::Running && !app.log_modal);
    if let Some(line) = latest {
        text = format!("{text} · {}", line.text.trim());
    }
    Some(Line::styled(clip(&text, room), theme::fg(tone)).right_aligned())
}
fn draw_footer(app: &App, f: &mut Frame, regions: &Regions) {
    let area = regions.footer.inner(Margin::new(1, 0));
    let status = footer_status(app, regions.logs.height > 0, area.width as usize / 2);
    let status_width = status.as_ref().map_or(0, Line::width) as u16;
    let room = area
        .width
        .saturating_sub(status_width + u16::from(status_width > 0) * 2);
    let (hints, pinned) = footer_hints(app);
    f.render_widget(
        Paragraph::new(theme::key_hints(&hints, Some(pinned), room as usize)),
        area,
    );
    if let Some(status) = status {
        f.render_widget(Paragraph::new(status), area);
    }
}

fn draw_small(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let width = area.width.saturating_sub(4).max(1) as usize;
    let mut lines = wrap_words(i.text("ui.small"), width)
        .into_iter()
        .map(|row| Line::styled(row, theme::fg(Tone::Warning)))
        .collect::<Vec<_>>();
    lines.push(Line::styled(
        i.format(
            "ui.current_size",
            &[
                ("width", &area.width.to_string()),
                ("height", &area.height.to_string()),
            ],
        ),
        theme::muted(),
    ));
    let height = (lines.len() as u16).min(area.height);
    let rect = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    f.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        rect,
    );
}

pub(super) fn draw(app: &App, f: &mut Frame) {
    let area = f.area();
    if !fits(area) {
        draw_small(app, f, area);
        return;
    }
    let regions = Regions::new(area, app);
    let specs = app.items();
    let spec = &specs[app.selected()];
    draw_header(app, f, regions.header);
    dashboard::draw(app, f, regions.dashboard);
    draw_categories(app, f, &regions);
    draw_actions(app, f, &regions, &specs);
    draw_details(app, f, &regions, spec);
    if regions.logs.height > 0 {
        logs::draw(app, f, regions.logs);
    }
    draw_footer(app, f, &regions);
    // Feedback sits under modal layers so a dialog always reads as the top layer.
    dialogs::draw_toast(app, f, regions.footer);
    dialogs::draw_overview(app, f);
    if app.log_modal {
        logs::draw(app, f, popup_full(area));
    }
    dialogs::draw_help(app, f);
    dialogs::draw_notice(app, f);
    dialogs::draw_loading(app, f);
    dialogs::draw_form(app, f);
    dialogs::draw_picker(app, f);
    dialogs::draw_confirmation(app, f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Paths,
        dashboard::Reading,
        i18n::I18n,
        ops::{Choice, clash::State},
        runtime::Context,
        tui::{Confirmation, FormMode, InputBuffer, Loading},
    };
    use ratatui::style::Color;

    fn app(locale: &str) -> App {
        App::new(Context::new(
            Paths::from_env().unwrap(),
            I18n::load(Some(locale), None).unwrap(),
            false,
        ))
    }
    fn terminal(
        app: &App,
        width: u16,
        height: u16,
    ) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal
    }
    /// Screen rows with spaces removed; TestBackend leaves a blank after each wide glyph.
    fn rows(app: &App, width: u16, height: u16) -> Vec<String> {
        let terminal = terminal(app, width, height);
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .replace(' ', "")
            })
            .collect()
    }
    fn render(app: &App, width: u16, height: u16) -> String {
        rows(app, width, height).concat()
    }
    /// A fact renders as its label followed by its value (after an optional state dot).
    fn shows(rows: &[String], label: &str, value: &str) -> bool {
        let label = label.replace(' ', "");
        let value = value.replace(' ', "");
        rows.iter().any(|row| {
            row.match_indices(&label).any(|(at, _)| {
                row[at + label.len()..]
                    .trim_start_matches(['●', '○'])
                    .starts_with(&value)
            })
        })
    }

    #[test]
    fn dashboard_is_permanent_and_checkboxes_belong_only_to_optimization() {
        let mut app = app("en-US");
        for (width, height) in [(80, 24), (120, 40), (200, 55)] {
            let text = render(&app, width, height);
            for label in [
                "Host",
                "CPU",
                "Kernelstatus",
                "Terminalproxy",
                "Mihomostatus",
                "Mihomoversion",
                "IPv4",
                "TCPconfig",
                "Memory",
                "Swap",
            ] {
                assert!(text.contains(label), "{label} at {width}x{height}");
            }
            assert!(!text.contains("[✓]"));
            assert!(!text.contains("[]"));
        }
        app.group = super::super::OPTIMIZATION_GROUP;
        let text = render(&app, 120, 40);
        assert!(text.contains("[]"));
        assert!(text.contains("Selected:0"));
    }
    #[test]
    fn proxy_and_mihomo_fields_render_independently_in_both_languages() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = app(locale);
            app.snapshot.local.mihomo_version = Reading::Value("v1.19.12".into());
            for (state, proxy, key) in [
                (State::Running(1234), false, "dash.mihomo_running"),
                (State::Stopped, true, "dash.stopped"),
                (State::NotInstalled, false, "dash.not_installed"),
                (State::Unknown, true, "dash.unknown"),
            ] {
                app.snapshot.local.mihomo_state = Reading::Value(state);
                app.snapshot.local.terminal_proxy = Reading::Value(proxy);
                let i = &app.ctx.i18n;
                let expected_state = i.format(key, &[("pid", "1234")]);
                let expected_proxy = i.text(if proxy {
                    "status.enabled"
                } else {
                    "status.disabled"
                });
                for (width, height) in [(80, 24), (120, 40), (200, 55)] {
                    let screen = rows(&app, width, height);
                    for (label, value) in [
                        ("dash.terminal_proxy", expected_proxy),
                        ("dash.mihomo_state", expected_state.as_str()),
                        ("dash.mihomo_version", "v1.19.12"),
                    ] {
                        assert!(
                            shows(&screen, app.ctx.i18n.text(label), value),
                            "missing {label}={value} at {width}x{height} ({locale})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn network_and_proxy_status_has_its_own_panel() {
        let app = app("en-US");
        for width in [80, 120, 200] {
            let height = 40;
            let terminal = terminal(&app, width, height);
            let buffer = terminal.backend().buffer();
            let dashboard = Regions::new(Rect::new(0, 0, width, height), &app).dashboard;
            let split = width / 2;
            // Column of `label` inside the status rows; en-US cells are all single width.
            let column = |label: &str| {
                (dashboard.y..dashboard.bottom()).find_map(|y| {
                    let row = (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>();
                    row.find(label).map(|at| row[..at].chars().count() as u16)
                })
            };
            for label in ["System & resources", "Host", "Kernel status", "Timezone"] {
                let x = column(label).unwrap_or_else(|| panic!("{label} missing at {width}"));
                assert!(x < split, "{label} belongs to the system panel at {width}");
            }
            for label in [
                "Network & proxy",
                "Congestion / queue",
                "TCP config",
                "IPv4",
                "ISP",
                "Terminal proxy",
                "Mihomo status",
                "Mihomo version",
            ] {
                let x = column(label).unwrap_or_else(|| panic!("{label} missing at {width}"));
                assert!(
                    x >= split,
                    "{label} belongs to the network panel at {width}"
                );
            }
        }
    }

    #[test]
    fn mihomo_probe_errors_remain_visible_in_overview_diagnostics() {
        let mut app = app("en-US");
        app.snapshot.local.mihomo_state = Reading::Unavailable("invalid PID file".into());
        app.snapshot.local.mihomo_version = Reading::Unavailable("version probe failed".into());
        let details = dashboard::overview(&app, 96)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(details.contains("Mihomo status: invalid PID file"));
        assert!(details.contains("Mihomo version: version probe failed"));
        let tone = |app: &App| {
            dashboard::facts(app)
                .into_iter()
                .find(|fact| fact.key == "dash.mihomo_state")
                .unwrap()
                .tone
        };
        assert_eq!(tone(&app), Tone::Warning);
        app.snapshot.local.mihomo_state = Reading::Value(State::Unknown);
        assert_eq!(tone(&app), Tone::Warning);
        let unverified = app.ctx.i18n.text("dash.mihomo_unverified");
        assert!(
            dashboard::overview(&app, 96)
                .iter()
                .any(|line| line.to_string().contains(unverified))
        );
    }

    #[test]
    fn network_summary_collapses_linux_only_fields_on_other_platforms() {
        let mut app = app("en-US");
        app.snapshot.local.congestion = Reading::NotApplicable;
        app.snapshot.local.qdisc = Reading::NotApplicable;
        app.snapshot.local.bbr_available = Reading::NotApplicable;
        app.snapshot.local.bbr_version = Reading::NotApplicable;
        let fact = dashboard::facts(&app)
            .into_iter()
            .find(|fact| fact.key == "dash.network")
            .unwrap();
        assert_eq!(fact.value, app.ctx.i18n.text("dash.na"));
        assert_eq!(fact.tone, Tone::Muted);

        app.snapshot.local.congestion = Reading::Value("bbr".into());
        app.snapshot.local.qdisc = Reading::Value("fq".into());
        app.snapshot.local.bbr_available = Reading::Value(true);
        app.snapshot.local.bbr_version = Reading::Value("v3".into());
        assert_eq!(
            dashboard::network_summary(&app),
            "bbr / fq / Enabled · available / v3"
        );
    }
    #[test]
    fn logs_expand_with_the_terminal_without_crowding_navigation() {
        let mut app = app("en-US");
        assert_eq!(Regions::new(Rect::new(0, 0, 160, 50), &app).logs.height, 0);
        app.logs_open = true;
        app.log("working");
        let large = Regions::new(Rect::new(0, 0, 160, 50), &app);
        assert!(large.logs.height >= 20);
        assert!(large.category_rows.height >= 10);
        for height in [34, 40, 50, 80] {
            let regions = Regions::new(Rect::new(0, 0, 160, height), &app);
            assert!(regions.logs.height >= MIN_LOGS, "{height}");
            assert!(regions.category_rows.height >= 10, "{height}");
            assert_eq!(regions.dashboard.height, dashboard::HEIGHT);
            assert_eq!(regions.footer.height, 1);
        }
        assert_eq!(Regions::new(Rect::new(0, 0, 80, 24), &app).logs.height, 0);
        app.logs_open = false;
        assert_eq!(Regions::new(Rect::new(0, 0, 160, 50), &app).logs.height, 0);
    }
    #[test]
    fn compact_terminals_keep_navigation_and_details_readable() {
        let app = app("zh-CN");
        let compact = Regions::new(Rect::new(0, 0, 80, 24), &app);
        assert!(compact.stacked);
        assert!(compact.action_rows.height >= 6, "{:?}", compact.action_rows);
        assert!(compact.details.height >= 5, "{:?}", compact.details);
        assert!(compact.category_rows.height >= 10);
        let wide = Regions::new(Rect::new(0, 0, 120, 36), &app);
        assert!(!wide.stacked);
        assert_eq!(wide.actions.width, ACTION_WIDTH);
    }
    #[test]
    fn text_wrapping_keeps_unicode_and_breaks_prose_at_spaces() {
        let original = "路径/very/long/path/错误诊断";
        let rows = wrap_cells(original, 8);
        assert_eq!(rows.concat(), original);
        assert!(rows.iter().all(|row| row.width() <= 8));
        assert_eq!(wrap_words("hello wide world", 10), ["hello wide", "world"]);
        assert_eq!(wrap_words("a\nb", 10), ["a", "b"]);
        assert_eq!(
            wrap_words("断开 SSH 连接。选择取消", 12),
            ["断开 SSH 连", "接。选择取消"],
            "CJK text fills the line instead of leaving a gap"
        );
        assert!(
            wrap_words("终端过小请调整", 6)
                .iter()
                .all(|row| row.width() <= 6)
        );
        assert_eq!(clip("abcdef", 4), "abc…");
    }
    #[test]
    fn all_parameter_forms_keep_submit_and_cancel_visible_at_minimum_size() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = app(locale);
            for spec in crate::actions::specs()
                .into_iter()
                .filter(|s| !s.fields.is_empty())
            {
                let values = spec
                    .fields
                    .iter()
                    .map(|f| InputBuffer::new(f.default.into()))
                    .collect::<Vec<_>>();
                let index = values.len();
                app.form = Some(super::super::Form {
                    options: vec![Vec::new(); values.len()],
                    picker: None,
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
    #[test]
    fn canvas_and_form_popup_preserve_terminal_background() {
        let mut app = app("en-US");
        let area = Rect::new(0, 0, 120, 40);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].bg, Color::Reset);
        let background = terminal.backend().buffer().content.clone();

        app.form = Some(
            app.make_form(
                crate::actions::find("clash.sub.add").unwrap(),
                FormMode::Run,
            )
            .unwrap(),
        );
        let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app.ctx.i18n);
        let popup = geometry.popup;
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(popup.x + 1, popup.y + 1)].bg, Color::Reset);
        let cleared_background_text = (popup.y + 1..popup.y + popup.height - 1).any(|y| {
            (popup.x + 1..popup.x + popup.width - 1).any(|x| {
                let index = usize::from(y) * usize::from(area.width) + usize::from(x);
                background[index].symbol() != " " && buffer[(x, y)].symbol() == " "
            })
        });
        assert!(cleared_background_text);
        let hint_cell = &buffer[(popup.x + popup.width - 2, geometry.sections[4].y)];
        assert_eq!(hint_cell.fg, Color::Reset);
        assert_eq!(hint_cell.bg, Color::Reset);
        // The scrim recedes the screen behind the dialog without painting its background.
        assert_eq!(buffer[(0, area.height - 1)].bg, Color::Reset);
        assert_eq!(buffer[(1, 0)].fg, theme::BORDER);
    }
    #[test]
    fn overlays_render_in_both_languages_at_minimum_size() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = app(locale);
            let screen = render(&app, 80, 24);
            let help = format!("?{}", app.ctx.i18n.text("key.help")).replace(' ', "");
            assert!(screen.contains(&help), "the footer always offers help");
            app.help = true;
            let screen = render(&app, 80, 24);
            assert!(screen.contains(&app.ctx.i18n.text("help.title").replace(' ', "")));
            assert!(screen.contains("F5") && screen.contains("Ctrl+C"));
            app.help = false;
            app.overview = true;
            let screen = render(&app, 80, 24);
            assert!(screen.contains(&app.ctx.i18n.text("dash.section_system").replace(' ', "")));
            app.overview = false;
            app.notify(
                app.ctx.i18n.text("toast.order_saved").to_owned(),
                Tone::Success,
            );
            let screen = render(&app, 80, 24);
            assert!(screen.contains(&app.ctx.i18n.text("toast.order_saved").replace(' ', "")));
            let small = render(&app, 40, 10);
            assert!(
                small.contains("80×24") && small.contains("40×10"),
                "{small}"
            );
        }
    }
    /// Text inside `rect` only, so a check cannot match the screen behind an overlay.
    fn region(app: &App, width: u16, height: u16, rect: Rect) -> String {
        let terminal = terminal(app, width, height);
        let buffer = terminal.backend().buffer();
        (rect.y..rect.bottom())
            .flat_map(|y| (rect.x..rect.right()).map(move |x| (x, y)))
            .map(|point| buffer[point].symbol().to_owned())
            .collect::<String>()
            .replace(' ', "")
    }
    #[test]
    fn picker_lists_items_with_context_at_minimum_size() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = app(locale);
            let spec = crate::actions::find("clash.sub.use").unwrap();
            let mut form = app.make_form(spec, FormMode::Run).unwrap();
            let choices = (0..30)
                .map(|n| Choice {
                    value: format!("订阅-{n}"),
                    detail: format!("host{n}.example"),
                    current: n == 20,
                    children: Vec::new(),
                })
                .collect();
            form.attach(vec![choices]);
            form.open_picker(String::new(), true);
            let cursor = form.picker.as_ref().unwrap().cursor;
            assert_eq!(cursor, 20, "the list opens on the current item");
            app.form = Some(form);
            let area = Rect::new(0, 0, 80, 24);
            let geometry = PickerGeometry::new(area, 30, cursor, 30);
            let text = region(&app, area.width, area.height, geometry.popup);
            let i = &app.ctx.i18n;
            let title = format!(
                "{}·{}",
                i.text("action.clash.sub.use"),
                i.text("field.name")
            );
            for expected in [
                title.as_str(),
                "订阅-20",
                "host20.example",
                i.text("ui.current"),
                i.text("ui.filter_hint"),
                i.text("key.pick"),
                "30/30",
            ] {
                assert!(
                    text.contains(&expected.replace(' ', "")),
                    "{expected} in {text}"
                );
            }
            assert!(!text.contains("host0."), "earlier rows scroll out: {text}");
            let rows = geometry.sections[1];
            assert_eq!(
                terminal(&app, area.width, area.height).backend().buffer()[(rows.x, rows.y + 11)]
                    .bg,
                theme::ACCENT,
                "the current item is highlighted"
            );

            app.form.as_mut().unwrap().picker.as_mut().unwrap().filter =
                InputBuffer::new("zzz".into());
            let geometry = PickerGeometry::new(area, 30, cursor, 0);
            let text = region(&app, area.width, area.height, geometry.popup);
            assert!(text.contains(&app.ctx.i18n.text("ui.no_match").replace(' ', "")));
            assert!(text.contains("0/30"), "{text}");

            // Once the list closes, the field shows the chosen item, its place and its context.
            app.form.as_mut().unwrap().picker = None;
            let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app.ctx.i18n);
            let text = region(&app, area.width, area.height, geometry.popup);
            assert!(text.contains("‹订阅-20›21/30host20.example"), "{text}");
            let hint = format!("Enter{}", app.ctx.i18n.text("key.list")).replace(' ', "");
            assert!(text.contains(&hint), "{text}");
        }
    }
    #[test]
    fn an_open_list_replaces_its_form() {
        let mut app = app("zh-CN");
        let spec = crate::actions::find("clash.sub.use").unwrap();
        let mut form = app.make_form(spec, FormMode::Run).unwrap();
        let choice = |value: &str| Choice {
            value: value.into(),
            detail: String::new(),
            current: false,
            children: Vec::new(),
        };
        form.attach(vec![vec![choice("home"), choice("work")]]);
        let area = Rect::new(0, 0, 80, 24);
        let behind = FormGeometry::new(area, &form, &app.ctx.i18n).popup;
        form.open_picker(String::new(), true);
        app.form = Some(form);
        // A short list is lower than its form, so a form drawn behind it would show its
        // titled border above the list.
        let top = Rect::new(behind.x, behind.y, behind.width, 1);
        let title = app.ctx.i18n.text("action.clash.sub.use").replace(' ', "");
        let text = region(&app, area.width, area.height, top);
        assert!(!text.contains(&title), "{text}");
    }
    #[test]
    fn slow_listings_show_a_cancellable_loading_dialog_after_a_delay() {
        let mut app = app("zh-CN");
        let (_tx, receiver) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        app.loading = Some(Loading {
            spec: crate::actions::find("clash.proxy.select").unwrap(),
            mode: FormMode::Run,
            receiver,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            started,
        });
        let loading = app.ctx.i18n.text("ui.loading_choices").replace(' ', "");
        app.now = started;
        assert!(
            !render(&app, 80, 24).contains(&loading),
            "quick reads never flash a dialog"
        );
        app.now = started + std::time::Duration::from_secs(1);
        let screen = render(&app, 80, 24);
        assert!(screen.contains(&loading), "{screen}");
        let title = app
            .ctx
            .i18n
            .text("action.clash.proxy.select")
            .replace(' ', "");
        assert!(screen.contains(&title));
    }
    #[test]
    fn hovering_a_row_highlights_it_without_changing_the_selection() {
        let mut app = app("en-US");
        let area = Rect::new(0, 0, 120, 36);
        let regions = Regions::new(area, &app);
        let point = (regions.action_rows.x + 4, regions.action_rows.y + 1);
        app.hover = Some(point.into());
        let hovered = terminal(&app, area.width, area.height);
        assert_eq!(hovered.backend().buffer()[point].fg, theme::ACCENT);
        assert_eq!(app.selected(), 0);
        app.help = true;
        let covered = terminal(&app, area.width, area.height);
        assert_ne!(covered.backend().buffer()[point].fg, theme::ACCENT);
    }
    #[test]
    fn confirmations_size_to_their_text_and_scroll_when_long() {
        let app = app("en-US");
        let area = Rect::new(0, 0, 80, 24);
        let confirmation = |text: String| Confirmation {
            text,
            token: "YES".into(),
            input: InputBuffer::default(),
            reply: std::sync::mpsc::channel().0,
            scroll: 0,
            accept: false,
        };
        let short =
            ConfirmationGeometry::new(area, &confirmation("Proceed?".into()), &app.ctx.i18n);
        assert!(short.popup.height <= 8, "{:?}", short.popup);
        assert_eq!(short.max_scroll(), 0);
        let long = (0..40)
            .map(|n| format!("item {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let long = ConfirmationGeometry::new(area, &confirmation(long), &app.ctx.i18n);
        assert_eq!(long.popup.height, area.height - 2);
        assert!(long.max_scroll() > 0);
        assert!(long.buttons[0].right() < long.buttons[1].x);
    }
}
