//! Responsive drawing and mouse geometry. Layout and hierarchy live here; tokens in `theme`,
//! the header row in `header`, the status panel in `dashboard`, modal layers in `dialogs`
//! and task output in `logs`.
#[cfg(test)]
mod boundary_tests;
mod dashboard;
mod dialogs;
mod header;
#[cfg(test)]
mod interaction_tests;
mod logs;
#[cfg(test)]
mod pattern_tests;
pub(super) mod plan;
mod style;
mod theme;

use super::{App, Phase, Row, Tone, preview, related};
use crate::{
    actions::{Field, Kind, Parameters, Spec},
    dashboard::Reading,
    ops::clash::State,
    runtime::Outcome,
    theme::{Level, Role, Theme},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Borders, HighlightSpacing, List, ListItem, ListState, Paragraph},
};
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) use dashboard::{gauge_targets, pending};
pub(super) use dialogs::{
    CommandPaletteGeometry, ConfirmationGeometry, FormGeometry, OverviewGeometry, PickerGeometry,
};
pub(super) use logs::{log_body, log_row_count};
pub(super) use style::StyleGeometry;
pub(super) use theme::spinner;

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const CATEGORY_WIDTH: u16 = 20;
const ACTION_WIDTH: u16 = 44;
/// Actions and details sit side by side from this width; below it they share one panel.
const SIDE_BY_SIDE: u16 = 112;
/// Rows outside the content: the header, the Status_Area and the Footer.
const CHROME: u16 = 1 + dashboard::HEIGHT + 1;
/// The action area never gets fewer rows: at 80×24 it takes the whole content, enough for
/// Category_List's 13 rows inside its borders (Requirements 15.1, 15.2).
const MIN_BODY: u16 = 15;
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
    clip_with(text, width, "…")
}
/// Clip to `width` display cells, ending with `mark` when anything was cut. A width too
/// narrow for the mark keeps what fits without it.
fn clip_with(text: &str, width: usize, mark: &str) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let room = width.checked_sub(mark.width());
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = c.width().unwrap_or(0);
        if used + n > room.unwrap_or(width) {
            break;
        }
        result.push(c);
        used += n;
    }
    if room.is_some() {
        result.push_str(mark);
    }
    result
}
/// `spans` clipped to `width` cells: whole spans while they fit, then the first that does
/// not, cut with `mark`; the rest are dropped.
fn clip_spans(spans: Vec<Span<'static>>, width: usize, mark: &str) -> Vec<Span<'static>> {
    let mut used = 0;
    let mut kept = Vec::with_capacity(spans.len());
    for span in spans {
        let cells = span.width();
        if used + cells > width {
            let text = clip_with(&span.content, width - used, mark);
            kept.push(Span::styled(text, span.style));
            break;
        }
        used += cells;
        kept.push(span);
    }
    kept
}
/// `items` left to right after `lead`, with `joint` between two items on the same line.
/// Lines break only between items; each continuation line starts with `indent` blanks, and
/// an item too wide for a line of its own is clipped with `mark`.
fn flow(
    lead: Vec<Span<'static>>,
    items: Vec<Vec<Span<'static>>>,
    joint: &Span<'static>,
    width: usize,
    indent: usize,
    mark: &str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut used = lead.iter().map(Span::width).sum::<usize>();
    let mut line = lead;
    let mut started = false;
    for item in items {
        let cells = item.iter().map(Span::width).sum::<usize>();
        if started {
            if used + joint.width() + cells <= width {
                line.push(joint.clone());
                line.extend(item);
                used += joint.width() + cells;
                continue;
            }
            lines.push(Line::from(std::mem::take(&mut line)));
            line.push(Span::raw(" ".repeat(indent)));
            used = indent;
        }
        started = true;
        line.extend(clip_spans(item, width.saturating_sub(used), mark));
        used += cells;
    }
    lines.push(Line::from(line));
    lines
}
/// The cells `items` take on one line with `joint` between them.
fn flow_width(items: &[Vec<Span<'static>>], joint: &Span<'static>) -> usize {
    let cells = items.iter().flatten().map(Span::width).sum::<usize>();
    cells + joint.width() * items.len().saturating_sub(1)
}
/// A duration as `mm:ss`; the minutes keep counting past the hour.
fn mm_ss(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// A run of text in one style. Status_Area rows, the header and Row_Tags are built as
/// Segment lists, so a line that does not fit loses whole Segments, never half a value.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Segment {
    pub text: String,
    pub style: Style,
}
impl Segment {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
    /// Display cells; wide (CJK) characters count twice.
    pub fn width(&self) -> usize {
        self.text.width()
    }
}
/// Drawn between two Segments of a line.
pub(super) const SEPARATOR: &str = " · ";
/// How many leading `segments` fit in `width` cells, counting the separators between them.
pub(super) fn fit_segments(segments: &[Segment], width: usize) -> usize {
    let separator = SEPARATOR.width();
    let mut used = 0;
    for (index, segment) in segments.iter().enumerate() {
        used += segment.width() + if index == 0 { 0 } else { separator };
        if used > width {
            return index;
        }
    }
    segments.len()
}
/// The leading Segments that fit in `width`, separated by `SEPARATOR` in `separator`.
/// When not even the first fits, it is clipped with an ellipsis and the rest are dropped
/// (Requirements 4.6, 4.7).
pub(super) fn segment_spans(
    segments: &[Segment],
    width: usize,
    separator: Style,
) -> Vec<Span<'static>> {
    let count = fit_segments(segments, width);
    if count == 0 {
        return segments
            .first()
            .map(|first| vec![Span::styled(clip(&first.text, width), first.style)])
            .unwrap_or_default();
    }
    let mut spans = Vec::with_capacity(2 * count - 1);
    for (index, segment) in segments[..count].iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR, separator));
        }
        spans.push(Span::styled(segment.text.clone(), segment.style));
    }
    spans
}

pub(super) fn themed_segment_spans(
    theme: &Theme,
    segments: &[Segment],
    width: usize,
    separator: Style,
) -> Vec<Span<'static>> {
    if theme.patterns.glyphs != crate::theme::GlyphSet::Ascii {
        return segment_spans(segments, width, separator);
    }
    let count = fit_segments(segments, width);
    if count == 0 {
        return segments
            .first()
            .map(|first| {
                vec![Span::styled(
                    clip_with(&first.text, width, ellipsis(theme)),
                    first.style,
                )]
            })
            .unwrap_or_default();
    }
    let mut spans = Vec::new();
    for (index, segment) in segments.iter().take(count).enumerate() {
        if index > 0 {
            spans.push(Span::styled(
                format!(" {} ", theme::glyph(theme, theme::Glyph::Separator)),
                separator,
            ));
        }
        spans.push(Span::styled(segment.text.clone(), segment.style));
    }
    spans
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

/// Rows of the inline log area for `content` rows between the Status_Area and the Footer:
/// three fifths of them, capped so the action area keeps MIN_BODY rows, and none when that
/// leaves fewer than MIN_LOGS (Requirement 15.3). It first appears at a height of 32.
fn inline_log_rows(content: u16) -> u16 {
    let share = ((u32::from(content) * 3 / 5) as u16).min(content.saturating_sub(MIN_BODY));
    if share >= MIN_LOGS { share } else { 0 }
}

/// Screen regions shared by drawing and mouse hit tests (the Main_View's Hit_Geometry).
/// `*_rows` are the list interiors and `*_offset` the first list row each one shows, so a
/// click or hover always lands on the row drawn under the pointer (Requirement 17.10).
pub(super) struct Regions {
    pub header: Rect,
    pub dashboard: Rect,
    pub categories: Rect,
    pub category_rows: Rect,
    pub category_offset: usize,
    pub actions: Rect,
    pub action_rows: Rect,
    pub action_offset: usize,
    pub checkboxes: Rect,
    pub details: Rect,
    pub logs: Rect,
    pub footer: Rect,
    /// Narrow screens join actions and details into one panel split by a rule.
    pub stacked: bool,
    /// Rows in `app.category_rows()` and `app.rows(app.group)`.
    category_count: usize,
    pub rows: Vec<Row>,
}
impl Regions {
    /// Top to bottom: header, Status_Area, action area, the optional inline logs and the
    /// Footer (Requirement 15.1).
    pub fn new(area: Rect, app: &App) -> Self {
        let content = area.height.saturating_sub(CHROME);
        let wanted = app.logs_open && (app.worker.is_some() || !app.logs.is_empty());
        let log_height = if wanted { inline_log_rows(content) } else { 0 };
        let [header, status, body, logs, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(dashboard::HEIGHT),
            Constraint::Fill(1),
            Constraint::Length(log_height),
            Constraint::Length(1),
        ])
        .areas(area);
        let [categories, main] =
            Layout::horizontal([Constraint::Length(CATEGORY_WIDTH), Constraint::Min(30)])
                .areas(body);
        let stacked = area.width < SIDE_BY_SIDE;
        let unframed = theme::unframed(&app.theme.active);
        let (actions, details, action_rows) = if stacked {
            let rows = (main.height.saturating_sub(6) / 2).clamp(3, 8);
            let detail_height = if app.style_panel.is_some() {
                main.height.saturating_sub(3)
            } else {
                app.progress
                    .as_ref()
                    .filter(|_| app.optimization())
                    .map_or(rows + 2, |progress| {
                        (progress.steps.len() as u16 + 5).min(main.height.saturating_sub(3))
                    })
            };
            let [actions, details] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(detail_height)])
                    .areas(main);
            let inner = if unframed {
                theme::panel(&app.theme.active, "", false).inner(actions)
            } else {
                Rect::new(
                    actions.x + 1,
                    actions.y + 1,
                    actions.width.saturating_sub(2),
                    actions.height.saturating_sub(1),
                )
            };
            (actions, details, inner)
        } else {
            let [actions, details] =
                Layout::horizontal([Constraint::Length(ACTION_WIDTH), Constraint::Min(25)])
                    .areas(main);
            let inner = theme::panel(&app.theme.active, "", false).inner(actions);
            (actions, details, inner)
        };
        let category_rows = theme::panel(&app.theme.active, "", false).inner(categories);
        let category_count = app.category_rows().len();
        let rows = app.rows(app.group);
        let action_count = rows.len();
        Self {
            header,
            dashboard: status,
            categories,
            category_rows,
            category_offset: list_offset(
                app.category_row(),
                category_count,
                category_rows.height as usize,
            ),
            actions,
            action_rows,
            checkboxes: Rect::new(
                action_rows.x + 2,
                action_rows.y,
                theme::glyph(&app.theme.active, theme::Glyph::Unchecked).width() as u16,
                action_rows.height,
            ),
            action_offset: list_offset(
                super::row_index(&rows, app.selected()),
                action_count,
                action_rows.height as usize,
            ),
            details,
            logs,
            footer,
            stacked,
            category_count,
            rows,
        }
    }
    pub fn selected_row(&self, app: &App) -> &Row {
        &self.rows[super::row_index(&self.rows, app.selected())]
    }
    /// Index into `app.category_rows()` of the Category_List row under `point`.
    pub fn category_at(&self, point: Position) -> Option<usize> {
        row_at(
            self.category_rows,
            self.category_offset,
            self.category_count,
            point,
        )
    }
    /// Index into `app.rows(app.group)` of the Action_List row under `point`.
    pub fn action_at(&self, point: Position) -> Option<usize> {
        row_at(self.action_rows, self.action_offset, self.rows.len(), point)
    }
}
/// The row of a list scrolled to `offset` that is drawn under `point`.
fn row_at(rows: Rect, offset: usize, count: usize, point: Position) -> Option<usize> {
    if !rows.contains(point) {
        return None;
    }
    let index = offset + usize::from(point.y - rows.y);
    (index < count).then_some(index)
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
        || app.command_palette.is_some()
}
/// The pointer, for hover feedback on clickable rows, while no layer above owns input.
fn pointer(app: &App) -> Option<Position> {
    app.hover
        .filter(|_| !dialog_open(app) && app.style_panel.is_none())
}

/// The selected row of any list: a filled bar in the selection text color. Navigation has
/// no focus, so every list draws its selection the same way. `fill` is the selection fill
/// of the surface the list sits on.
fn selection(t: &Theme, fill: Role) -> Style {
    use crate::theme::SelectionStyle;
    let mut style = Style::default().add_modifier(Modifier::BOLD);
    if t.patterns.selection != SelectionStyle::Arrow {
        style = style.bg(theme::color(t, fill));
    }
    if matches!(
        t.patterns.selection,
        SelectionStyle::Fill | SelectionStyle::Inverse
    ) {
        style = style.fg(theme::color(t, Role::SelectionText));
    }
    style
}
fn list(t: &Theme, items: Vec<ListItem<'static>>, highlight: Style) -> List<'static> {
    List::new(items)
        .highlight_style(highlight)
        .highlight_symbol(theme::selection_marker(t))
        .highlight_spacing(HighlightSpacing::Always)
}
/// The digit that selects the category at `position` in `display_order()`: 0 for the pinned
/// Optimization_Category, 1–9 for the first nine Reorderable_Categories, a blank of the
/// same width for the rest.
fn category_digit(position: usize) -> String {
    if position <= 9 {
        position.to_string()
    } else {
        " ".into()
    }
}
fn draw_categories(app: &App, f: &mut Frame, regions: &Regions) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let rows = regions.category_rows;
    let entries = app.category_rows();
    let count = entries.len();
    let selected = app.category_row();
    let offset = regions.category_offset;
    let hover = pointer(app)
        .and_then(|point| regions.category_at(point))
        .filter(|index| entries[*index].is_some());
    // The rule spans the row after the two-cell selection marker.
    let rule_width = (rows.width as usize).saturating_sub(2);
    let mut position = 0;
    let items = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let Some(group) = entry else {
                return ListItem::new(Line::from(theme::rule(t, rule_width)));
            };
            let mut spans = vec![
                Span::styled(
                    format!("{} ", category_digit(position)),
                    theme::text(t, Level::Tertiary),
                ),
                Span::styled(
                    i.text(&format!("group.{}", super::GROUPS[*group]))
                        .to_owned(),
                    if index == selected
                        && matches!(
                            t.patterns.selection,
                            crate::theme::SelectionStyle::Bar | crate::theme::SelectionStyle::Arrow
                        )
                    {
                        theme::fg(t, Role::SelectionText)
                    } else {
                        Style::default()
                    },
                ),
            ];
            position += 1;
            if *group == super::OPTIMIZATION_GROUP && !app.marked.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", app.marked.len()),
                    theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
                ));
            }
            if super::GROUPS[*group] == "clash" {
                spans.extend(mihomo_marker(app));
            }
            let style = if hover == Some(index) {
                theme::fg(t, Role::Accent)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();
    let mut state = ListState::default()
        .with_selected(Some(selected))
        .with_offset(offset);
    f.render_stateful_widget(
        list(t, items, selection(t, Role::Selection)).block(theme::panel(
            t,
            i.text("ui.categories"),
            false,
        )),
        regions.categories,
        &mut state,
    );
    theme::scrollbar(
        t,
        f,
        theme::track(regions.categories, rows),
        count,
        rows.height as usize,
        offset,
    );
}
/// The clash row's mark (Requirements 2.7, 2.8): a success dot while Mihomo runs, a warning
/// "!" while its state cannot be verified, nothing otherwise.
fn mihomo_marker(app: &App) -> Option<Span<'static>> {
    let t = &app.theme.active;
    let (glyph, role) = match app.snapshot.local.mihomo_state {
        Reading::Value(State::Running(_)) => (theme::Glyph::On, Role::Success),
        Reading::Value(State::Unknown) => (theme::Glyph::Warning, Role::Warning),
        _ => return None,
    };
    Some(Span::styled(
        format!(" {}", theme::glyph(t, glyph)),
        theme::fg(t, role),
    ))
}
/// The running action spins in place; a finished single action keeps its outcome glyph.
fn task_marker(app: &App, spec: &Spec) -> Option<Span<'static>> {
    if app.progress.as_ref().is_some_and(|progress| {
        progress
            .current()
            .is_none_or(|index| app.task_ids[index] != spec.id)
    }) {
        return None;
    }
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
        format!(
            " {}",
            theme::phase_icon(&app.theme.active, app.phase, app.elapsed())
        ),
        theme::tone(&app.theme.active, app.phase.tone()),
    ))
}
/// The fewest blank cells between an action's name and its Row_Tag.
const TAG_GAP: usize = 2;
/// The glyph and color of a Last_Result's outcome.
fn outcome_mark(outcome: Outcome) -> (theme::Glyph, Role) {
    match outcome {
        Outcome::Done => (theme::Glyph::Success, Role::Success),
        Outcome::Failed => (theme::Glyph::Failure, Role::Danger),
        Outcome::Cancelled => (theme::Glyph::Cancelled, Role::Warning),
    }
}
/// The Row_Tag of an action row (Requirements 5.6, 5.7): its Last_Result's glyph and
/// Relative_Age, otherwise the Related_Reading's tag. Empty when it has neither.
fn row_tag(app: &App, spec: &Spec) -> Vec<Span<'static>> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    if let Some(last) = app.records.last.get(spec.id) {
        let (glyph, role) = outcome_mark(last.outcome);
        let age = related::relative_age(i, app.now.saturating_duration_since(last.finished));
        return vec![
            Span::styled(theme::glyph(t, glyph), theme::fg(t, role)),
            Span::styled(format!(" {age}"), theme::muted(t)),
        ];
    }
    related_tag(app, spec, theme::muted(t))
}
/// The Related_Reading's tag of `spec`: its Segments in `style`, joined by the tertiary
/// separator. Empty when the Action has no Related_Reading or its reading is not a Value.
fn related_tag(app: &App, spec: &Spec, style: Style) -> Vec<Span<'static>> {
    let t = &app.theme.active;
    let Some(related) = related::related(spec, &app.snapshot, &app.ctx.i18n, app.now) else {
        return Vec::new();
    };
    let joint = format!(" {} ", theme::glyph(t, theme::Glyph::Separator));
    let mut spans = Vec::new();
    for (index, segment) in related.tag.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(joint.clone(), theme::text(t, Level::Tertiary)));
        }
        spans.push(Span::styled(segment.text, style.patch(segment.style)));
    }
    spans
}
/// An action row `width` cells wide: the Optimization_View's checkbox, the name with "…"
/// for a Form_Action and "↗" for one that hands over the terminal, the task marker, then
/// the Row_Tag right-aligned. A row too narrow for all of it drops the whole Row_Tag
/// first, then clips the name (Requirement 5.9). The Optimization_View has no Row_Tag
/// (Requirement 5.12).
fn action_line(app: &App, spec: &Spec, width: usize) -> Line<'static> {
    if app.optimization() {
        return plan::action_line(app, spec, width);
    }
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let available = app.available(spec).is_ok();
    let mut spans = Vec::new();
    let mut name = i.text(&spec.label()).to_owned();
    if !spec.fields.is_empty() {
        name.push_str(theme::glyph(t, theme::Glyph::Ellipsis));
    }
    if spec.external {
        name.push(' ');
        name.push_str(theme::glyph(t, theme::Glyph::External));
    }
    let marker = task_marker(app, spec);
    let fixed =
        spans.iter().map(Span::width).sum::<usize>() + marker.as_ref().map_or(0, Span::width);
    let mut tag = row_tag(app, spec);
    let tag_width = tag.iter().map(Span::width).sum::<usize>();
    if fixed + name.width() + TAG_GAP + tag_width > width {
        tag.clear();
    }
    let name = clip_with(&name, width.saturating_sub(fixed), ellipsis(t));
    spans.push(if available {
        Span::raw(name)
    } else {
        Span::styled(name, theme::muted(t))
    });
    spans.extend(marker);
    if !tag.is_empty() {
        let used = spans.iter().map(Span::width).sum::<usize>();
        spans.push(Span::raw(" ".repeat(width - used - tag_width)));
        spans.extend(tag);
    }
    Line::from(spans)
}
/// `rows` is `app.rows(app.group)`. Section titles and the Rule take a line each but are
/// never selected or highlighted.
fn draw_actions(app: &App, f: &mut Frame, regions: &Regions, rows: &[Row]) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let area = regions.action_rows;
    let selected = super::row_index(rows, app.selected());
    let offset = regions.action_offset;
    let hover = pointer(app)
        .and_then(|point| regions.action_at(point))
        .filter(|index| rows[*index].selectable());
    let group = super::GROUPS[app.group];
    // Rows start after the two-cell selection marker.
    let row_width = (area.width as usize).saturating_sub(2);
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut line = match row {
                Row::Section(id) => Line::styled(
                    i.text(&format!("section.{group}.{id}")).to_owned(),
                    theme::muted(t).add_modifier(Modifier::BOLD),
                ),
                Row::Rule => Line::from(theme::rule(t, row_width)),
                Row::Action(spec) => action_line(app, spec, row_width),
                Row::Execute => Line::styled(
                    format!(
                        "{} {}",
                        theme::glyph(t, theme::Glyph::Execute),
                        i.format(
                            "ui.execute_selected",
                            &[("count", &app.marked.len().to_string())],
                        )
                    ),
                    theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
                ),
            };
            if index == selected
                && matches!(
                    t.patterns.selection,
                    crate::theme::SelectionStyle::Bar | crate::theme::SelectionStyle::Arrow
                )
            {
                for span in &mut line.spans {
                    if span.style.fg.is_none() {
                        span.style = span.style.fg(theme::color(t, Role::SelectionText));
                    }
                }
            }
            let style = if app.optimization() && index == selected {
                let selected_style = selection(t, Role::Selection);
                // A warning chip keeps its own surface and contrast inside the selected row.
                for span in &mut line.spans {
                    if span.style.fg == Some(theme::color(t, Role::Warning)) {
                        span.style = span.style.bg(theme::color(t, Role::Surface));
                    } else {
                        span.style = span.style.patch(selected_style);
                    }
                }
                selected_style
            } else if hover == Some(index) {
                theme::fg(t, Role::Accent)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
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
    let mut block = theme::panel(t, heading, false);
    // The counter places the selected action among the actions alone; the Execute_Row is
    // not one of them, so it shows none.
    if matches!(rows[selected], Row::Action(_)) {
        let actions = rows
            .iter()
            .filter(|row| matches!(row, Row::Action(_)))
            .count();
        block = block.title(
            Line::styled(
                format!(" {}/{actions} ", app.selected() + 1),
                theme::muted(t),
            )
            .right_aligned(),
        );
    }
    if regions.stacked && !theme::unframed(t) {
        block = block.borders(Borders::TOP | Borders::LEFT | Borders::RIGHT);
    }
    let mut state = ListState::default()
        .with_selected(Some(selected))
        .with_offset(offset);
    let highlight = if app.optimization() {
        Style::default()
    } else {
        selection(t, Role::Selection)
    };
    f.render_stateful_widget(
        list(t, items, highlight).block(block),
        regions.actions,
        &mut state,
    );
    theme::scrollbar(
        t,
        f,
        theme::track(regions.actions, area),
        rows.len(),
        area.height as usize,
        offset,
    );
}
/// Blank cells between a label and its value in the Details_Panel.
const LABEL_GAP: &str = "  ";
/// The mark that ends text cut short in the theme's glyph set.
fn ellipsis(t: &Theme) -> &'static str {
    theme::glyph(t, theme::Glyph::Ellipsis)
}
/// Drawn between two items of a Details_Panel line, as between Row_Tag Segments.
fn joint(t: &Theme) -> Span<'static> {
    Span::styled(
        format!(" {} ", theme::glyph(t, theme::Glyph::Separator)),
        theme::text(t, Level::Tertiary),
    )
}
fn heading(app: &App, key: &str) -> Line<'static> {
    Line::styled(
        app.ctx.i18n.text(key).to_owned(),
        theme::muted(&app.theme.active).add_modifier(Modifier::BOLD),
    )
}
fn name_line(app: &App, spec: &Spec, width: usize) -> Line<'static> {
    let name = app.ctx.i18n.text(&spec.label());
    Line::styled(
        clip_with(name, width, ellipsis(&app.theme.active)),
        theme::bold(),
    )
}
/// The impact (Requirement 6.2), toned by risk: whether the action changes the system and
/// whether it needs root, then, for an action that hands the terminal to another program,
/// that it takes it over.
fn impact_lines(app: &App, spec: &Spec, width: usize) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let item = |text: String, tone: Tone| vec![Span::styled(text, theme::tone(t, tone))];
    let mut items = vec![
        if spec.changes {
            item(i.text("ui.mutating").to_owned(), Tone::Warning)
        } else {
            item(i.text("ui.readonly").to_owned(), Tone::Success)
        },
        if spec.root {
            item(i.text("ui.root").to_owned(), Tone::Warning)
        } else {
            item(i.text("ui.user").to_owned(), Tone::Muted)
        },
    ];
    if spec.external {
        let external = theme::glyph(t, theme::Glyph::External);
        items.push(item(
            format!("{} {external}", i.text("ui.takes_terminal")),
            Tone::Muted,
        ));
    }
    flow(Vec::new(), items, &joint(t), width, 0, ellipsis(t))
}
fn unavailable_lines(app: &App, spec: &Spec, width: usize) -> Vec<Line<'static>> {
    let Err(error) = app.available(spec) else {
        return Vec::new();
    };
    let t = &app.theme.active;
    let text = format!(
        "{} {}",
        theme::glyph(t, theme::Glyph::Warning),
        app.ctx.i18n.error(&error)
    );
    wrap_words(&text, width)
        .into_iter()
        .map(|row| Line::styled(row, theme::fg(t, Role::Warning)))
        .collect()
}

/// A block of the Details_Panel. Compact_Mode may shorten the description to its first
/// line, `short`.
struct Part {
    lines: Vec<Line<'static>>,
    short: Option<Vec<Line<'static>>>,
}
impl Part {
    fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines, short: None }
    }
}
/// The description as prose; shortened, its first line ends in an ellipsis.
fn description(app: &App, spec: &Spec, width: usize) -> Part {
    let mark = ellipsis(&app.theme.active);
    let rows = wrap_words(app.ctx.i18n.text(&spec.description()), width);
    let short = (rows.len() > 1).then(|| {
        vec![Line::from(clip_with(
            &format!("{}{mark}", rows[0]),
            width,
            mark,
        ))]
    });
    Part {
        lines: rows.into_iter().map(Line::from).collect(),
        short,
    }
}
/// Compact_Mode for the Details_Panel (design component 3): when the parts do not fit in
/// `room` rows, the blank lines between them go first, then the description's lines after
/// its first; whatever still does not fit is cut at the bottom.
fn fit_parts(parts: &[Part], room: usize, spaced: bool) -> Vec<Line<'static>> {
    let compose = |spaced: bool, shorten: bool| {
        let mut lines = Vec::new();
        for part in parts.iter().filter(|part| !part.lines.is_empty()) {
            if spaced && !lines.is_empty() {
                lines.push(Line::default());
            }
            let chosen = match &part.short {
                Some(short) if shorten => short,
                _ => &part.lines,
            };
            lines.extend(chosen.iter().cloned());
        }
        lines
    };
    let mut lines = [(spaced, false), (false, false)]
        .into_iter()
        .map(|(spaced, shorten)| compose(spaced, shorten))
        .find(|lines| lines.len() <= room)
        .unwrap_or_else(|| compose(false, true));
    lines.truncate(room);
    lines
}

/// The label of a `related.state` line, which holds it in its first span.
fn state_label(line: &Line<'_>) -> String {
    line.spans
        .first()
        .map(|span| span.content.clone().into_owned())
        .unwrap_or_default()
}
/// The value of a `related.state` line: every span after the label and the gap, in the
/// primary Text_Level.
fn state_value(t: &Theme, line: &Line<'_>) -> Vec<Span<'static>> {
    line.spans
        .iter()
        .skip(2)
        .map(|span| {
            Span::styled(
                span.content.clone().into_owned(),
                theme::text(t, Level::Primary),
            )
        })
        .collect()
}
/// "Current state" (Requirement 6.3): each reading of the Related_Reading, labels in one
/// column. system.clean draws the system disk's gauge above its used, total and available
/// space.
fn state_lines(
    app: &App,
    spec: &Spec,
    related: &related::Related,
    width: usize,
) -> Vec<Line<'static>> {
    let t = &app.theme.active;
    let mark = ellipsis(t);
    let label_width = related
        .state
        .iter()
        .map(|line| state_label(line).width())
        .max()
        .unwrap_or(0);
    let column = label_width + LABEL_GAP.width();
    let disk = match &app.snapshot.local.disk {
        Reading::Value(disk) if spec.id == "system.clean" && disk.total > 0 => Some(disk),
        _ => None,
    };
    let mut lines = vec![heading(app, "ui.current_state")];
    for line in &related.state {
        let label = Span::styled(pad(&state_label(line), label_width), theme::muted(t));
        let mut spans = vec![label, Span::raw(LABEL_GAP)];
        if let Some(disk) = disk {
            // The gauge takes the label's row; the amounts sit under it in the value column.
            let percent = disk.percent();
            let room = width.saturating_sub(column);
            spans.extend(theme::bar(t, percent, room, theme::gauge(percent)));
            lines.push(Line::from(clip_spans(spans, width, mark)));
            spans = vec![Span::raw(" ".repeat(column))];
        }
        spans.extend(state_value(t, line));
        lines.push(Line::from(clip_spans(spans, width, mark)));
    }
    lines
}
/// One line of current state for the stacked panel (Requirement 6.8): each reading as
/// `label value`, as many whole ones as fit (as `fit_segments` counts them), or the first
/// one clipped.
fn state_summary(app: &App, related: &related::Related, width: usize) -> Line<'static> {
    let t = &app.theme.active;
    let readings = related
        .state
        .iter()
        .map(|line| (state_label(line), state_value(t, line)))
        .collect::<Vec<_>>();
    let segments = readings
        .iter()
        .map(|(label, value)| {
            let value = value
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            Segment::new(format!("{label} {value}"), Style::default())
        })
        .collect::<Vec<_>>();
    let count = fit_segments(&segments, width).max(1);
    let mut spans = Vec::new();
    for (index, (label, value)) in readings.into_iter().take(count).enumerate() {
        if index > 0 {
            spans.push(joint(t));
        }
        spans.push(Span::styled(label, theme::muted(t)));
        spans.push(Span::raw(" "));
        spans.extend(value);
    }
    Line::from(clip_spans(spans, width, ellipsis(t)))
}

/// A field's current value and the values it takes (Requirement 6.4), as items for `flow`
/// with the joint between them. Fields are told apart as the Parameter_Form tells them: a
/// listed field names an existing item, a field limited to its choices is a Chip_Field, and
/// an editable field takes any value, offering its choices as presets with `allow_custom`.
fn field_values(app: &App, field: &Field, value: &str) -> (Vec<Vec<Span<'static>>>, Span<'static>) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let note = |key: &str| {
        vec![Span::styled(
            i.text(key).to_owned(),
            theme::text(t, Level::Tertiary),
        )]
    };
    let current = (!value.is_empty()).then(|| {
        vec![Span::styled(
            value.to_owned(),
            theme::text(t, Level::Primary),
        )]
    });
    if field.source.is_some() {
        let mut items = current.into_iter().collect::<Vec<_>>();
        items.push(note("ui.choose_on_open"));
        return (items, joint(t));
    }
    if !field.editable() {
        let chips = field
            .choices
            .iter()
            .map(|choice| {
                if *choice == value {
                    vec![
                        Span::styled(
                            format!("{} ", theme::glyph(t, theme::Glyph::On)),
                            theme::fg(t, Role::Accent),
                        ),
                        Span::styled((*choice).to_owned(), theme::text(t, Level::Primary)),
                    ]
                } else {
                    vec![Span::styled(
                        format!("{} {choice}", theme::glyph(t, theme::Glyph::Off)),
                        theme::text(t, Level::Tertiary),
                    )]
                }
            })
            .collect();
        return (chips, Span::raw(LABEL_GAP));
    }
    let mut items = match current {
        Some(current) => vec![current],
        None if field.required => vec![vec![Span::styled(
            i.text("ui.not_set").to_owned(),
            theme::tone(t, Tone::Warning),
        )]],
        None => Vec::new(),
    };
    if field.allow_custom {
        items.push(note("ui.accepts_custom"));
    }
    (items, joint(t))
}
/// "Parameters" (Requirement 6.4): each field's saved value, or its default, and the values
/// it takes. Labels share a column while every field fits on one line; otherwise each label
/// takes a line of its own with the values indented under it.
fn parameter_lines(
    app: &App,
    spec: &Spec,
    parameters: &Parameters,
    width: usize,
) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let mark = ellipsis(t);
    let fields = spec
        .fields
        .iter()
        .map(|field| {
            let value = parameters
                .get(field.name)
                .map_or(field.default, String::as_str);
            (i.text(&field.label()), field_values(app, field, value))
        })
        .collect::<Vec<_>>();
    let label_width = fields
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or(0);
    let column = label_width + LABEL_GAP.width();
    let aligned = fields
        .iter()
        .all(|(_, (items, joint))| column + flow_width(items, joint) <= width);
    let mut lines = vec![heading(app, "ui.parameters")];
    for (label, (items, joint)) in fields {
        if aligned {
            let lead = vec![
                Span::styled(pad(label, label_width), theme::muted(t)),
                Span::raw(LABEL_GAP),
            ];
            lines.extend(flow(lead, items, &joint, width, column, mark));
            continue;
        }
        lines.extend(
            wrap_words(label, width)
                .into_iter()
                .map(|row| Line::styled(row, theme::muted(t))),
        );
        if !items.is_empty() {
            let indent = LABEL_GAP.width();
            lines.extend(flow(
                vec![Span::raw(LABEL_GAP)],
                items,
                &joint,
                width,
                indent,
                mark,
            ));
        }
    }
    lines
}
/// Command_Preview (Requirement 7): `cqs run` with the values the action would run with,
/// wrapped between words. A command still missing a required value cannot run as shown, so
/// all of it is tertiary (Requirement 7.7); a complete one is in the accent color.
fn command_lines(
    app: &App,
    spec: &Spec,
    parameters: &Parameters,
    width: usize,
) -> Vec<Line<'static>> {
    let t = &app.theme.active;
    let words = preview::single_words(spec, parameters);
    let style = if preview::incomplete(&words) {
        theme::text(t, Level::Tertiary)
    } else {
        theme::fg(t, Role::Accent)
    };
    let shown = words.iter().map(preview::Word::shown).collect::<Vec<_>>();
    preview::wrap(&shown, width)
        .into_iter()
        // A word wider than a whole line still shows every character.
        .flat_map(|line| {
            if line.width() > width {
                wrap_cells(&line, width)
            } else {
                vec![line]
            }
        })
        .map(|line| Line::styled(line, style))
        .collect()
}
/// The Last_Result (Requirements 6.5, 6.6): its outcome, Relative_Age and elapsed time, or
/// that the action has not run in this session.
fn last_result_line(app: &App, spec: &Spec, width: usize) -> Line<'static> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let mut spans = vec![
        Span::styled(i.text("ui.last_result").to_owned(), theme::muted(t)),
        Span::raw(LABEL_GAP),
    ];
    match app.records.last.get(spec.id) {
        None => spans.push(Span::styled(
            i.text("ui.not_run_yet").to_owned(),
            theme::text(t, Level::Tertiary),
        )),
        Some(last) => {
            let (glyph, role) = outcome_mark(last.outcome);
            let outcome = match last.outcome {
                Outcome::Done => "ui.done",
                Outcome::Failed => "ui.failed",
                Outcome::Cancelled => "ui.cancelled",
            };
            let age = related::relative_age(i, app.now.saturating_duration_since(last.finished));
            let elapsed = i.format("ui.elapsed", &[("time", &mm_ss(last.elapsed))]);
            spans.extend([
                Span::styled(theme::glyph(t, glyph), theme::fg(t, role)),
                Span::raw(" "),
                Span::styled(i.text(outcome).to_owned(), theme::text(t, Level::Primary)),
                joint(t),
                Span::styled(age, theme::muted(t)),
                joint(t),
                Span::styled(elapsed, theme::muted(t)),
            ]);
        }
    }
    Line::from(clip_spans(spans, width, ellipsis(t)))
}
/// What Enter does with the selected action (Requirement 6.7), as `activate` decides it;
/// tertiary while the action cannot run.
fn enter_hint(app: &App, spec: &Spec) -> Line<'static> {
    let t = &app.theme.active;
    let label = match spec.kind {
        Kind::Info => "key.open_overview",
        Kind::PublicIp => "key.query_public",
        _ if !spec.fields.is_empty() => "key.configure_run",
        _ => "key.accept",
    };
    let (key, text) = if app.available(spec).is_ok() {
        (
            theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
            theme::muted(t),
        )
    } else {
        let dim = theme::text(t, Level::Tertiary);
        (dim, dim)
    };
    Line::from(vec![
        Span::styled(theme::glyph(t, theme::Glyph::Enter), key),
        Span::styled(format!(" {}", app.ctx.i18n.text(label)), text),
    ])
}

/// The Details_Panel beside the action list (Requirement 6.1), top-down: name, impact, why
/// it is unavailable, description, current state, parameters, Command_Preview and
/// Last_Result, fitted to `height` rows less the footer. The footer is pinned to the bottom:
/// the Enter hint. Optimization uses the separate Plan_Panel.
fn full_details(
    app: &App,
    spec: &Spec,
    width: usize,
    height: usize,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let i = &app.ctx.i18n;
    let footer = vec![enter_hint(app, spec)];
    let parameters = app
        .parameters
        .get(spec.id)
        .cloned()
        .unwrap_or_else(|| spec.defaults());
    let mut head = vec![name_line(app, spec, width)];
    head.extend(impact_lines(app, spec, width));
    head.extend(unavailable_lines(app, spec, width));
    let mut parts = vec![Part::new(head), description(app, spec, width)];
    if let Some(related) = related::related(spec, &app.snapshot, i, app.now) {
        parts.push(Part::new(state_lines(app, spec, &related, width)));
    }
    if !spec.fields.is_empty() {
        parts.push(Part::new(parameter_lines(app, spec, &parameters, width)));
    }
    parts.push(Part::new(command_lines(app, spec, &parameters, width)));
    parts.push(Part::new(vec![last_result_line(app, spec, width)]));
    let room = height.saturating_sub(footer.len());
    (fit_parts(&parts, room, true), footer)
}
/// The stacked Details_Panel under the action list (Requirement 6.8): name, impact and one
/// line of current state, then the description and Command_Preview, fitted to `height`.
fn compact_details(app: &App, spec: &Spec, width: usize, height: usize) -> Vec<Line<'static>> {
    let parameters = app
        .parameters
        .get(spec.id)
        .cloned()
        .unwrap_or_else(|| spec.defaults());
    let mut head = vec![name_line(app, spec, width)];
    head.extend(impact_lines(app, spec, width));
    if let Some(related) = related::related(spec, &app.snapshot, &app.ctx.i18n, app.now) {
        head.push(state_summary(app, &related, width));
    }
    let parts = [
        Part::new(head),
        description(app, spec, width),
        Part::new(command_lines(app, spec, &parameters, width)),
    ];
    fit_parts(&parts, height, false)
}
/// `spec` is the selected action; `None` on the Execute_Row, which runs the whole Checklist.
fn draw_details(app: &App, f: &mut Frame, regions: &Regions, spec: Option<&Spec>) {
    if app.style_panel.is_some() {
        style::draw(app, f, regions);
        return;
    }
    if app.optimization() {
        if app.progress.is_some() {
            plan::draw_progress(app, f, regions);
        } else {
            plan::draw(app, f, regions);
        }
        return;
    }
    let spec = spec.expect("a Reorderable_Category selects an Action");
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let mut block = theme::panel(t, i.text("ui.details"), false)
        .padding(theme::panel_padding(t, regions.details.width));
    if regions.stacked && !theme::unframed(t) {
        // The rule continues the action panel's sides, so both read as one panel.
        block = block.border_set(theme::joined_border_set(t));
    }
    let inner = block.inner(regions.details);
    f.render_widget(block, regions.details);
    let (width, height) = (inner.width as usize, inner.height as usize);
    let (body, footer) = if regions.stacked {
        (compact_details(app, spec, width, height), Vec::new())
    } else {
        full_details(app, spec, width, height)
    };
    let [body_area, footer_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(footer.len() as u16)])
            .areas(inner);
    f.render_widget(Paragraph::new(body), body_area);
    f.render_widget(Paragraph::new(footer), footer_area);
}

/// Keys for the current moment, by priority; `?` keeps the full reference one key away.
fn footer_hints(app: &App) -> (Vec<theme::Hint<'_>>, theme::Hint<'_>) {
    let t = |key: &str| app.ctx.i18n.text(key);
    let help = ("?", t("key.help"));
    let back = ("Esc", t("key.back"));
    if app.confirmation.is_some() {
        return (vec![("Enter", t("key.confirm"))], back);
    }
    if app.loading.is_some() {
        return (Vec::new(), back);
    }
    if let Some(form) = &app.form {
        let hints = if form.picker.is_some() {
            vec![("↑↓", t("key.move")), ("Enter", t("key.pick"))]
        } else {
            vec![
                ("Tab/↑↓", t("key.field")),
                ("←→", t("key.choice")),
                ("Ctrl+Enter", t("key.submit")),
            ]
        };
        return (hints, back);
    }
    if app.help || app.notice.is_some() {
        return (Vec::new(), ("Esc", t("key.close")));
    }
    if app.command_palette.is_some() {
        return (
            vec![
                ("↑↓", t("key.move")),
                ("Enter", t("key.accept")),
                ("Tab", t("key.jump")),
                ("Ctrl+U", t("key.clear")),
            ],
            ("Esc", t("key.close")),
        );
    }
    if app.log_modal {
        return (vec![("PgUp/PgDn", t("key.scroll"))], back);
    }
    if app.overview {
        let hints = vec![("R", t("key.refresh")), ("N", t("key.public"))];
        return (hints, back);
    }
    if app.style_panel.is_some() {
        return style_hints(app);
    }
    if app.worker.is_some() || matches!(app.phase, Phase::Running | Phase::Cancelling) {
        return (
            vec![
                ("Esc", t("key.cancel_task")),
                ("F2", t("key.full_log")),
                ("G", t("key.logs")),
                ("Q", t("key.quit")),
            ],
            help,
        );
    }
    let hints = if app.optimization() {
        vec![
            ("Space", t("key.select")),
            ("Enter", t("key.configure")),
            ("X/F5", t("key.run_checklist")),
            ("A", t("key.recommended")),
            ("/", t("key.search")),
            ("←→", t("key.pan")),
            ("T", t("key.style")),
            ("Q", t("key.quit")),
        ]
    } else {
        vec![
            ("↑↓", t("key.move")),
            ("Enter", t("key.open")),
            ("/", t("key.search")),
            ("←→", t("key.pan")),
            ("O", t("key.optimize")),
            ("T", t("key.style")),
            ("R", t("key.refresh")),
            ("Q", t("key.quit")),
        ]
    };
    (hints, help)
}

pub(super) fn style_hints(app: &App) -> (Vec<theme::Hint<'_>>, theme::Hint<'_>) {
    let i18n = &app.ctx.i18n;
    (
        vec![
            ("↑↓/j k/Home/End", i18n.text("key.preview")),
            ("Enter", i18n.text("key.keep_theme")),
            ("?/F1", i18n.text("key.help")),
        ],
        ("Esc", i18n.text("key.restore_theme")),
    )
}
/// Task state for screens too short for the inline log panel: phase, task and the latest
/// output while it runs; the outcome stays until the next task starts.
fn footer_status(app: &App, logs_visible: bool, room: usize) -> Option<Line<'static>> {
    if app.phase == Phase::Idle
        || logs_visible
        || app.form.is_some()
        || app.confirmation.is_some()
        || app.loading.is_some()
        || app.notice.is_some()
        || app.help
        || app.command_palette.is_some()
    {
        return None;
    }
    let i = &app.ctx.i18n;
    let tone = app.phase.tone();
    let mut text = format!(
        "{} {} · {}",
        theme::phase_icon(&app.theme.active, app.phase, app.elapsed()),
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
    Some(
        Line::styled(
            clip_with(&text, room, ellipsis(&app.theme.active)),
            theme::tone(&app.theme.active, tone),
        )
        .right_aligned(),
    )
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
        Paragraph::new(theme::key_hints(
            &app.theme.active,
            &hints,
            Some(pinned),
            room as usize,
        )),
        area,
    );
    if let Some(status) = status {
        f.render_widget(Paragraph::new(status), area);
    }
}

fn draw_small(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let width = area.width.saturating_sub(4).max(1) as usize;
    let mut lines = wrap_words(i.text("ui.small"), width)
        .into_iter()
        .map(|row| Line::styled(row, theme::fg(t, Role::Warning)))
        .collect::<Vec<_>>();
    lines.push(Line::styled(
        i.format(
            "ui.current_size",
            &[
                ("width", &area.width.to_string()),
                ("height", &area.height.to_string()),
            ],
        ),
        theme::muted(t),
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
    if app.theme.active.patterns.background == crate::theme::BackgroundStyle::Paint {
        f.buffer_mut().set_style(
            area,
            Style::default()
                .bg(theme::color(&app.theme.active, Role::Background))
                .fg(theme::color(&app.theme.active, Role::Text1)),
        );
    }
    if !fits(area) {
        draw_small(app, f, area);
        return;
    }
    let regions = Regions::new(area, app);
    let selected = regions.selected_row(app);
    let spec = match selected {
        Row::Action(spec) => Some(spec),
        _ => None,
    };
    header::draw(app, f, regions.header, selected);
    dashboard::draw(app, f, regions.dashboard);
    draw_categories(app, f, &regions);
    draw_actions(app, f, &regions, &regions.rows);
    draw_details(app, f, &regions, spec);
    if regions.logs.height > 0 {
        logs::draw(app, f, regions.logs);
    }
    draw_footer(app, f, &regions);
    decorate_panels(app, f, &regions);
    // Feedback sits under modal layers so a dialog always reads as the top layer.
    dialogs::draw_toast(app, f, regions.footer);
    dialogs::draw_overview(app, f);
    if app.log_modal {
        logs::draw(app, f, popup_full(area));
    }
    dialogs::draw_palette(app, f);
    dialogs::draw_help(app, f);
    dialogs::draw_notice(app, f);
    dialogs::draw_loading(app, f);
    dialogs::draw_form(app, f);
    dialogs::draw_picker(app, f);
    dialogs::draw_confirmation(app, f);
    if app.dialog_fading() {
        dialogs::fade(app, f);
    }
}

fn decorate_panels(app: &App, frame: &mut Frame, regions: &Regions) {
    use crate::theme::BorderStyle;
    let active = &app.theme.active;
    if active.patterns.border == BorderStyle::Card {
        let mut columns = vec![regions.categories.right().saturating_sub(1)];
        if !regions.stacked {
            columns.push(regions.actions.right().saturating_sub(1));
        }
        for column in columns {
            for row in regions.categories.top()..regions.categories.bottom() {
                let cell = &mut frame.buffer_mut()[(column, row)];
                if cell.symbol() != "┃" {
                    cell.set_symbol("│")
                        .set_style(theme::fg(active, Role::Rule));
                }
                cell.set_bg(
                    if active.patterns.background == crate::theme::BackgroundStyle::Terminal {
                        ratatui::style::Color::Reset
                    } else {
                        theme::color(active, Role::Background)
                    },
                );
            }
        }
    }
    if active.patterns.border == BorderStyle::Titlebar {
        let [left, right] = Layout::horizontal([Constraint::Fill(1); 2]).areas(regions.dashboard);
        for area in [
            regions.categories,
            regions.actions,
            regions.details,
            left,
            right,
            regions.logs,
        ] {
            if area.height > 0 {
                frame.buffer_mut().set_style(
                    Rect { height: 1, ..area },
                    Style::default().bg(theme::color(active, Role::SurfaceAlt)),
                );
            }
        }
    }
    if matches!(
        active.patterns.selection,
        crate::theme::SelectionStyle::Bar | crate::theme::SelectionStyle::Arrow
    ) {
        for (area, selected, offset) in [
            (
                regions.category_rows,
                app.category_row(),
                regions.category_offset,
            ),
            (
                regions.action_rows,
                super::row_index(&app.rows(app.group), app.selected()),
                regions.action_offset,
            ),
        ] {
            if selected >= offset && selected - offset < area.height as usize {
                frame.buffer_mut()[(area.x, area.y + (selected - offset) as u16)]
                    .set_fg(theme::color(active, Role::SelectionMark));
            }
        }
    }
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
            let screen = rows(&app, width, height);
            // The seven Status_Area rows label every row; the proxy row leads with Mihomo
            // and adds the terminal proxy once there is room for it.
            let status = screen[1..8].concat();
            for label in [
                "CPU", "Memory", "Disk", "Swap", "Host", "Kernel", "Uptime", "BBR", "TCP", "IPv4",
                "IPv6", "Proxy", "Mihomo",
            ] {
                assert!(status.contains(label), "{label} at {width}x{height}");
            }
            assert_eq!(
                status.contains("Terminal"),
                width >= 120,
                "terminal proxy at {width}x{height}"
            );
            let text = screen.concat();
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
            for (state, proxy, key, short) in [
                (
                    State::Running(1234),
                    false,
                    "dash.mihomo_running",
                    "dash.running",
                ),
                (State::Stopped, true, "dash.stopped", "dash.stopped"),
                (
                    State::NotInstalled,
                    false,
                    "dash.not_installed",
                    "dash.not_installed",
                ),
                (State::Unknown, true, "dash.unknown", "dash.unknown"),
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
                // The Status_Area's proxy row: Mihomo's state, its version from 120 columns
                // and the terminal proxy where the whole row fits.
                for (width, height) in [(80, 24), (120, 40), (200, 55)] {
                    let status = rows(&app, width, height)[1..8].to_vec();
                    assert!(
                        shows(&status, "Mihomo", i.text(short)),
                        "Mihomo state at {width}x{height} ({locale})"
                    );
                    let version = status
                        .iter()
                        .any(|row| row.contains("Mihomo") && row.contains("v1.19.12"));
                    assert!(version || width < 120, "version at {width} ({locale})");
                    assert!(
                        shows(&status, i.text("dash.terminal"), expected_proxy) || width < 200,
                        "terminal proxy at {width} ({locale})"
                    );
                }
                // The overview lists every proxy fact in full.
                for width in [60, 96] {
                    let screen = dashboard::overview(&app, width)
                        .iter()
                        .map(|line| line.to_string().replace(' ', ""))
                        .collect::<Vec<_>>();
                    for (label, value) in [
                        ("dash.terminal_proxy", expected_proxy),
                        ("dash.mihomo_state", expected_state.as_str()),
                        ("dash.mihomo_version", "v1.19.12"),
                    ] {
                        assert!(
                            shows(&screen, app.ctx.i18n.text(label), value),
                            "missing {label}={value} at {width} ({locale})"
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
            for label in [
                "System & resources",
                "CPU",
                "Memory",
                "Disk",
                "Swap",
                "Host",
                "Kernel",
                "Uptime",
            ] {
                let x = column(label).unwrap_or_else(|| panic!("{label} missing at {width}"));
                assert!(x < split, "{label} belongs to the system panel at {width}");
            }
            for label in [
                "Network & proxy",
                "BBR",
                "TCP",
                "IPv4",
                "IPv6",
                "Proxy",
                "Mihomo",
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
    /// An app whose inline logs are open and hold output, so only the size decides.
    fn logging_app() -> App {
        let mut app = app("en-US");
        app.logs_open = true;
        app.log("working");
        app
    }
    #[test]
    fn regions_split_80x24_and_120x36_into_fixed_rows() {
        let mut app = logging_app();
        let small = Regions::new(Rect::new(0, 0, 80, 24), &app);
        assert_eq!(small.header, Rect::new(0, 0, 80, 1));
        assert_eq!(small.dashboard, Rect::new(0, 1, 80, 7));
        assert_eq!(small.categories, Rect::new(0, 8, 20, 15));
        assert_eq!(small.category_rows, Rect::new(1, 9, 18, 13));
        // Below 112 columns the actions and their details share one panel.
        assert!(small.stacked);
        assert_eq!(small.actions, Rect::new(20, 8, 60, 9));
        assert_eq!(small.action_rows, Rect::new(21, 9, 58, 8));
        assert_eq!(small.details, Rect::new(20, 17, 60, 6));
        assert_eq!(small.logs.height, 0, "15 content rows leave none for logs");
        assert_eq!(small.footer, Rect::new(0, 23, 80, 1));

        let large = Regions::new(Rect::new(0, 0, 120, 36), &app);
        assert_eq!(large.header, Rect::new(0, 0, 120, 1));
        assert_eq!(large.dashboard, Rect::new(0, 1, 120, 7));
        assert_eq!(large.categories, Rect::new(0, 8, 20, 15));
        assert!(!large.stacked);
        assert_eq!(large.actions, Rect::new(20, 8, ACTION_WIDTH, 15));
        assert_eq!(large.details, Rect::new(64, 8, 56, 15));
        assert_eq!(large.logs, Rect::new(0, 23, 120, 12));
        assert_eq!(large.footer, Rect::new(0, 35, 120, 1));

        // Closed logs give their rows back to the action area.
        app.logs_open = false;
        let closed = Regions::new(Rect::new(0, 0, 120, 36), &app);
        assert_eq!(closed.categories, Rect::new(0, 8, 20, 27));
        assert_eq!(closed.logs.height, 0);
    }
    #[test]
    fn inline_logs_appear_from_a_height_of_32() {
        let mut app = logging_app();
        let at = |app: &App, height| Regions::new(Rect::new(0, 0, 120, height), app);
        assert_eq!(at(&app, 31).logs.height, 0);
        assert_eq!(at(&app, 31).categories.height, 22);
        assert_eq!(at(&app, 32).logs.height, MIN_LOGS);
        assert_eq!(at(&app, 32).categories.height, MIN_BODY);
        // Every height splits its content between the action area and the logs as
        // min(⌊3·content/5⌋, content − 15), shown only from 8 rows.
        for height in MIN_HEIGHT..=120 {
            let regions = at(&app, height);
            let content = height - 9;
            let share = (content * 3 / 5).min(content - MIN_BODY);
            let logs = if share >= MIN_LOGS { share } else { 0 };
            assert_eq!(regions.logs.height, logs, "{height}");
            assert_eq!(regions.categories.height, content - logs, "{height}");
            assert_eq!(regions.footer.y, height - 1, "{height}");
        }
        app.logs_open = false;
        assert_eq!(at(&app, 50).logs.height, 0);
    }
    #[test]
    fn all_thirteen_categories_show_without_scrolling_at_80x24() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let area = Rect::new(0, 0, 80, 24);
        for locale in ["zh-CN", "en-US"] {
            let mut app = app(locale);
            // The last category would scroll the list if any row were missing.
            app.group = *app.display_order().last().unwrap();
            let regions = Regions::new(area, &app);
            let rows = regions.category_rows;
            let entries = app.category_rows();
            assert_eq!(entries.len(), 13);
            assert_eq!(rows.height, 13, "{locale}");
            assert_eq!(regions.category_offset, 0, "{locale}");
            let buffer = terminal(&app, area.width, area.height)
                .backend()
                .buffer()
                .clone();
            for (index, entry) in entries.iter().enumerate() {
                let y = rows.y + index as u16;
                let line = (rows.x..rows.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .replace(' ', "");
                let expected = match entry {
                    Some(group) => app
                        .ctx
                        .i18n
                        .text(&format!("group.{}", super::super::GROUPS[*group]))
                        .replace(' ', ""),
                    None => "─".repeat(rows.width as usize - 2),
                };
                assert!(line.contains(&expected), "{locale} row {index}: {line:?}");
                // No scrollbar thumb: the track keeps the plain border.
                assert_eq!(buffer[(regions.categories.right() - 1, y)].symbol(), "│");
            }
            // A click lands on the row drawn under it; the rule changes nothing.
            for (index, entry) in entries.iter().enumerate() {
                let before = app.group;
                let click = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: rows.x + 4,
                    row: rows.y + index as u16,
                    modifiers: KeyModifiers::NONE,
                };
                app.mouse(click, area).unwrap();
                assert_eq!(app.group, entry.unwrap_or(before), "{locale} row {index}");
            }
        }
    }
    #[test]
    fn actions_and_details_sit_side_by_side_from_width_112() {
        let app = app("en-US");
        let narrow = Regions::new(Rect::new(0, 0, SIDE_BY_SIDE - 1, 24), &app);
        assert!(narrow.stacked);
        assert_eq!(narrow.actions, Rect::new(20, 8, 91, 9));
        assert_eq!(narrow.details, Rect::new(20, 17, 91, 6));
        let wide = Regions::new(Rect::new(0, 0, SIDE_BY_SIDE, 24), &app);
        assert!(!wide.stacked);
        assert_eq!(wide.actions, Rect::new(20, 8, ACTION_WIDTH, 15));
        assert_eq!(wide.details, Rect::new(64, 8, 48, 15));
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
                let primary = if app.form.as_ref().unwrap().spec.reversible {
                    "ui.confirm_execute"
                } else {
                    "ui.continue"
                };
                assert!(screen.contains(&app.ctx.i18n.text(primary).replace(' ', "")));
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
        let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
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
        assert_eq!(
            buffer[(1, 0)].fg,
            theme::color(&app.theme.active, Role::Dim)
        );
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
                theme::color(&app.theme.active, Role::DialogSelection),
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
            let geometry = FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
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
        let behind = FormGeometry::new(area, &form, &app).popup;
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
        // Below the first Section title and the selected first action.
        let point = (regions.action_rows.x + 4, regions.action_rows.y + 2);
        app.hover = Some(point.into());
        let accent = theme::color(&app.theme.active, Role::Accent);
        let hovered = terminal(&app, area.width, area.height);
        assert_eq!(hovered.backend().buffer()[point].fg, accent);
        assert_eq!(app.selected(), 0);
        app.help = true;
        let covered = terminal(&app, area.width, area.height);
        assert_ne!(covered.backend().buffer()[point].fg, accent);
    }
    #[test]
    fn the_next_frame_draws_in_the_active_theme() {
        use crate::theme::{Background, builtin, resolve};
        let mut app = app("en-US");
        let area = Rect::new(0, 0, 120, 36);
        let regions = Regions::new(area, &app);
        // The status panel's top-left corner and the selected action row, which follows the
        // first Section title.
        let corner = (regions.dashboard.x, regions.dashboard.y);
        let selected = (regions.action_rows.x, regions.action_rows.y + 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .unwrap();
        let mut frame = |app: &App| {
            terminal.draw(|f| app.draw(f)).unwrap();
            let buffer = terminal.backend().buffer();
            (buffer[corner].clone(), buffer[selected].clone())
        };
        let (dark_corner, dark_row) = frame(&app);
        let dark = app.theme.active.clone();
        assert_eq!(dark.palette, builtin::CLASSIC_DARK);
        assert_eq!(dark_corner.fg, theme::color(&dark, Role::Border));

        let white = Background::Detected((0xff, 0xff, 0xff));
        app.theme.active = resolve(&builtin::classic(), &white);
        let light = app.theme.active.clone();
        assert_eq!(light.palette, builtin::CLASSIC_LIGHT);
        let (light_corner, light_row) = frame(&app);
        assert_eq!(light_corner.symbol(), dark_corner.symbol());
        assert_eq!(light_corner.fg, theme::color(&light, Role::Border));
        assert_ne!(light_corner.fg, dark_corner.fg);
        assert_eq!(light_row.symbol(), dark_row.symbol());
        assert_eq!(light_row.bg, theme::color(&light, Role::Selection));
        assert_ne!(light_row.bg, dark_row.bg);
        assert_ne!(light_row.fg, dark_row.fg);
    }
    #[test]
    fn main_view_has_no_focus_and_numbers_the_category_list() {
        let mut app = app("en-US");
        let area = Rect::new(0, 0, 120, 36);
        let regions = Regions::new(area, &app);
        let rows = regions.category_rows;
        assert!(rows.height >= 13, "the whole list fits: {rows:?}");
        let t = app.theme.active.clone();
        let screen = |app: &App| {
            terminal(app, area.width, area.height)
                .backend()
                .buffer()
                .clone()
        };
        let text = |buffer: &ratatui::buffer::Buffer, y: u16| {
            (rows.x..rows.right())
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        };
        // Every selected row is filled the same way: classic's `fill` selection pattern.
        let filled = |buffer: &ratatui::buffer::Buffer, x: u16, y: u16| {
            let cell = &buffer[(x, y)];
            cell.fg == theme::color(&t, Role::SelectionText)
                && cell.bg == theme::color(&t, Role::Selection)
                && cell.modifier.contains(Modifier::BOLD)
        };

        let buffer = screen(&app);
        // 0 for the pinned category, a rule, then 1–9 and a blank of the same width.
        let lines = (rows.y..rows.y + 13)
            .map(|y| text(&buffer, y))
            .collect::<Vec<_>>();
        assert_eq!(
            lines[1],
            format!("  {}", "─".repeat(rows.width as usize - 2))
        );
        for (position, group) in app.display_order().iter().enumerate() {
            let line = &lines[position + usize::from(position > 0)];
            let name = app
                .ctx
                .i18n
                .text(&format!("group.{}", super::super::GROUPS[*group]));
            let digit = if position <= 9 {
                position.to_string()
            } else {
                " ".into()
            };
            let after_marker = line.chars().skip(2).collect::<String>();
            assert!(
                after_marker.starts_with(&format!("{digit} {name}")),
                "{position}: {line:?}"
            );
        }
        // The selected category and action are both filled; neither list owns a focus.
        let selected = rows.y + 2;
        assert!(lines[2].starts_with("› 1 "), "{:?}", lines[2]);
        for x in rows.x..rows.right() {
            assert!(filled(&buffer, x, selected), "category cell {x}");
        }
        let action = &regions.action_rows;
        assert!(filled(&buffer, action.x, action.y + 1));
        for panel in [regions.categories, regions.actions] {
            let corner = &buffer[(panel.x, panel.y)];
            assert_eq!(corner.fg, theme::color(&t, Role::Border), "{panel:?}");
            let title = &buffer[(panel.x + 2, panel.y)];
            assert_eq!(title.fg, theme::color(&t, Role::Text1), "{panel:?}");
            assert!(title.modifier.contains(Modifier::BOLD), "{panel:?}");
        }

        // → moves the fill down the list; the old row keeps no highlight.
        app.key(crossterm::event::KeyCode::Right.into()).unwrap();
        let buffer = screen(&app);
        assert!(filled(&buffer, rows.x + 4, selected + 1));
        assert_eq!(buffer[(rows.x + 4, selected)].bg, Color::Reset);
        app.key(crossterm::event::KeyCode::Char('0').into())
            .unwrap();
        let buffer = screen(&app);
        assert!(filled(&buffer, rows.x + 4, rows.y));
        assert!(text(&buffer, rows.y).starts_with("› 0 "));
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

    fn fixture(locale: &str) -> App {
        crate::tui::snapshot::fixture_app(locale)
    }
    fn spec(id: &str) -> Spec {
        crate::actions::find(id).unwrap()
    }
    /// Each Action_List row of `app` at 120×36 after the selection marker, without trailing
    /// blanks; en-US cells are all single width.
    fn action_texts(app: &App) -> Vec<String> {
        let rows = Regions::new(Rect::new(0, 0, 120, 36), app).action_rows;
        let buffer = terminal(app, 120, 36).backend().buffer().clone();
        (rows.y..rows.bottom())
            .map(|y| {
                (rows.x + 2..rows.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }
    fn action_row(rows: &[String], name: &str) -> String {
        rows.iter()
            .find(|row| row.starts_with(name))
            .cloned()
            .unwrap_or_else(|| panic!("{name} in {rows:?}"))
    }
    #[test]
    fn action_rows_end_with_a_last_result_or_a_related_reading() {
        use crate::runtime::Outcome;
        use std::time::Duration;
        let mut app = fixture("en-US");
        let width = Regions::new(Rect::new(0, 0, 120, 36), &app)
            .action_rows
            .width as usize
            - 2;
        let rows = action_texts(&app);
        // Form_Actions end their name with "…"; Row_Tags are right-aligned.
        for (name, tag) in [
            ("Set timezone…", "Asia/Shanghai"),
            ("Clean system…", "Disk 41%"),
            ("Create swap…", "existing 2.0 GiB"),
            ("Public IP / ISP lookup", "12s ago"),
        ] {
            let row = action_row(&rows, name);
            assert!(row.ends_with(tag), "{row:?}");
            assert_eq!(row.width(), width, "{row:?}");
        }
        assert_eq!(action_row(&rows, "Update packages"), "Update packages");
        // Tag Segments are joined by the theme's separator.
        let bbr = action_line(&app, &spec("bbr.status"), 60).to_string();
        assert!(
            bbr.starts_with("BBR and queue status ") && bbr.ends_with(" ○ Disabled · available v3"),
            "{bbr:?}"
        );
        assert_eq!(bbr.width(), 60);

        // A Last_Result takes the Row_Tag's place: its glyph in the outcome's color and its
        // Relative_Age.
        let t = app.theme.active.clone();
        let finished = app.now - Duration::from_secs(8 * 60);
        for (outcome, glyph, role) in [
            (Outcome::Done, "✓", Role::Success),
            (Outcome::Failed, "✗", Role::Danger),
            (Outcome::Cancelled, "■", Role::Warning),
        ] {
            app.records.finished(
                "system.timezone",
                outcome,
                Duration::from_secs(48),
                finished,
            );
            let row = action_row(&action_texts(&app), "Set timezone…");
            assert!(row.ends_with(&format!("{glyph} 8m ago")), "{row:?}");
            let line = action_line(&app, &spec("system.timezone"), width);
            let span = line
                .spans
                .iter()
                .find(|span| span.content == glyph)
                .unwrap();
            assert_eq!(span.style.fg, Some(theme::color(&t, role)), "{outcome:?}");
        }

        // The Optimization_View has no Row_Tag, and its counter skips the Rule and the
        // Execute_Row as the system category's skips its Section titles.
        let title = |app: &App| {
            let actions = Regions::new(Rect::new(0, 0, 120, 36), app).actions;
            let buffer = terminal(app, 120, 36).backend().buffer().clone();
            (actions.x..actions.right())
                .map(|x| buffer[(x, actions.y)].symbol())
                .collect::<String>()
        };
        assert!(title(&app).contains(" 1/12 "), "{}", title(&app));
        app.group = super::super::OPTIMIZATION_GROUP;
        let rows = action_texts(&app);
        assert!(action_row(&rows, "[ ] Set time").ends_with("Asia/Shanghai = current"));
        assert!(action_row(&rows, "[ ] Clean system…").ends_with("standard"));
        assert!(rows.iter().all(|row| !row.contains("8m ago")));
        assert!(title(&app).contains(" 1/11 "), "{}", title(&app));
    }
    #[test]
    fn a_narrow_row_drops_its_tag_before_clipping_the_name() {
        let app = fixture("en-US");
        let spec = spec("system.timezone");
        let name = "Set timezone…";
        let tag = "Asia/Shanghai";
        let text = |width: usize| action_line(&app, &spec, width).to_string();
        let full = name.width() + TAG_GAP + tag.width();
        assert_eq!(text(full + 3), format!("{name}     {tag}"));
        assert_eq!(text(full), format!("{name}  {tag}"));
        assert_eq!(text(full - 1), name, "the whole Row_Tag goes first");
        assert_eq!(text(name.width()), name);
        assert_eq!(
            text(name.width() - 1),
            "Set timezon…",
            "then the name is clipped"
        );
    }
    #[test]
    fn external_and_form_actions_mark_their_names_in_the_glyph_set() {
        use crate::theme::GlyphSet;
        let mut app = fixture("en-US");
        let external = crate::actions::specs()
            .into_iter()
            .find(|spec| spec.external)
            .expect("an external action");
        let name = app.ctx.i18n.text(&external.label()).to_owned();
        let suffix = if external.fields.is_empty() {
            ""
        } else {
            "…"
        };
        assert!(
            action_line(&app, &external, 80)
                .to_string()
                .starts_with(&format!("{name}{suffix} ↗")),
            "{}",
            action_line(&app, &external, 80)
        );
        app.theme.active.patterns.glyphs = GlyphSet::Ascii;
        let suffix = if external.fields.is_empty() {
            ""
        } else {
            "..."
        };
        assert!(
            action_line(&app, &external, 80)
                .to_string()
                .starts_with(&format!("{name}{suffix} ^"))
        );
        assert!(
            action_line(&app, &spec("system.timezone"), 80)
                .to_string()
                .starts_with("Set timezone...  ")
        );
    }
    #[test]
    fn the_clash_category_marks_a_running_or_unverified_mihomo() {
        let mut app = fixture("en-US");
        let regions = Regions::new(Rect::new(0, 0, 120, 36), &app);
        let rows = regions.category_rows;
        let clash = super::super::GROUPS
            .iter()
            .position(|group| *group == "clash")
            .unwrap();
        let y = rows.y
            + app
                .category_rows()
                .iter()
                .position(|row| *row == Some(clash))
                .unwrap() as u16;
        // The row's text and the color of its last visible cell.
        let mark = |app: &App| {
            let buffer = terminal(app, 120, 36).backend().buffer().clone();
            let cells = (rows.x..rows.right())
                .map(|x| &buffer[(x, y)])
                .collect::<Vec<_>>();
            let text = cells.iter().map(|cell| cell.symbol()).collect::<String>();
            let last = cells
                .iter()
                .rev()
                .find(|cell| cell.symbol() != " ")
                .unwrap();
            (text.trim_end().to_owned(), last.fg)
        };
        let t = app.theme.active.clone();
        let (text, fg) = mark(&app);
        assert!(text.ends_with("Mihomo proxy ●"), "{text:?}");
        assert_eq!(fg, theme::color(&t, Role::Success));
        app.snapshot.local.mihomo_state = Reading::Value(State::Unknown);
        let (text, fg) = mark(&app);
        assert!(text.ends_with("Mihomo proxy !"), "{text:?}");
        assert_eq!(fg, theme::color(&t, Role::Warning));
        for state in [
            Reading::Value(State::Stopped),
            Reading::Value(State::NotInstalled),
            Reading::Unavailable("invalid PID file".into()),
            Reading::Loading,
        ] {
            app.snapshot.local.mihomo_state = state;
            assert!(mark(&app).0.ends_with("Mihomo proxy"), "{:?}", mark(&app).0);
        }
    }

    /// Select `id` in its category.
    fn select(app: &mut App, id: &str) {
        let group = super::super::GROUPS
            .iter()
            .position(|group| *group == spec(id).group)
            .unwrap();
        app.group = group;
        app.positions[group] = app
            .rows(group)
            .iter()
            .filter(|row| row.selectable())
            .position(|row| matches!(row, Row::Action(spec) if spec.id == id))
            .unwrap();
    }
    /// The Details_Panel drawn at `width`×`height`: its rows inside the border and padding
    /// without trailing blanks, and the foreground of the first cell of a row's `text`.
    /// en-US cells are all single width.
    struct Details {
        rows: Vec<String>,
        inner: Rect,
        buffer: ratatui::buffer::Buffer,
    }
    impl Details {
        fn of(app: &App, width: u16, height: u16) -> Self {
            let panel = Regions::new(Rect::new(0, 0, width, height), app).details;
            let inner = Rect::new(panel.x + 2, panel.y + 1, panel.width - 4, panel.height - 2);
            let buffer = terminal(app, width, height).backend().buffer().clone();
            let rows = (inner.y..inner.bottom())
                .map(|y| {
                    (inner.x..inner.right())
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect();
            Self {
                rows,
                inner,
                buffer,
            }
        }
        fn fg(&self, text: &str) -> Color {
            let (y, at) = self
                .rows
                .iter()
                .enumerate()
                .find_map(|(y, row)| row.find(text).map(|at| (y, row[..at].chars().count())))
                .unwrap_or_else(|| panic!("{text:?} in {:?}", self.rows));
            self.buffer[(self.inner.x + at as u16, self.inner.y + y as u16)].fg
        }
    }

    #[test]
    fn the_details_panel_reads_top_down_with_chips_and_the_disk_gauge() {
        use crate::runtime::Outcome;
        let mut app = fixture("en-US");
        select(&mut app, "system.clean");
        let t = app.theme.active.clone();
        let panel = Details::of(&app, 120, 36);
        let width = panel.inner.width as usize;
        // The disk gauge spans the value column after "Disk" and the gap.
        let Reading::Value(disk) = &app.snapshot.local.disk else {
            panic!("the fixture reads the disk");
        };
        let bar = theme::bar(&t, disk.percent(), width - 6, Role::Meter)
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let mut expected = vec![
            "Clean system".to_owned(),
            "Changes system or user files · Requires root".into(),
            // The fixture runs as a user, and system.clean needs root.
            "! This action requires root privileges: sudo cqs".into(),
            String::new(),
        ];
        expected.extend(wrap_words(
            app.ctx.i18n.text("description.system.clean"),
            width,
        ));
        expected.extend([
            String::new(),
            "Current state".into(),
            format!("Disk  {bar}"),
            "      41% · 16.4 GiB / 40.0 GiB · available 23.6 GiB".into(),
            String::new(),
            "Parameters".into(),
            "Cleanup scope  ● standard  ○ full".into(),
            String::new(),
            "sudo cqs run system.clean --set scope=standard".into(),
            String::new(),
            "Last result  Not run in this session".into(),
        ]);
        let rows = &panel.rows;
        assert_eq!(rows[..expected.len()], expected[..], "{rows:#?}");
        // The Enter hint is pinned to the bottom row, dimmed while the action cannot run.
        assert!(
            rows[expected.len()..rows.len() - 1]
                .iter()
                .all(String::is_empty)
        );
        assert_eq!(rows.last().unwrap(), "↵ configure and run");
        assert_eq!(panel.fg("↵ configure"), theme::color(&t, Role::Text3));
        assert_eq!(panel.fg("━"), theme::color(&t, Role::Meter));
        assert_eq!(panel.fg("● standard"), theme::color(&t, Role::Accent));
        assert_eq!(panel.fg("○ full"), theme::color(&t, Role::Text3));
        assert_eq!(panel.fg("sudo cqs run"), theme::color(&t, Role::Accent));

        // The saved values, and the Last_Result with its age and elapsed time.
        app.parameters.insert(
            "system.clean".into(),
            Parameters::from([("scope".to_owned(), "full".to_owned())]),
        );
        app.records.finished(
            "system.clean",
            Outcome::Done,
            Duration::from_secs(48),
            app.now - Duration::from_secs(8 * 60),
        );
        let rows = Details::of(&app, 120, 36).rows;
        for row in [
            "Cleanup scope  ○ standard  ● full",
            "sudo cqs run system.clean --set scope=full",
            "Last result  ✓ Completed · 8m ago · took 00:48",
        ] {
            assert!(
                rows.iter().any(|shown| shown == row),
                "{row:?} in {rows:#?}"
            );
        }

        // With the inline logs open the panel has 13 rows: the blank lines between parts
        // go first, then the description after its first line.
        app.logs_open = true;
        app.log("working");
        let rows = Details::of(&app, 120, 36).rows;
        assert_eq!(rows.len(), 13);
        assert!(rows[..11].iter().all(|row| !row.is_empty()), "{rows:#?}");
        assert!(rows[3].ends_with('…'), "{:?}", rows[3]);
        assert_eq!(rows[4], "Current state");
        assert_eq!(rows[10], "Last result  ✓ Completed · 8m ago · took 00:48");
        assert_eq!(rows[12], "↵ configure and run");
    }
    #[test]
    fn a_listed_field_is_chosen_from_a_list_when_its_form_opens() {
        let mut app = fixture("en-US");
        select(&mut app, "clash.sub.use");
        let t = app.theme.active.clone();
        let panel = Details::of(&app, 120, 36);
        let rows = &panel.rows;
        let at = rows.iter().position(|row| row == "Parameters").unwrap();
        assert_eq!(
            rows[at + 1..at + 4],
            [
                "Subscription name  chosen from a list when opened",
                "",
                "cqs run clash.sub.use --set name=<name>",
            ]
        );
        // The name is still missing, so the command is tertiary.
        assert_eq!(panel.fg("cqs run"), theme::color(&t, Role::Text3));
        assert_eq!(rows.last().unwrap(), "↵ configure and run");

        // A saved name no longer fits beside the label, so the value goes under it.
        app.parameters.insert(
            "clash.sub.use".into(),
            Parameters::from([("name".to_owned(), "work".to_owned())]),
        );
        let panel = Details::of(&app, 120, 36);
        let rows = &panel.rows;
        let at = rows.iter().position(|row| row == "Parameters").unwrap();
        assert_eq!(
            rows[at + 1..at + 5],
            [
                "Subscription name",
                "  work · chosen from a list when opened",
                "",
                "cqs run clash.sub.use --set name=work",
            ]
        );
        assert_eq!(panel.fg("cqs run"), theme::color(&t, Role::Accent));
    }
    #[test]
    fn the_details_footer_says_what_enter_does() {
        use crate::theme::GlyphSet;
        let mut app = fixture("en-US");
        app.root = true;
        let t = app.theme.active.clone();
        for (id, hint) in [
            ("system.info", "↵ open overview"),
            ("system.public_ip", "↵ look up public IP"),
            ("system.mirror.apply", "↵ configure and run"),
            ("system.update", "↵ run"),
        ] {
            select(&mut app, id);
            let panel = Details::of(&app, 120, 36);
            assert_eq!(panel.rows.last().unwrap(), hint, "{id}");
            assert_eq!(panel.fg("↵"), theme::color(&t, Role::Accent), "{id}");
        }
        app.theme.active.patterns.glyphs = GlyphSet::Ascii;
        assert_eq!(Details::of(&app, 120, 36).rows.last().unwrap(), "Enter run");
        // The Optimization_View now shows the plan, with its batch and recommendation keys.
        app.theme.active = t;
        app.group = super::super::OPTIMIZATION_GROUP;
        let rows = Details::of(&app, 120, 36).rows;
        assert!(!rows.iter().any(|row| row.starts_with('↵')), "{rows:#?}");
        assert_eq!(rows.last().unwrap(), "X review and run  A use recommended");
    }
    #[test]
    fn below_112_columns_the_stacked_panel_leads_with_name_impact_and_state() {
        let mut app = fixture("en-US");
        // Everything fits: name, impact, description and command; no state to show.
        select(&mut app, "system.info");
        let panel = Details::of(&app, SIDE_BY_SIDE - 1, 24);
        assert_eq!(panel.inner.height, 4);
        assert_eq!(
            panel.rows,
            [
                "System overview",
                "Read only · Current user",
                "Read host, CPU, memory, disk, IP and BBR status.",
                "cqs run system.info",
            ]
        );
        // Too much: the description keeps its first line, and the command is cut.
        select(&mut app, "system.clean");
        let rows = Details::of(&app, SIDE_BY_SIDE - 1, 24).rows;
        assert_eq!(
            rows[..3],
            [
                "Clean system",
                "Changes system or user files · Requires root",
                "Disk 41% · 16.4 GiB / 40.0 GiB · available 23.6 GiB",
            ]
        );
        let description = app.ctx.i18n.text("description.system.clean");
        let first = rows[3].strip_suffix('…').expect("an ellipsis");
        assert!(description.starts_with(first), "{:?}", rows[3]);
        // The state line keeps as many whole readings as fit.
        select(&mut app, "system.public_ip");
        assert_eq!(
            Details::of(&app, 80, 24).rows[2],
            "IPv4 203.0.113.24 · IPv6 2001:db8:3c4d:15::1a2f"
        );
    }

    mod segment_property {
        use super::super::{SEPARATOR, Segment, fit_segments, segment_spans};
        use proptest::prelude::*;
        use ratatui::style::{Color, Style};
        use unicode_width::UnicodeWidthStr;

        /// Text of Latin letters, digits, CJK ideographs (two cells each) and the narrow
        /// symbols status rows draw, including the separator dot and the ellipsis.
        fn text() -> impl Strategy<Value = String> {
            proptest::collection::vec(
                prop_oneof![
                    3 => proptest::char::range('a', 'z'),
                    1 => proptest::char::range('0', '9'),
                    3 => proptest::char::range('\u{4e00}', '\u{9fa5}'),
                    1 => proptest::sample::select(vec![' ', '·', '/', '●', '○', '…', '–']),
                ],
                0..12,
            )
            .prop_map(String::from_iter)
        }
        fn segment() -> impl Strategy<Value = Segment> {
            (text(), 0u8..8).prop_map(|(text, color)| {
                Segment::new(text, Style::new().fg(Color::Indexed(color)))
            })
        }
        /// Cells the first `count` Segments take with the separators between them.
        fn cells(segments: &[Segment], count: usize) -> usize {
            segments[..count].iter().map(Segment::width).sum::<usize>()
                + SEPARATOR.width() * count.saturating_sub(1)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]

            // Feature: tui-redesign, Property 12: Segment 截断
            // **Validates: Requirements 4.6, 4.7**
            #[test]
            fn segments_shorten_whole_from_the_end_and_never_overflow(
                segments in proptest::collection::vec(segment(), 0..8),
                width in 1usize..80,
            ) {
                let count = fit_segments(&segments, width);
                prop_assert!(count <= segments.len());
                prop_assert!(cells(&segments, count) <= width);
                if count < segments.len() {
                    prop_assert!(cells(&segments, count + 1) > width, "one more would fit");
                }
                let separator = Style::new().fg(Color::Indexed(9));
                let spans = segment_spans(&segments, width, separator);
                let line = spans.iter().map(|span| span.content.as_ref()).collect::<String>();
                prop_assert!(line.width() <= width, "{line:?} is wider than {width}");
                if count == 0 && !segments.is_empty() {
                    // Only the first Segment, cut short with an ellipsis.
                    let first = &segments[0];
                    prop_assert_eq!(spans.len(), 1);
                    prop_assert_eq!(spans[0].style, first.style);
                    let kept = line.strip_suffix('…');
                    prop_assert!(
                        kept.is_some_and(|kept| first.text.starts_with(kept)),
                        "{line:?} does not clip {:?}",
                        first.text
                    );
                } else {
                    let texts = segments[..count]
                        .iter()
                        .map(|segment| segment.text.as_str())
                        .collect::<Vec<_>>();
                    prop_assert_eq!(line, texts.join(SEPARATOR));
                    for (index, span) in spans.iter().enumerate() {
                        let style = if index % 2 == 0 { segments[index / 2].style } else { separator };
                        prop_assert_eq!(span.style, style);
                    }
                }
            }
        }
    }

    /// `app` with its Command_Palette open on `query`, `cursor` hits down.
    fn with_palette(mut app: App, query: &str, cursor: usize) -> App {
        app.command_palette = Some(crate::tui::palette::CommandPalette {
            query: InputBuffer::new(query.into()),
            cursor,
        });
        app
    }
    /// The palette's result rows at 120×36 after the highlight symbol, without trailing
    /// blanks, and the buffer they were read from.
    fn palette_rows(app: &App) -> (Vec<String>, ratatui::buffer::Buffer, CommandPaletteGeometry) {
        let area = Rect::new(0, 0, 120, 36);
        let geometry = CommandPaletteGeometry::new(area, app, &app.palette_results());
        let buffer = terminal(app, 120, 36).backend().buffer().clone();
        let rows = geometry.rows;
        let texts = (rows.y..rows.bottom())
            .map(|y| {
                (rows.x + 2..rows.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        (texts, buffer, geometry)
    }
    /// The first cell of `text` on screen row `y` of the palette's result rows.
    fn cell_of(buffer: &ratatui::buffer::Buffer, rows: Rect, y: u16, text: &str) -> Position {
        let cells = (rows.x + 2..rows.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect::<Vec<_>>();
        let at = (0..cells.len())
            .find(|start| cells[*start..].concat().starts_with(text))
            .unwrap_or_else(|| panic!("{text:?} in {:?}", cells.concat()));
        Position::new(rows.x + 2 + at as u16, y)
    }

    #[test]
    fn palette_rows_mark_matches_and_show_tags_root_and_the_exec_button() {
        let i = I18n::load(Some("en-US"), None).unwrap();
        // The fixture runs as a normal user, so Actions that need root cannot run.
        let app = with_palette(fixture("en-US"), "bbr", 2);
        let t = app.theme.active.clone();
        let (rows, buffer, geometry) = palette_rows(&app);
        let total = crate::actions::specs().len();
        let title = (geometry.popup.x..geometry.popup.right())
            .map(|x| buffer[(x, geometry.popup.y)].symbol())
            .collect::<String>();
        assert!(title.contains(" Search actions "), "{title:?}");
        assert!(title.contains(&format!(" 5 / {total} ")), "{title:?}");

        let width = geometry.rows.width as usize - 2;
        let exec = " run ↵ ";
        let room = width - exec.width() - 1;
        assert_eq!(rows[0], i.text("group.kernel"));
        assert_eq!(rows[3], i.text("group.network"));
        let kernel = &rows[1];
        assert!(kernel.starts_with("Install BBRv3 kernel…") && kernel.ends_with("root"));
        assert_eq!(kernel.width(), room, "{kernel:?}");
        // The Related_Reading's tag, right-aligned before the button's place; root after it.
        let enable = &rows[5];
        assert!(
            enable.starts_with("Enable BBR in the running kernel…"),
            "{enable:?}"
        );
        assert!(
            enable.ends_with(" ○ Disabled · available v3  root"),
            "{enable:?}"
        );
        assert_eq!(enable.width(), room, "{enable:?}");
        // Only the highlighted row carries the button, at the end of the row.
        let status = &rows[4];
        assert!(status.starts_with("BBR and queue status "), "{status:?}");
        assert!(
            status.ends_with(" ○ Disabled · available v3  run ↵"),
            "{status:?}"
        );
        let y = geometry.rows.y + 4;
        let button = cell_of(&buffer, geometry.rows, y, exec);
        assert_eq!(button.x + exec.width() as u16, geometry.rows.right());
        assert!(!rows[1].contains("run ↵") && !rows[5].contains("run ↵"));
        assert_eq!(buffer[button].bg, theme::color(&t, Role::Accent));
        assert_eq!(buffer[button].fg, theme::color(&t, Role::OnAccent));
        assert!(geometry.exec_at(button) && !geometry.exec_at(Position::new(button.x, y + 1)));

        // Matches in accent; an Action that cannot run in text3, its matches in bold.
        let unhighlighted = geometry.rows.y + 5;
        let enable_bbr = cell_of(&buffer, geometry.rows, unhighlighted, "BBR");
        let enable_word = cell_of(&buffer, geometry.rows, unhighlighted, "Enable");
        let text3 = theme::color(&t, Role::Text3);
        assert_eq!(buffer[enable_word].fg, text3);
        assert_eq!(buffer[enable_bbr].fg, text3);
        assert!(buffer[enable_bbr].modifier.contains(Modifier::BOLD));
        let root = cell_of(&buffer, geometry.rows, unhighlighted, "root");
        assert_eq!(buffer[root].fg, text3);
        let mut runnable = with_palette(fixture("en-US"), "bbr", 2);
        runnable.root = true;
        let (_, buffer, geometry) = palette_rows(&runnable);
        let y = geometry.rows.y + 5;
        let bbr = cell_of(&buffer, geometry.rows, y, "BBR");
        assert_eq!(buffer[bbr].fg, theme::color(&t, Role::Accent));
        assert!(buffer[bbr].modifier.contains(Modifier::BOLD));
        let word = cell_of(&buffer, geometry.rows, y, "Enable");
        assert_ne!(buffer[word].fg, text3);
        let root = cell_of(&buffer, geometry.rows, y, "root");
        assert_eq!(buffer[root].fg, theme::color(&t, Role::Warning));

        // A highlighted Action that cannot run keeps text3 on the selection fill.
        let app = with_palette(fixture("en-US"), "bbr", 0);
        let (_, buffer, geometry) = palette_rows(&app);
        let y = geometry.rows.y + 1;
        let name = cell_of(&buffer, geometry.rows, y, "Install");
        assert_eq!(buffer[name].fg, text3);
        assert_eq!(buffer[name].bg, theme::color(&t, Role::DialogSelection));
        let button = cell_of(&buffer, geometry.rows, y, exec);
        assert_eq!(buffer[button].fg, text3);

        // A hit by ID alone also shows the ID, in text3 with its match in accent.
        let mut app = with_palette(fixture("en-US"), "BBR.", 0);
        app.root = true;
        let (rows, buffer, geometry) = palette_rows(&app);
        assert_eq!(rows[0], i.text("group.network"));
        let restore = &rows[3];
        assert!(
            restore.starts_with("Restore BBR settings  bbr.restore "),
            "{restore:?}"
        );
        assert!(restore.ends_with(" root"), "{restore:?}");
        let y = geometry.rows.y + 3;
        let id = cell_of(&buffer, geometry.rows, y, "bbr.");
        assert_eq!(buffer[id].fg, theme::color(&t, Role::Accent));
        let rest = cell_of(&buffer, geometry.rows, y, "restore");
        assert_eq!(buffer[rest].fg, text3);
        let name = cell_of(&buffer, geometry.rows, y, "BBR set");
        assert_ne!(
            buffer[name].fg,
            theme::color(&t, Role::Accent),
            "the name did not match"
        );
    }

    #[test]
    fn an_empty_palette_says_what_to_type_and_lists_recent_use_once_something_ran() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = with_palette(fixture(locale), "", 0);
            let i = &app.ctx.i18n;
            let (hint, no_match) = (
                i.text("ui.palette_hint").replace(' ', ""),
                i.text("ui.no_match").replace(' ', ""),
            );
            let total = crate::actions::specs().len();
            let geometry = |app: &App, width, height| {
                CommandPaletteGeometry::new(
                    Rect::new(0, 0, width, height),
                    app,
                    &app.palette_results(),
                )
            };
            for (width, height) in [(80, 24), (120, 36)] {
                let popup = geometry(&app, width, height).popup;
                let shown = region(&app, width, height, popup);
                assert!(shown.contains(&hint), "{locale} {width}×{height}: {shown}");
                assert!(shown.contains(&format!("0/{total}")), "{shown}");
                assert!(shown.contains(&i.text("ui.palette_title").replace(' ', "")));
            }

            app.records.started("tcp.tune");
            app.records.started("bbr.status");
            let popup = geometry(&app, 80, 24).popup;
            let rows = geometry(&app, 80, 24).rows;
            let shown = region(&app, 80, 24, popup);
            assert!(!shown.contains(&hint), "{shown}");
            assert!(shown.contains(&format!("2/{total}")), "{shown}");
            let listed = region(&app, 80, 24, rows);
            let names = ["bbr.status", "tcp.tune"]
                .map(|id| app.ctx.i18n.text(&format!("action.{id}")).replace(' ', ""));
            let first = listed.find(&names[0]).expect("bbr.status");
            let second = listed.find(&names[1]).expect("tcp.tune");
            assert!(first < second, "the newest first: {listed}");
            assert!(!listed.contains(&app.ctx.i18n.text("group.network").replace(' ', "")));

            let app = with_palette(fixture(locale), "zzzz", 0);
            let shown = region(&app, 80, 24, geometry(&app, 80, 24).popup);
            assert!(
                shown.contains(&no_match) && !shown.contains(&hint),
                "{shown}"
            );
            assert!(shown.contains(&format!("0/{total}")), "{shown}");
        }
    }
}
