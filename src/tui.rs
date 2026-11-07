mod detect;
mod input;
mod motion;
mod palette;
mod preview;
#[cfg(test)]
mod progress_tests;
mod related;
mod sections;
mod session;
#[cfg(test)]
mod snapshot;
mod style_panel;
#[cfg(test)]
mod theme_tests;
mod view;

use crate::{
    actions::{self, Kind, Listing, Parameters, Source, Spec},
    config::Preferences,
    dashboard::{Monitor, Snapshot},
    error::{Error, Result},
    ops::Choice,
    runtime::{Context, Event, Outcome, Step, StepState},
    theme::{self, Background, ThemeSource, ThemeState, builtin},
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as Input, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::InputBuffer;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Position, Rect},
};
use session::SessionRecords;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const GROUPS: &[&str] = &[
    "system",
    "network",
    "kernel",
    "ssh",
    "docker",
    "tools",
    "ai",
    "terminal",
    "tests",
    "app",
    "clash",
    "optimization",
];
/// Optimization_Category: pinned above the Reorderable_Categories, never reordered.
const OPTIMIZATION_GROUP: usize = GROUPS.len() - 1;
/// Default order of the Reorderable_Categories, i.e. every group except optimization.
const DEFAULT_CATEGORY_ORDER: &[&str] = &[
    "system", "kernel", "network", "ssh", "docker", "clash", "tools", "ai", "terminal", "tests",
    "app",
];
const RECOMMENDED: &[&str] = &["system.update", "system.clean", "tcp.tune", "bbr.enable"];

#[derive(Debug)]
enum Task {
    Single {
        spec: Spec,
        params: Parameters,
        reviewed: bool,
    },
    Batch(Vec<(Spec, Parameters)>),
}

impl Task {
    fn jobs(&self) -> Vec<(&Spec, &Parameters)> {
        match self {
            Self::Single { spec, params, .. } => vec![(spec, params)],
            Self::Batch(jobs) => jobs.iter().map(|(spec, params)| (spec, params)).collect(),
        }
    }
}

fn default_category_order() -> Vec<usize> {
    let mut order = DEFAULT_CATEGORY_ORDER
        .iter()
        .map(|id| {
            GROUPS
                .iter()
                .position(|group| group == id)
                .expect("category ID")
        })
        .collect::<Vec<_>>();
    for index in (0..GROUPS.len()).filter(|index| *index != OPTIMIZATION_GROUP) {
        if !order.contains(&index) {
            order.push(index);
        }
    }
    order
}

/// Older versions saved `optimization` in the list; it is pinned now, so those entries are
/// dropped wherever they appear. The result holds only Reorderable_Categories.
fn category_order_from_ids(ids: &[String]) -> Result<Vec<usize>> {
    let mut seen = [false; GROUPS.len()];
    let mut order = Vec::with_capacity(DEFAULT_CATEGORY_ORDER.len());
    for id in ids.iter().filter(|id| *id != GROUPS[OPTIMIZATION_GROUP]) {
        let Some(index) = GROUPS.iter().position(|group| *group == id) else {
            return Err(Error::new(
                "error.config",
                format!("unknown category: {id}"),
            ));
        };
        if seen[index] {
            return Err(Error::new(
                "error.config",
                format!("duplicate category: {id}"),
            ));
        }
        seen[index] = true;
        order.push(index);
    }
    for index in default_category_order() {
        if !seen[index] {
            order.push(index);
        }
    }
    Ok(order)
}

/// A row of a category's Action_List. Section titles and the Rule are never selected.
#[derive(Debug, Clone)]
enum Row {
    /// The title of a Section, by its ID in the Section table.
    Section(&'static str),
    Rule,
    Action(Spec),
    /// Execute_Row: runs the Checklist; only in the Optimization_View.
    Execute,
}
impl Row {
    /// Selectable_Row: an action or the Execute_Row.
    fn selectable(&self) -> bool {
        matches!(self, Self::Action(_) | Self::Execute)
    }
}
/// Index in `rows` of the `position`-th Selectable_Row.
fn row_index(rows: &[Row], position: usize) -> usize {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.selectable())
        .nth(position)
        .map(|(index, _)| index)
        .expect("positions name a Selectable_Row")
}
/// The Selectable_Row number of `rows[index]`; `None` for a Section title, the Rule or a
/// row past the end.
fn row_position(rows: &[Row], index: usize) -> Option<usize> {
    rows.get(index)
        .filter(|row| row.selectable())
        .map(|_| rows[..index].iter().filter(|row| row.selectable()).count())
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
impl Phase {
    fn key(self) -> &'static str {
        match self {
            Self::Idle => "ui.ready",
            Self::Running => "ui.running",
            Self::Cancelling => "ui.cancelling",
            Self::Completed => "ui.done",
            Self::Failed => "ui.failed",
            Self::Cancelled => "ui.cancelled",
        }
    }
    fn tone(self) -> Tone {
        match self {
            Self::Idle => Tone::Muted,
            Self::Running => Tone::Info,
            Self::Cancelling | Self::Cancelled => Tone::Warning,
            Self::Completed => Tone::Success,
            Self::Failed => Tone::Danger,
        }
    }
}
/// Semantic color roles; the view maps them to concrete colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Muted,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}
/// Non-blocking feedback for actions whose result is otherwise invisible.
struct Toast {
    text: String,
    tone: Tone,
    shown: Instant,
}
/// A log line keeps its tone so the final outcome stands out from tool output.
#[derive(Debug, Clone)]
struct LogLine {
    text: String,
    tone: Tone,
    bold: bool,
}
#[derive(Debug)]
enum Effect {
    None,
    Quit,
    Run(Task),
    Refresh,
    PublicIp,
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum FormMode {
    Run,
    Optimization,
}
struct Form {
    spec: Spec,
    values: Vec<InputBuffer>,
    index: usize,
    mode: FormMode,
    error: Option<String>,
    /// Existing items for each field whose source is a listing; empty for other fields.
    options: Vec<Vec<Choice>>,
    /// The open list for the active field, if any.
    picker: Option<Picker>,
}
impl Form {
    fn parameters(&self) -> Parameters {
        self.spec
            .fields
            .iter()
            .zip(&self.values)
            .map(|(field, value)| (field.name.to_owned(), value.text.clone()))
            .collect()
    }
    fn chip_field(&self, index: usize) -> bool {
        let field = &self.spec.fields[index];
        !field.choices.is_empty() && !field.allow_custom
    }
    /// Listed fields name existing items, so they are chosen from a list, never typed.
    fn listed(&self, index: usize) -> bool {
        self.spec.fields[index].source.is_some()
    }
    /// The items a listed field can take: its own listing, or the children of the item
    /// chosen in the field it depends on.
    fn choices(&self, index: usize) -> &[Choice] {
        match self.spec.fields[index].source {
            None => &[],
            Some(Source::Listing(_)) => &self.options[index],
            Some(Source::Children(parent)) => {
                let parent = self
                    .spec
                    .fields
                    .iter()
                    .position(|field| field.name == parent)
                    .expect("a dependent field names an earlier field");
                let chosen = &self.values[parent].text;
                self.choices(parent)
                    .iter()
                    .find(|choice| choice.value == *chosen)
                    .map(|choice| choice.children.as_slice())
                    .unwrap_or_default()
            }
        }
    }
    /// Store loaded items and settle every listed field on a value it can take, parents
    /// first: the value it already holds if still listed, else the current item, else the
    /// first. A held value can be stale, such as a removed subscription.
    fn attach(&mut self, options: Vec<Vec<Choice>>) {
        self.options = options;
        for index in 0..self.values.len() {
            if self.listed(index) {
                let value = initial(self.choices(index), &self.values[index].text);
                self.values[index].replace(value);
            }
        }
    }
    /// Set a listed field and reset the fields that depend on it, whose old values belonged
    /// to a different parent item.
    fn choose(&mut self, index: usize, value: String) {
        self.values[index].replace(value);
        let name = self.spec.fields[index].name;
        for dependent in 0..self.values.len() {
            if self.spec.fields[dependent].source == Some(Source::Children(name)) {
                let value = initial(self.choices(dependent), "");
                self.choose(dependent, value);
            }
        }
    }
    fn open_picker(&mut self, filter: String, with_form: bool) {
        let mut picker = Picker {
            filter: InputBuffer::new(filter),
            cursor: 0,
            with_form,
        };
        let held = &self.values[self.index].text;
        picker.cursor = picker
            .matches(self.choices(self.index))
            .iter()
            .position(|choice| choice.value == *held)
            .unwrap_or(0);
        self.picker = Some(picker);
    }
}
fn initial(choices: &[Choice], held: &str) -> String {
    choices
        .iter()
        .find(|choice| choice.value == held)
        .or_else(|| choices.iter().find(|choice| choice.current))
        .or_else(|| choices.first())
        .map(|choice| choice.value.clone())
        .unwrap_or_default()
}
/// A filterable list over the active field's items.
struct Picker {
    filter: InputBuffer,
    /// Position within the filtered matches.
    cursor: usize,
    /// Opened together with its form, so leaving the list leaves the form too.
    with_form: bool,
}
impl Picker {
    /// Case-insensitive substring match on the value.
    fn matches<'a>(&self, choices: &'a [Choice]) -> Vec<&'a Choice> {
        let filter = self.filter.text.to_lowercase();
        choices
            .iter()
            .filter(|choice| choice.value.to_lowercase().contains(&filter))
            .collect()
    }
}
/// Items for a form being read in the background before it opens.
struct Loading {
    spec: Spec,
    mode: FormMode,
    receiver: Receiver<Result<Vec<Vec<Choice>>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}
/// Move through `count` items with wrap-around; with nothing selected, start at an end.
fn step(current: Option<usize>, count: usize, forward: bool) -> usize {
    match (current, forward) {
        (Some(current), true) => (current + 1) % count,
        (Some(current), false) => (current + count - 1) % count,
        (None, true) => 0,
        (None, false) => count - 1,
    }
}
fn empty_listing(listing: Listing) -> &'static str {
    match listing {
        Listing::Subscriptions => "toast.no_subscriptions",
        Listing::ProxyGroups => "toast.no_proxy_groups",
        Listing::BootKernels => "toast.no_boot_kernels",
        Listing::RemovableKernels => "toast.no_removable_kernels",
    }
}
struct Confirmation {
    text: String,
    token: String,
    input: InputBuffer,
    reply: Sender<bool>,
    scroll: u16,
    accept: bool,
}

pub struct App {
    ctx: Context,
    group: usize,
    category_order: Vec<usize>,
    /// Per category, the selected row counted among its Selectable_Rows only, so a
    /// selection never rests on a Section title or the Rule.
    positions: [usize; GROUPS.len()],
    marked: BTreeSet<String>,
    parameters: BTreeMap<String, Parameters>,
    logs: VecDeque<LogLine>,
    scroll: u16,
    logs_open: bool,
    log_modal: bool,
    phase: Phase,
    task_ids: Vec<String>,
    form: Option<Form>,
    loading: Option<Loading>,
    confirmation: Option<Confirmation>,
    notice: Option<String>,
    overview: bool,
    overview_scroll: u16,
    worker: Option<JoinHandle<()>>,
    exit_requested: bool,
    receiver: Option<Receiver<Event>>,
    pub snapshot: Snapshot,
    platform_error: Option<Error>,
    root: bool,
    completed_since_refresh: bool,
    viewport: Rect,
    /// Frame clock, set once per loop iteration so drawing never reads the system clock.
    now: Instant,
    started: Instant,
    toast: Option<Toast>,
    help: bool,
    /// The open Command_Palette, an Overlay that owns input (design component 5).
    command_palette: Option<palette::CommandPalette>,
    style_panel: Option<style_panel::StylePanel>,
    dialog_opened: Option<Instant>,
    dialog_kind: u8,
    hover: Option<Position>,
    gauges: [Option<motion::Tween>; 4],
    /// A user-requested refresh in flight; cleared when a newer sample arrives.
    refreshing: Option<Instant>,
    /// Drawing reads `theme.active`; a change shows from the next frame.
    theme: ThemeState,
    /// Last_Result and Recent_Use for this process; never written to a file.
    records: SessionRecords,
    /// Action ID and wall-clock start of the running single task. `None` while idle and
    /// during a batch, whose steps are recorded from Step events instead (task 13.2).
    single_task: Option<(&'static str, Instant)>,
    progress: Option<view::plan::Progress>,
}
fn initial_theme() -> ThemeState {
    let background = Background::Unavailable("background detection is not enabled".into());
    ThemeState {
        active: theme::resolve(&builtin::aurora(), &background),
        definitions: builtin::all(),
        saved_id: None,
        source: ThemeSource::Default,
        error: None,
        background,
    }
}
impl App {
    pub fn new(ctx: Context) -> Self {
        let platform_error = ctx.platform().err();
        #[cfg(unix)]
        let root = unsafe { libc::geteuid() } == 0;
        #[cfg(not(unix))]
        let root = false;
        let now = Instant::now();
        Self {
            ctx,
            group: 0,
            category_order: default_category_order(),
            positions: [0; GROUPS.len()],
            marked: BTreeSet::new(),
            parameters: BTreeMap::new(),
            logs: VecDeque::new(),
            scroll: 0,
            logs_open: false,
            log_modal: false,
            phase: Phase::Idle,
            task_ids: Vec::new(),
            form: None,
            loading: None,
            confirmation: None,
            notice: None,
            overview: false,
            overview_scroll: 0,
            worker: None,
            exit_requested: false,
            receiver: None,
            snapshot: Snapshot::default(),
            platform_error,
            root,
            completed_since_refresh: false,
            viewport: Rect::new(0, 0, 120, 36),
            now,
            started: now,
            toast: None,
            help: false,
            command_palette: None,
            style_panel: None,
            dialog_opened: None,
            dialog_kind: 0,
            hover: None,
            gauges: [None; 4],
            refreshing: None,
            theme: initial_theme(),
            records: SessionRecords::default(),
            single_task: None,
            progress: None,
        }
    }
    fn elapsed(&self) -> Duration {
        self.now.saturating_duration_since(self.started)
    }
    fn spin(&self) -> &'static str {
        view::spinner(&self.theme.active, self.elapsed())
    }

    pub fn preview_theme(&mut self, id: &str, background: Background) -> Result<()> {
        let definition = builtin::all()
            .into_iter()
            .find(|definition| definition.id == id)
            .ok_or_else(|| Error::new("error.input", id))?;
        self.theme.active = theme::resolve(&definition, &background);
        self.theme.background = background;
        Ok(())
    }

    fn configure_theme(
        &mut self,
        flag: Option<&str>,
        environment: Option<&str>,
        background: Background,
        preferences: &Preferences,
    ) {
        let customs = theme::file::load_dir(&self.ctx.paths.themes);
        let builtins = builtin::all();
        let (id, source) =
            theme::resolve::resolve_id(flag, environment, preferences.theme.as_deref());
        let (definition, error) = theme::resolve::select(&id, source, &builtins, &customs);
        let mut definitions = builtins;
        definitions.extend(customs.into_iter().filter_map(|custom| custom.result.ok()));
        self.theme = ThemeState {
            definitions,
            active: theme::resolve(&definition, &background),
            saved_id: preferences.theme.clone(),
            source,
            error: error.clone(),
            background,
        };
        if let Some(detail) = error {
            self.notify(
                self.ctx
                    .i18n
                    .format("toast.theme_error", &[("detail", &detail)]),
                Tone::Warning,
            );
        }
    }

    fn open_style(&mut self) {
        self.style_panel = Some(style_panel::StylePanel::new(
            self.theme.active.clone(),
            &mut self.theme.definitions,
            theme::file::load_dir(&self.ctx.paths.themes),
        ));
    }

    fn preview_style(&mut self, cursor: usize) {
        let panel = self.style_panel.as_mut().expect("open style panel");
        panel.cursor = cursor.min(panel.entries.len() - 1);
        if let Some(theme) = panel.preview(&self.theme) {
            self.theme.active = theme;
        }
    }

    fn style_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let panel = self.style_panel.as_ref().expect("open style panel");
        let last = panel.entries.len() - 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.preview_style(panel.cursor.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.preview_style((panel.cursor + 1).min(last)),
            KeyCode::Home => self.preview_style(0),
            KeyCode::End => self.preview_style(last),
            KeyCode::Esc => {
                self.theme.active = self.style_panel.take().unwrap().previous;
            }
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Enter => {
                let entry = &panel.entries[panel.cursor];
                let definition = match entry.definition(&self.theme.definitions) {
                    Ok(definition) => definition,
                    Err(detail) => {
                        self.notify(
                            self.ctx
                                .i18n
                                .format("ui.theme_error", &[("detail", detail)]),
                            Tone::Warning,
                        );
                        return Ok(Effect::None);
                    }
                };
                let id = entry.id.clone();
                let selected_theme = theme::resolve(definition, &self.theme.background);
                let result = (|| {
                    let mut preferences =
                        crate::config::Preferences::read(&self.ctx.paths.preferences, false)?;
                    preferences.theme = Some(id.clone());
                    preferences.write(&self.ctx.paths.preferences)
                })();
                if let Err(error) = result {
                    self.theme.active = panel.previous.clone();
                    self.fail_ui(error);
                } else {
                    self.theme.active = selected_theme;
                    self.theme.saved_id = Some(id.clone());
                    self.style_panel = None;
                    self.notify(
                        self.ctx.i18n.format("toast.theme_saved", &[("theme", &id)]),
                        Tone::Success,
                    );
                }
            }
            _ => (),
        }
        Ok(Effect::None)
    }
    /// Toasts use the wall clock: they may be raised after a terminal handoff, when the
    /// frame clock is stale.
    fn notify(&mut self, text: String, tone: Tone) {
        self.toast = Some(Toast {
            text,
            tone,
            shown: Instant::now(),
        });
    }
    /// Advance the frame clock and the state that depends on it.
    fn tick(&mut self, now: Instant) {
        self.now = now;
        let kind = self.overlay_kind();
        if kind != self.dialog_kind {
            self.dialog_kind = kind;
            self.dialog_opened = (kind != 0).then_some(now);
        }
        if self.toast.as_ref().is_some_and(|toast| {
            now.saturating_duration_since(toast.shown) >= motion::TOAST_LIFETIME
        }) {
            self.toast = None;
        }
        if let (Some(requested), Some(updated)) = (self.refreshing, self.snapshot.local.updated)
            && updated >= requested
        {
            self.refreshing = None;
        }
        for (slot, target) in self
            .gauges
            .iter_mut()
            .zip(view::gauge_targets(&self.snapshot))
        {
            match (slot.as_mut(), target) {
                (_, None) => *slot = None,
                (None, Some(to)) => *slot = Some(motion::Tween::new(to, now)),
                (Some(tween), Some(to)) => tween.retarget(to, now),
            }
        }
    }
    /// Smooth while something moves, relaxed while only a spinner turns, idle otherwise.
    fn frame_interval(&self) -> Duration {
        let moving = self
            .gauges
            .iter()
            .flatten()
            .any(|tween| tween.moving(self.now))
            || self.toast.as_ref().is_some_and(|toast| {
                motion::toast_fading(self.now.saturating_duration_since(toast.shown))
            });
        // A pending form polls quickly so it opens as soon as its items arrive.
        if moving || self.loading.is_some() || self.dialog_fading() {
            motion::ANIMATION_FRAME
        } else if self.worker.is_some()
            || self.refreshing.is_some()
            || view::pending(&self.snapshot)
        {
            motion::SPINNER_FRAME
        } else {
            motion::IDLE_FRAME
        }
    }
    fn overlay_kind(&self) -> u8 {
        if self.confirmation.is_some() {
            1
        } else if self.loading.is_some() {
            2
        } else if self.form.as_ref().is_some_and(|form| form.picker.is_some()) {
            3
        } else if self.form.is_some() {
            4
        } else if self.notice.is_some() {
            5
        } else if self.help {
            6
        } else if self.command_palette.is_some() {
            7
        } else if self.overview {
            8
        } else if self.log_modal {
            9
        } else {
            0
        }
    }
    fn dialog_fading(&self) -> bool {
        matches!(self.theme.active.palette.dialog, theme::Color::Rgb(..))
            && self.dialog_opened.is_some_and(|opened| {
                self.now.saturating_duration_since(opened) < motion::DIALOG_FADE
            })
    }
    /// Category_List order: the pinned Optimization_Category, then `category_order`.
    fn display_order(&self) -> Vec<usize> {
        std::iter::once(OPTIMIZATION_GROUP)
            .chain(self.category_order.iter().copied())
            .collect()
    }
    /// Row of Current_Category in `display_order()`.
    fn category_position(&self) -> usize {
        self.display_order()
            .iter()
            .position(|index| *index == self.group)
            .unwrap_or(0)
    }
    /// The rows Category_List draws: `display_order()` with the rule (`None`) under the
    /// pinned Optimization_Category.
    fn category_rows(&self) -> Vec<Option<usize>> {
        let order = self.display_order();
        let (pinned, reorderable) = order.split_at(1);
        pinned
            .iter()
            .copied()
            .map(Some)
            .chain([None])
            .chain(reorderable.iter().copied().map(Some))
            .collect()
    }
    /// Row of Current_Category in `category_rows()`.
    fn category_row(&self) -> usize {
        let position = self.category_position();
        position + usize::from(position > 0)
    }
    fn set_category_order(&mut self, order: Vec<usize>) {
        self.group = order[0];
        self.category_order = order;
    }
    fn optimization(&self) -> bool {
        self.group == OPTIMIZATION_GROUP
    }
    /// Position of the selection among Current_Category's Selectable_Rows.
    fn selected(&self) -> usize {
        self.positions[self.group]
    }
    /// The Action_List of `group`: grouped categories follow the Section table, each Section
    /// title before its actions; the others list their actions in `actions::specs()` order.
    /// Both come from `sections::blocks`, which the Command_Palette's search reads too.
    /// The Optimization_View lists OPTIMIZATION, then a Rule and the Execute_Row.
    fn rows(&self, group: usize) -> Vec<Row> {
        let specs = actions::specs();
        let action = |id: &str| {
            Row::Action(
                specs
                    .iter()
                    .find(|spec| spec.id == id)
                    .cloned()
                    .expect("static action ID"),
            )
        };
        if group == OPTIMIZATION_GROUP {
            return actions::OPTIMIZATION
                .iter()
                .map(|id| action(id))
                .chain([Row::Rule, Row::Execute])
                .collect();
        }
        sections::blocks(GROUPS[group], &specs)
            .into_iter()
            .flat_map(|(section, ids)| {
                section
                    .map(Row::Section)
                    .into_iter()
                    .chain(ids.into_iter().map(&action))
            })
            .collect()
    }
    fn selectable_count(&self, group: usize) -> usize {
        self.rows(group)
            .iter()
            .filter(|row| row.selectable())
            .count()
    }
    /// Current_Category's selected row: always an action or the Execute_Row.
    fn selected_row(&self) -> Row {
        self.rows(self.group)
            .into_iter()
            .filter(Row::selectable)
            .nth(self.selected())
            .expect("positions name a Selectable_Row")
    }
    fn available(&self, spec: &Spec) -> Result<()> {
        if matches!(spec.kind, Kind::Info | Kind::PublicIp) {
            return Ok(());
        }
        if let Some(error) = &self.platform_error {
            return Err(error.clone());
        }
        if spec.root && !self.root {
            return Err(Error::new("error.root", "sudo cqs"));
        }
        Ok(())
    }
    fn log(&mut self, text: impl AsRef<str>) {
        self.log_tone(text, Tone::Plain);
    }
    fn log_tone(&mut self, text: impl AsRef<str>, tone: Tone) {
        self.log_styled(text, tone, false);
    }
    fn log_styled(&mut self, text: impl AsRef<str>, tone: Tone, bold: bool) {
        for line in text.as_ref().lines() {
            self.logs.push_back(LogLine {
                text: crate::runtime::sanitize(line),
                tone,
                bold,
            });
        }
        while self.logs.len() > 2000 {
            self.logs.pop_front();
        }
    }
    fn fail_ui(&mut self, error: Error) {
        let text = self.ctx.i18n.error(&error);
        if let Some(form) = &mut self.form {
            form.error = Some(text);
        } else {
            self.notice = Some(text);
        }
    }
    fn finish(&mut self, result: Result<()>) {
        self.confirmation = None;
        let outcome = outcome(&result);
        if let Some(progress) = &mut self.progress {
            progress.finish(outcome, Instant::now());
        }
        self.phase = match outcome {
            Outcome::Done => Phase::Completed,
            Outcome::Failed => Phase::Failed,
            Outcome::Cancelled => Phase::Cancelled,
        };
        // The wall clock, not the frame clock: this runs before `tick` and after a terminal
        // handoff, when the frame clock is stale.
        if let Some((id, started)) = self.single_task.take() {
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(started);
            self.records.finished(id, outcome, elapsed, now);
        }
        let tone = self.phase.tone();
        let message = match result {
            Err(error) => self.ctx.i18n.error(&error),
            Ok(()) => self.ctx.i18n.text("ui.done").to_owned(),
        };
        self.log_tone(message, tone);
        // A visible log panel already shows the outcome; a toast would cover its last line.
        if view::log_body(self, self.viewport).height == 0 {
            let summary = format!(
                "{} · {}",
                self.ctx.i18n.text(self.phase.key()),
                self.task_title()
            );
            self.notify(summary, tone);
        }
        self.completed_since_refresh = true;
        self.ctx.cancel.store(false, Ordering::SeqCst);
    }
    fn receive(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(receiver) = &self.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            match event {
                Event::Log(text) => self.log(text),
                Event::Confirm { text, token, reply } => {
                    self.dialog_opened = Some(Instant::now());
                    self.confirmation = Some(Confirmation {
                        text,
                        token,
                        input: InputBuffer::default(),
                        reply,
                        scroll: 0,
                        accept: false,
                    })
                }
                Event::Step(step) => self.record_step(&step),
                Event::Finished(result) => {
                    let joined = self
                        .worker
                        .take()
                        .is_none_or(|worker| worker.join().is_ok());
                    self.finish(if joined {
                        result
                    } else {
                        Err(Error::new("error.io", "worker panicked"))
                    });
                    self.receiver = None;
                }
            }
        }
        if self.worker.is_some() && disconnected {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            self.finish(Err(Error::new(
                "error.io",
                "worker exited without a result",
            )));
            self.receiver = None;
        }
        self.receive_choices();
    }
    fn start(&mut self, terminal: &mut DefaultTerminal, task: Task) -> Result<()> {
        if self.worker.is_some() {
            return Err(Error::new("error.busy", ""));
        }
        let jobs = task.jobs();
        for (spec, p) in &jobs {
            self.available(spec)?;
            spec.parameters(p)?;
        }
        self.ctx.cancel.store(false, Ordering::SeqCst);
        self.logs.clear();
        self.scroll = 0;
        self.logs_open = true;
        self.phase = Phase::Running;
        self.record_start(&task);
        if jobs.iter().any(|(s, _)| s.external) {
            leave()?;
            println!("{}", self.ctx.i18n.text("ui.external"));
            let report = perform(&self.ctx, &task);
            self.record_report(&report);
            if let Err(e) = &report.result {
                eprintln!("{}", self.ctx.i18n.error(e));
            }
            println!("{}", self.ctx.i18n.text("ui.return"));
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            enter()?;
            terminal.clear()?;
            self.finish(report.result);
        } else {
            let (tx, rx) = mpsc::channel();
            let mut ctx = self.ctx.clone();
            ctx.events = Some(tx.clone());
            self.receiver = Some(rx);
            self.worker = Some(thread::spawn(move || {
                let report = perform(&ctx, &task);
                let _ = tx.send(Event::Finished(report.result));
            }));
        }
        Ok(())
    }
    fn record_start(&mut self, task: &Task) {
        self.task_ids = task
            .jobs()
            .iter()
            .map(|(spec, _)| spec.id.to_owned())
            .collect();
        self.progress = match task {
            Task::Batch(jobs) => Some(view::plan::Progress::new(jobs.len(), Instant::now())),
            Task::Single { .. } => None,
        };
        self.single_task = match task {
            Task::Single { spec, .. } => {
                self.records.started(spec.id);
                Some((spec.id, Instant::now()))
            }
            Task::Batch(_) => None,
        };
    }
    fn record_step(&mut self, step: &Step) {
        let now = Instant::now();
        if let Some(line) = self.ctx.step_line(step) {
            let started = matches!(step.state, StepState::Started);
            self.log_styled(
                line,
                if started { Tone::Info } else { Tone::Success },
                started,
            );
        }
        match step.state {
            StepState::Started => self.records.started(step.id),
            StepState::Finished { outcome, elapsed } => {
                self.records.finished(step.id, outcome, elapsed, now)
            }
        }
        if let Some(progress) = &mut self.progress {
            progress.apply(step, now);
        }
    }
    fn record_report(&mut self, report: &actions::BatchReport) {
        for (index, step) in report.steps.iter().enumerate() {
            if let (Some(outcome), Some(elapsed)) = (step.outcome, step.elapsed) {
                for state in [StepState::Started, StepState::Finished { outcome, elapsed }] {
                    self.record_step(&Step {
                        index,
                        total: report.steps.len(),
                        id: step.id,
                        state,
                    });
                }
            }
        }
    }
    fn make_form(&self, spec: Spec, mode: FormMode) -> Result<Form> {
        let mut values = spec.defaults();
        if spec.id == "terminal.apply" && !self.parameters.contains_key(spec.id) {
            let options = crate::ops::terminal::options(&self.ctx)?;
            values = BTreeMap::from([
                ("shell".into(), options.shell),
                ("style".into(), options.style),
                ("color".into(), options.color),
                ("time".into(), options.time),
                ("git".into(), options.git),
            ]);
        }
        if let Some(saved) = self.parameters.get(spec.id) {
            values.extend(saved.clone());
        }
        Ok(Form {
            values: spec
                .fields
                .iter()
                .map(|f| InputBuffer::new(values[f.name].clone()))
                .collect(),
            index: 0,
            mode,
            error: None,
            options: vec![Vec::new(); spec.fields.len()],
            picker: None,
            spec,
        })
    }
    /// Open a parameter form. Fields that name existing items read them first, off the UI
    /// thread: a controller query can wait on the network.
    fn open_form(&mut self, spec: Spec, mode: FormMode) -> Result<()> {
        let listings = spec
            .fields
            .iter()
            .map(|field| match field.source {
                Some(Source::Listing(listing)) => Some(listing),
                _ => None,
            })
            .collect::<Vec<_>>();
        if listings.iter().all(Option::is_none) {
            self.form = Some(self.make_form(spec, mode)?);
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let mut ctx = self.ctx.clone();
        ctx.cancel = cancel.clone();
        let (tx, receiver) = mpsc::channel();
        thread::spawn(move || {
            let options = listings
                .into_iter()
                .map(|listing| match listing {
                    Some(listing) => actions::choices(&ctx, listing),
                    None => Ok(Vec::new()),
                })
                .collect();
            // The form may have been abandoned; then nobody is waiting for the items.
            let _ = tx.send(options);
        });
        self.loading = Some(Loading {
            spec,
            mode,
            receiver,
            cancel,
            started: Instant::now(),
        });
        Ok(())
    }
    fn receive_choices(&mut self) {
        let Some(loading) = &self.loading else {
            return;
        };
        let result = match loading.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::new(
                "error.io",
                "choice loader exited without a result",
            )),
        };
        let loading = self.loading.take().expect("pending form");
        if let Err(error) = result.and_then(|options| self.show_form(loading, options)) {
            self.fail_ui(error);
        }
    }
    /// With nothing to choose from, the form would be a dead end, so say why instead.
    fn show_form(&mut self, loading: Loading, options: Vec<Vec<Choice>>) -> Result<()> {
        for (field, choices) in loading.spec.fields.iter().zip(&options) {
            if let Some(Source::Listing(listing)) = field.source
                && choices.is_empty()
            {
                let message = self.ctx.i18n.text(empty_listing(listing)).to_owned();
                self.notify(message, Tone::Warning);
                return Ok(());
            }
        }
        let mut form = self.make_form(loading.spec, loading.mode)?;
        form.attach(options);
        // A form that starts by naming an item opens straight into its list.
        if form.listed(0) {
            form.open_picker(String::new(), true);
        }
        self.form = Some(form);
        Ok(())
    }
    fn cancel_loading(&mut self) {
        if let Some(loading) = self.loading.take() {
            loading.cancel.store(true, Ordering::SeqCst);
        }
    }
    fn activate(&mut self) -> Result<Effect> {
        let spec = match self.selected_row() {
            Row::Action(spec) => spec,
            // Enter on the Execute_Row runs the Checklist, as F5 does.
            _ => return self.optimization_jobs(),
        };
        if !self.optimization() {
            return self.activate_action(spec);
        }
        self.available(&spec)?;
        if spec.fields.is_empty() {
            self.toggle_selected()?;
        } else {
            self.open_form(spec, FormMode::Optimization)?;
        }
        Ok(Effect::None)
    }
    /// Enter on an Action of a Reorderable_Category (Requirement 1.7), from the Action_List
    /// or the Command_Palette (Requirement 8.8): a Form_Action opens its Run-mode form,
    /// system.info the overview, system.public_ip the public lookup; any other Action runs.
    fn activate_action(&mut self, spec: Spec) -> Result<Effect> {
        self.available(&spec)?;
        match spec.kind {
            Kind::Info => {
                self.overview = true;
                self.overview_scroll = 0;
                Ok(Effect::Refresh)
            }
            Kind::PublicIp => Ok(Effect::PublicIp),
            _ if spec.fields.is_empty() => Ok(Effect::Run(Task::Single {
                spec,
                params: Parameters::new(),
                reviewed: false,
            })),
            _ => {
                self.open_form(spec, FormMode::Run)?;
                Ok(Effect::None)
            }
        }
    }
    fn toggle_selected(&mut self) -> Result<()> {
        if !self.optimization() {
            return Ok(());
        }
        let Row::Action(spec) = self.selected_row() else {
            return Ok(());
        };
        self.available(&spec)?;
        if !self.marked.remove(spec.id) {
            self.marked.insert(spec.id.into());
        }
        self.progress = None;
        Ok(())
    }
    fn cycle_language(&mut self) -> Result<()> {
        let locales = self
            .ctx
            .i18n
            .languages()
            .into_iter()
            .map(|(id, _)| id.to_owned())
            .collect::<Vec<_>>();
        let next = (locales
            .iter()
            .position(|id| id == self.ctx.i18n.locale())
            .unwrap_or(0)
            + 1)
            % locales.len();
        let locale = &locales[next];
        let mut preferences = crate::config::Preferences::read(&self.ctx.paths.preferences, false)?;
        preferences.language = Some(locale.clone());
        preferences.write(&self.ctx.paths.preferences)?;
        self.ctx.i18n.select(locale)?;
        let name = self
            .ctx
            .i18n
            .languages()
            .into_iter()
            .find(|(id, _)| id == locale)
            .map(|(_, name)| name.to_owned())
            .expect("selected locale is listed");
        let message = self
            .ctx
            .i18n
            .format("toast.language", &[("language", &name)]);
        self.notify(message, Tone::Success);
        Ok(())
    }
    fn optimization_jobs(&mut self) -> Result<Effect> {
        if !self.optimization() {
            return Ok(Effect::None);
        }
        if self.marked.is_empty() {
            let message = self.ctx.i18n.text("ui.no_selection").to_owned();
            self.notify(message, Tone::Warning);
            return Ok(Effect::None);
        }
        let mut jobs = Vec::new();
        for (spec, values) in self.checklist() {
            self.available(&spec)?;
            match spec.parameters(&values) {
                Ok(values) => jobs.push((spec, values)),
                Err(error) => {
                    let mut form = self.make_form(spec, FormMode::Optimization)?;
                    form.error = Some(self.ctx.i18n.error(&error));
                    self.form = Some(form);
                    return Ok(Effect::None);
                }
            }
        }
        Ok(Effect::Run(Task::Batch(jobs)))
    }
    fn checklist(&self) -> Vec<(Spec, Parameters)> {
        self.checklist_from_rows(&self.rows(OPTIMIZATION_GROUP))
    }
    fn checklist_from_rows(&self, rows: &[Row]) -> Vec<(Spec, Parameters)> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Action(spec) if self.marked.contains(spec.id) => {
                    let params = self.saved_parameters(spec);
                    Some((spec.clone(), params))
                }
                _ => None,
            })
            .collect()
    }
    fn saved_parameters(&self, spec: &Spec) -> Parameters {
        let mut params = spec.defaults();
        if let Some(saved) = self.parameters.get(spec.id) {
            params.extend(saved.clone());
        }
        params
    }
    fn recommend(&mut self) -> Result<()> {
        if !self.optimization() {
            return Ok(());
        }
        for id in RECOMMENDED {
            self.available(&actions::find(id)?)?;
        }
        let marked = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
        if self.marked != marked
            || self
                .saved_parameters(&actions::find("system.clean")?)
                .get("scope")
                .map(String::as_str)
                != Some("standard")
        {
            self.progress = None;
        }
        self.marked = marked;
        self.parameters.insert(
            "system.clean".into(),
            Parameters::from([("scope".into(), "standard".into())]),
        );
        self.notify(
            self.ctx.i18n.text("toast.recommended").to_owned(),
            Tone::Success,
        );
        Ok(())
    }
    fn submit_form(&mut self) -> Result<Effect> {
        let form = self.form.as_ref().expect("active form");
        let values = form.spec.parameters(&form.parameters())?;
        let spec = form.spec.clone();
        let mode = form.mode;
        if self.marked.contains(spec.id) && self.saved_parameters(&spec) != values
            || mode == FormMode::Optimization && !self.marked.contains(spec.id)
        {
            self.progress = None;
        }
        self.parameters.insert(spec.id.into(), values.clone());
        self.form = None;
        if mode == FormMode::Optimization {
            self.marked.insert(spec.id.into());
            let message = self.ctx.i18n.format(
                "toast.selection_saved",
                &[("action", self.ctx.i18n.text(&spec.label()))],
            );
            self.notify(message, Tone::Success);
            Ok(Effect::None)
        } else {
            Ok(Effect::Run(Task::Single {
                spec,
                params: values,
                reviewed: true,
            }))
        }
    }
    fn cycle_field(&mut self, forward: bool) {
        let form = self.form.as_mut().expect("active form");
        if form.index >= form.values.len() {
            form.index = if form.index == form.values.len() {
                form.values.len() + 1
            } else {
                form.values.len()
            };
            return;
        }
        if form.listed(form.index) {
            let choices = form.choices(form.index);
            if choices.is_empty() {
                return;
            }
            let held = &form.values[form.index].text;
            let current = choices.iter().position(|choice| choice.value == *held);
            let value = choices[step(current, choices.len(), forward)].value.clone();
            form.choose(form.index, value);
            return;
        }
        let field = &form.spec.fields[form.index];
        let choices = field.choices;
        let value = &form.values[form.index].text;
        let current = choices.iter().position(|v| *v == value);
        if choices.is_empty() || (field.allow_custom && !value.is_empty() && current.is_none()) {
            if forward {
                form.values[form.index].right();
            } else {
                form.values[form.index].left();
            }
        } else {
            let next = step(current, choices.len(), forward);
            form.values[form.index].replace(choices[next].into());
        }
    }
    /// Enter on a field: the last one submits, any other moves on, opening the next
    /// field's list when it names an existing item.
    fn advance(&mut self) -> Result<Effect> {
        let form = self.form.as_mut().expect("active form");
        if form.index + 1 == form.values.len() {
            return self.submit_form();
        }
        form.index += 1;
        if form.listed(form.index) {
            form.open_picker(String::new(), false);
        }
        Ok(Effect::None)
    }
    /// Take the highlighted item and move on as Enter would.
    fn pick(&mut self, value: String) -> Result<Effect> {
        let form = self.form.as_mut().expect("active form");
        form.picker = None;
        form.choose(form.index, value);
        self.advance()
    }
    fn close_picker(&mut self) {
        let form = self.form.as_mut().expect("active form");
        if form.picker.take().is_some_and(|picker| picker.with_form) {
            self.form = None;
        }
    }
    fn picker_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let viewport = self.viewport;
        let (highlighted, count, page) = {
            let form = self.form.as_ref().expect("active form");
            let picker = form.picker.as_ref().expect("open picker");
            let choices = form.choices(form.index);
            let matches = picker.matches(choices);
            let geometry =
                view::PickerGeometry::new(viewport, choices.len(), picker.cursor, matches.len());
            (
                matches
                    .get(picker.cursor)
                    .map(|choice| choice.value.clone()),
                matches.len(),
                geometry.sections[1].height.saturating_sub(1).max(1) as usize,
            )
        };
        match key.code {
            KeyCode::Esc => {
                self.close_picker();
                return Ok(Effect::None);
            }
            KeyCode::Enter => {
                return highlighted.map_or(Ok(Effect::None), |value| self.pick(value));
            }
            _ => (),
        }
        let picker = self
            .form
            .as_mut()
            .and_then(|form| form.picker.as_mut())
            .expect("open picker");
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Down if count > 0 => picker.cursor = (picker.cursor + 1) % count,
            KeyCode::Up if count > 0 => picker.cursor = (picker.cursor + count - 1) % count,
            KeyCode::PageDown => picker.cursor = (picker.cursor + page).min(last),
            KeyCode::PageUp => picker.cursor = picker.cursor.saturating_sub(page),
            KeyCode::Home => picker.cursor = 0,
            KeyCode::End => picker.cursor = last,
            KeyCode::Left => picker.filter.left(),
            KeyCode::Right => picker.filter.right(),
            code => {
                let before = picker.filter.text.clone();
                match code {
                    KeyCode::Backspace => picker.filter.backspace(),
                    KeyCode::Delete => picker.filter.delete(),
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        picker.filter.clear()
                    }
                    KeyCode::Char(c)
                        if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        picker.filter.insert(c)
                    }
                    _ => (),
                }
                // A new filter starts from its best match.
                if picker.filter.text != before {
                    picker.cursor = 0;
                }
            }
        }
        Ok(Effect::None)
    }
    fn form_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let form = self.form.as_mut().expect("active form");
        if form.picker.is_some() {
            return self.picker_key(key);
        }
        let n = form.values.len();
        if key.code == KeyCode::Esc {
            self.form = None;
            return Ok(Effect::None);
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.submit_form();
        }
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.index = (form.index + 1) % (n + 2),
            KeyCode::BackTab | KeyCode::Up => form.index = (form.index + n + 1) % (n + 2),
            KeyCode::Enter if form.index == n => return self.submit_form(),
            KeyCode::Enter if form.index == n + 1 => self.form = None,
            KeyCode::Enter if form.listed(form.index) => form.open_picker(String::new(), false),
            KeyCode::Enter => return self.advance(),
            KeyCode::Left => self.cycle_field(false),
            KeyCode::Right => self.cycle_field(true),
            KeyCode::Char(' ') if form.index >= n => {
                if form.index == n {
                    return self.submit_form();
                }
                self.form = None;
            }
            _ if form.index >= n => (),
            // Typing on a listed field searches its list; Space just opens it.
            KeyCode::Char(c)
                if form.listed(form.index)
                    && !c.is_control()
                    && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let filter = if c == ' ' { String::new() } else { c.into() };
                form.open_picker(filter, false);
            }
            // A listed value is never edited by hand: it must name an existing item.
            _ if form.listed(form.index) => (),
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && form.spec.fields[form.index].editable() =>
            {
                form.values[form.index].clear()
            }
            KeyCode::Home => form.values[form.index].cursor = 0,
            KeyCode::End => form.values[form.index].cursor = form.values[form.index].text.len(),
            KeyCode::Backspace if form.spec.fields[form.index].editable() => {
                form.values[form.index].backspace()
            }
            KeyCode::Delete if form.spec.fields[form.index].editable() => {
                form.values[form.index].delete()
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && form.spec.fields[form.index].editable() =>
            {
                form.values[form.index].insert(c)
            }
            _ => (),
        }
        Ok(Effect::None)
    }
    fn confirmation_key(&mut self, key: KeyEvent) -> Effect {
        let (max_scroll, page) = {
            let confirm = self.confirmation.as_ref().expect("confirmation");
            let geometry = view::ConfirmationGeometry::new(self.viewport, confirm, &self.ctx.i18n);
            (
                geometry.max_scroll(),
                geometry.sections[0].height.saturating_sub(1).max(1),
            )
        };
        let confirm = self.confirmation.as_mut().expect("confirmation");
        let answer = if key.code == KeyCode::Esc {
            Some(false)
        } else if confirm.token == "YES" {
            match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    confirm.accept = !confirm.accept;
                    None
                }
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') => Some(false),
                KeyCode::Enter => Some(confirm.accept),
                _ => None,
            }
        } else {
            match key.code {
                KeyCode::Enter if confirm.input.text == confirm.token => Some(true),
                KeyCode::Backspace => {
                    confirm.input.backspace();
                    None
                }
                KeyCode::Left => {
                    confirm.input.left();
                    None
                }
                KeyCode::Right => {
                    confirm.input.right();
                    None
                }
                KeyCode::Home => {
                    confirm.input.cursor = 0;
                    None
                }
                KeyCode::End => {
                    confirm.input.cursor = confirm.input.text.len();
                    None
                }
                KeyCode::Delete => {
                    confirm.input.delete();
                    None
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    confirm.input.clear();
                    None
                }
                KeyCode::Char(c)
                    if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    confirm.input.insert(c);
                    None
                }
                _ => None,
            }
        };
        match key.code {
            KeyCode::PageUp => confirm.scroll = confirm.scroll.saturating_sub(page),
            KeyCode::PageDown => {
                confirm.scroll = confirm.scroll.saturating_add(page).min(max_scroll)
            }
            _ => (),
        }
        if let Some(answer) = answer {
            let _ = confirm.reply.send(answer);
            self.confirmation = None;
        }
        Effect::None
    }
    fn request_cancel(&mut self, exit: bool) {
        self.exit_requested |= exit;
        self.ctx.cancel.store(true, Ordering::SeqCst);
        self.phase = Phase::Cancelling;
        if let Some(confirm) = self.confirmation.take() {
            let _ = confirm.reply.send(false);
        }
    }
    /// Alt+↑/↓: swap Current_Category with its neighbour in `category_order`. The new order
    /// takes effect only once config.toml holds it; a failed save is shown and the old
    /// order stays. Optimization_Category is not in `category_order`, so it never moves and
    /// nothing moves above it; at either end nothing is written.
    fn reorder_category(&mut self, forward: bool) {
        let Some(position) = self
            .category_order
            .iter()
            .position(|index| *index == self.group)
        else {
            return;
        };
        let neighbour = if forward {
            Some(position + 1)
        } else {
            position.checked_sub(1)
        };
        let Some(neighbour) = neighbour.filter(|index| *index < self.category_order.len()) else {
            return;
        };
        let mut order = self.category_order.clone();
        order.swap(position, neighbour);
        match self.save_category_order(&order) {
            Ok(()) => {
                self.category_order = order;
                let message = self.ctx.i18n.text("toast.order_saved").to_owned();
                self.notify(message, Tone::Success);
            }
            Err(error) => self.fail_ui(error),
        }
    }
    /// Store `order`, Reorderable_Category IDs only, keeping every other preference.
    fn save_category_order(&self, order: &[usize]) -> Result<()> {
        let mut preferences = crate::config::Preferences::read(&self.ctx.paths.preferences, false)?;
        preferences.category_order = order
            .iter()
            .map(|index| GROUPS[*index].to_owned())
            .collect();
        preferences.write(&self.ctx.paths.preferences)
    }
    /// The Command_Palette's rows for its query, grouped in Category_List order; empty
    /// while it is closed.
    fn palette_results(&self) -> palette::Results {
        let query = self
            .command_palette
            .as_ref()
            .map_or("", |palette| palette.query.text.as_str());
        palette::Results::new(
            query,
            self.records.recent.iter().copied(),
            &actions::specs(),
            &self.ctx.i18n,
            &self.category_order,
        )
    }
    /// The Action of the highlighted result, if the Command_Palette lists any.
    fn palette_highlighted(&self) -> Option<Spec> {
        let cursor = self.command_palette.as_ref()?.cursor;
        self.palette_results()
            .hit(cursor)
            .map(|hit| hit.spec.clone())
    }
    /// Enter, or a click on "执行 ↵": while a task runs the Command_Palette stays open and
    /// shows `error.busy` (Requirement 8.9), and an Action that cannot run shows why without
    /// running (8.13). Otherwise it closes and the Action is activated exactly as Enter on
    /// the Action_List activates it (8.8), while Current_Category and its selection stay as
    /// they were: only Tab moves them.
    fn palette_enter(&mut self) -> Result<Effect> {
        let Some(spec) = self.palette_highlighted() else {
            return Ok(Effect::None);
        };
        if self.worker.is_some() {
            self.fail_ui(Error::new("error.busy", ""));
            return Ok(Effect::None);
        }
        if let Err(error) = self.available(&spec) {
            self.fail_ui(error);
            return Ok(Effect::None);
        }
        self.command_palette = None;
        self.activate_action(spec)
    }
    /// Tab (Requirement 8.10): close, then select the highlighted Action in its own category.
    fn palette_jump(&mut self) {
        let Some(spec) = self.palette_highlighted() else {
            return;
        };
        self.command_palette = None;
        self.group = GROUPS
            .iter()
            .position(|group| *group == spec.group)
            .expect("an Action belongs to a category");
        self.positions[self.group] = self
            .rows(self.group)
            .iter()
            .filter(|row| row.selectable())
            .position(|row| matches!(row, Row::Action(listed) if listed.id == spec.id))
            .expect("an Action is listed in its category");
    }
    /// Command_Palette keys (design component 2): printable characters, digits and letter
    /// shortcuts included, edit the query with ←→, Home/End, Backspace, Delete and Ctrl+U;
    /// ↑↓ and PgUp/PgDn move the highlight, which a new query puts back on its first result.
    fn palette_key(&mut self, key: KeyEvent) -> Result<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.command_palette = None;
                return Ok(Effect::None);
            }
            KeyCode::Enter => return self.palette_enter(),
            KeyCode::Tab => {
                self.palette_jump();
                return Ok(Effect::None);
            }
            _ => (),
        }
        let results = self.palette_results();
        let count = results.count();
        let page = view::CommandPaletteGeometry::new(self.viewport, self, &results)
            .rows
            .height
            .saturating_sub(1)
            .max(1) as usize;
        let palette = self.command_palette.as_mut().expect("open Command_Palette");
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Down if count > 0 => palette.cursor = (palette.cursor + 1) % count,
            KeyCode::Up if count > 0 => palette.cursor = (palette.cursor + count - 1) % count,
            KeyCode::PageDown => palette.cursor = (palette.cursor + page).min(last),
            KeyCode::PageUp => palette.cursor = palette.cursor.saturating_sub(page),
            KeyCode::Left => palette.query.left(),
            KeyCode::Right => palette.query.right(),
            KeyCode::Home => palette.query.cursor = 0,
            KeyCode::End => palette.query.cursor = palette.query.text.len(),
            code => {
                let before = palette.query.text.clone();
                match code {
                    KeyCode::Backspace => palette.query.backspace(),
                    KeyCode::Delete => palette.query.delete(),
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        palette.query.clear()
                    }
                    KeyCode::Char(c)
                        if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        palette.query.insert(c)
                    }
                    _ => (),
                }
                if palette.query.text != before {
                    palette.cursor = 0;
                }
            }
        }
        Ok(Effect::None)
    }
    fn key(&mut self, key: KeyEvent) -> Result<Effect> {
        // Keyboard navigation takes over from the pointer.
        self.hover = None;
        // Exit/cancel must remain reachable through logs and other modal views.
        let interrupt = matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL);
        if interrupt || (self.worker.is_some() && matches!(key.code, KeyCode::Char('q' | 'Q'))) {
            if self.worker.is_some() {
                self.request_cancel(true);
                return Ok(Effect::None);
            }
            return Ok(Effect::Quit);
        }
        if self.confirmation.is_some() {
            return Ok(self.confirmation_key(key));
        }
        if self.loading.is_some() {
            if key.code == KeyCode::Esc {
                self.cancel_loading();
            }
            return Ok(Effect::None);
        }
        if self.form.is_some() {
            return self.form_key(key);
        }
        if self.notice.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.notice = None;
            }
            return Ok(Effect::None);
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?') | KeyCode::F(1)
            ) {
                self.help = false;
            }
            return Ok(Effect::None);
        }
        if self.command_palette.is_some() {
            return self.palette_key(key);
        }
        if self.overview {
            let geometry = view::OverviewGeometry::new(self.viewport, self);
            let (page, max) = (geometry.page(), geometry.max_scroll());
            let scroll = self.overview_scroll.min(max);
            self.overview_scroll = match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.overview = false;
                    scroll
                }
                KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1).min(max),
                KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                KeyCode::PageDown => scroll.saturating_add(page).min(max),
                KeyCode::PageUp => scroll.saturating_sub(page),
                KeyCode::Home => 0,
                KeyCode::End => max,
                KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
                KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
                _ => scroll,
            };
            return Ok(Effect::None);
        }
        if self.log_modal {
            match key.code {
                KeyCode::Esc | KeyCode::Char('g' | 'G' | 'q') | KeyCode::F(2) => {
                    self.log_modal = false
                }
                code => self.log_key(code),
            }
            return Ok(Effect::None);
        }
        if self.style_panel.is_some() {
            return self.style_key(key);
        }
        self.main_key(key)
    }
    /// Main_View keys. There is no focus: ↑/↓ always move the Action_List, while ←/→, Tab,
    /// the digits and O pick the category.
    fn main_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let idle = self.worker.is_none();
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down)
        {
            // Reordering waits for the task; the plain arrow never acts in its place.
            if idle {
                self.reorder_category(key.code == KeyCode::Down);
            }
            return Ok(Effect::None);
        }
        match key.code {
            // Esc never navigates: it only cancels a running task.
            KeyCode::Esc => {
                if !idle {
                    self.request_cancel(false);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_row(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(true),
            KeyCode::Home => self.positions[self.group] = 0,
            KeyCode::End => self.positions[self.group] = self.selectable_count(self.group) - 1,
            KeyCode::Left | KeyCode::BackTab => self.cycle_category(false),
            KeyCode::Right | KeyCode::Tab => self.cycle_category(true),
            // n is the n-th Reorderable_Category, as numbered in Category_List.
            KeyCode::Char(digit @ '1'..='9') => {
                let n = digit as usize - '0' as usize;
                if let Some(group) = self.category_order.get(n - 1) {
                    self.group = *group;
                }
            }
            KeyCode::Char('o' | 'O' | '0') => self.group = OPTIMIZATION_GROUP,
            // Opens with an empty query and its first result highlighted (Requirement 8.1).
            KeyCode::Char('/') => self.command_palette = Some(palette::CommandPalette::default()),
            KeyCode::Char('t' | 'T') => self.open_style(),
            KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
            KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
            KeyCode::Char('g' | 'G') => self.logs_open = !self.logs_open,
            KeyCode::F(2) => self.log_modal = !self.log_modal,
            KeyCode::Char('l' | 'L') => self.cycle_language()?,
            KeyCode::Char('q' | 'Q') if idle => return Ok(Effect::Quit),
            KeyCode::Enter if idle => return self.activate(),
            KeyCode::Char(' ') if idle => self.toggle_selected()?,
            KeyCode::Char('x' | 'X') | KeyCode::F(5) if idle => return self.optimization_jobs(),
            KeyCode::Char('a' | 'A') if idle => self.recommend()?,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            code => self.log_key(code),
        }
        Ok(Effect::None)
    }
    /// The previous or next Selectable_Row of Current_Category, wrapping at both ends.
    fn move_row(&mut self, forward: bool) {
        let count = self.selectable_count(self.group);
        self.positions[self.group] = step(Some(self.selected()), count, forward);
    }
    /// The previous or next category in `display_order()`, wrapping at both ends. Each
    /// category keeps its own entry in `positions`.
    fn cycle_category(&mut self, forward: bool) {
        let order = self.display_order();
        self.group = order[step(Some(self.category_position()), order.len(), forward)];
    }
    /// Pages move by the visible height; the full-screen view also takes line and end keys.
    fn log_key(&mut self, code: KeyCode) {
        let page = i32::from(
            view::log_body(self, self.viewport)
                .height
                .saturating_sub(1)
                .max(1),
        );
        let delta = match code {
            KeyCode::PageUp => page,
            KeyCode::PageDown => -page,
            KeyCode::Up | KeyCode::Char('k') if self.log_modal => 1,
            KeyCode::Down | KeyCode::Char('j') if self.log_modal => -1,
            KeyCode::Home if self.log_modal => i32::MAX,
            KeyCode::End if self.log_modal => i32::MIN,
            _ => return,
        };
        self.scroll_logs(delta);
    }
    /// Positive deltas move toward older output. The offset is clamped so the oldest row
    /// stops at the top of the panel instead of scrolling into blank space.
    fn scroll_logs(&mut self, delta: i32) {
        let body = view::log_body(self, self.viewport);
        if body.height == 0 {
            return;
        }
        let rows = view::log_row_count(self, body.width as usize);
        let max = rows
            .saturating_sub(body.height as usize)
            .min(u16::MAX as usize) as i64;
        self.scroll = (i64::from(self.scroll) + i64::from(delta)).clamp(0, max) as u16;
    }
    fn mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        self.hover = Some(point);
        // Motion only moves the hover; the "too small" notice has no hit areas.
        if matches!(event.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
            || !view::fits(area)
        {
            return Ok(Effect::None);
        }
        if let Some(confirm) = &mut self.confirmation {
            let geometry = view::ConfirmationGeometry::new(area, confirm, &self.ctx.i18n);
            match event.kind {
                MouseEventKind::ScrollUp => confirm.scroll = confirm.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => {
                    confirm.scroll = confirm.scroll.saturating_add(3).min(geometry.max_scroll())
                }
                MouseEventKind::Down(MouseButton::Left) if confirm.token == "YES" => {
                    let answer = if geometry.buttons[0].contains(point) {
                        Some(false)
                    } else if geometry.buttons[1].contains(point) {
                        Some(true)
                    } else {
                        None
                    };
                    if let Some(answer) = answer {
                        let _ = confirm.reply.send(answer);
                        self.confirmation = None;
                    }
                }
                _ => (),
            }
            return Ok(Effect::None);
        }
        // Nothing is clickable until the pending form opens.
        if self.loading.is_some() {
            return Ok(Effect::None);
        }
        if let Some(form) = &self.form {
            if form.picker.is_some() {
                return self.picker_mouse(event, area);
            }
            let geometry = view::FormGeometry::new(area, form, self);
            let form = self.form.as_mut().expect("active form");
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if geometry.buttons[0].contains(point) {
                    return self.submit_form();
                }
                if geometry.buttons[1].contains(point) {
                    self.form = None;
                    return Ok(Effect::None);
                }
                if let Some((index, choice)) = geometry.chip_at(point) {
                    form.index = index;
                    form.values[index].replace(form.spec.fields[index].choices[choice].into());
                } else if let Some(index) = geometry.field_at(point) {
                    form.index = index;
                    if form.listed(index) {
                        form.open_picker(String::new(), false);
                    } else if !form.chip_field(index) && !form.spec.fields[index].choices.is_empty()
                    {
                        self.cycle_field(true);
                    }
                }
            }
            return Ok(Effect::None);
        }
        // Modal input owns the screen; background clicks never trigger operations.
        if self.notice.is_some() {
            return Ok(Effect::None);
        }
        if self.help {
            return Ok(Effect::None);
        }
        if self.command_palette.is_some() {
            return self.palette_mouse(event, area);
        }
        if self.overview {
            let geometry = view::OverviewGeometry::new(area, self);
            if !geometry.body.contains(point) {
                return Ok(Effect::None);
            }
            let max = geometry.max_scroll();
            let scroll = self.overview_scroll.min(max);
            self.overview_scroll = match event.kind {
                MouseEventKind::ScrollUp => scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => scroll.saturating_add(3).min(max),
                _ => scroll,
            };
            return Ok(Effect::None);
        }
        if self.log_modal {
            if !view::log_body(self, area).contains(point) {
                return Ok(Effect::None);
            }
            match event.kind {
                MouseEventKind::ScrollUp => self.scroll_logs(3),
                MouseEventKind::ScrollDown => self.scroll_logs(-3),
                _ => (),
            }
            return Ok(Effect::None);
        }
        if let Some(panel) = &self.style_panel {
            let geometry = view::StyleGeometry::new(area, self);
            let cursor = match event.kind {
                MouseEventKind::Down(MouseButton::Left) => geometry.row_at(point),
                MouseEventKind::ScrollUp if geometry.panel.contains(point) => {
                    Some(panel.cursor.saturating_sub(1))
                }
                MouseEventKind::ScrollDown if geometry.panel.contains(point) => {
                    Some((panel.cursor + 1).min(panel.entries.len() - 1))
                }
                _ => None,
            };
            if let Some(cursor) = cursor {
                self.preview_style(cursor);
            }
            return Ok(Effect::None);
        }
        // The same geometry `view::draw` lays out for this area, including each list's scroll
        // offset, so a click lands on the row drawn under it.
        let layout = view::Regions::new(area, self);
        let down = event.kind == MouseEventKind::ScrollDown;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = layout.category_at(point) {
                    // A click on the rule changes nothing.
                    if let Some(group) = self.category_rows()[index] {
                        self.group = group;
                    }
                } else if let Some(index) = layout.action_at(point) {
                    let rows = self.rows(self.group);
                    // A click on a Section title or the Rule changes nothing.
                    if let Some(position) = row_position(&rows, index) {
                        self.positions[self.group] = position;
                        if matches!(rows[index], Row::Execute) && self.worker.is_none() {
                            return self.optimization_jobs();
                        }
                        // The checkbox sits after the two-cell selection marker.
                        if matches!(rows[index], Row::Action(_))
                            && self.optimization()
                            && self.worker.is_none()
                            && layout.checkboxes.contains(point)
                        {
                            self.toggle_selected()?;
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if layout.logs.contains(point) {
                    self.scroll_logs(if down { -3 } else { 3 })
                } else if layout.categories.contains(point) {
                    self.cycle_category(down)
                } else if layout.actions.contains(point) {
                    self.move_row(down)
                }
            }
            _ => (),
        }
        Ok(Effect::None)
    }
    /// A click picks the row under it; the wheel moves the highlight. Clicks elsewhere are
    /// ignored, as in other dialogs.
    fn picker_mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        let (clicked, count, over) = {
            let form = self.form.as_ref().expect("active form");
            let picker = form.picker.as_ref().expect("open picker");
            let choices = form.choices(form.index);
            let matches = picker.matches(choices);
            let geometry =
                view::PickerGeometry::new(area, choices.len(), picker.cursor, matches.len());
            let clicked = geometry
                .row_at(point, matches.len())
                .map(|row| matches[row].value.clone());
            (clicked, matches.len(), geometry.popup.contains(point))
        };
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            return clicked.map_or(Ok(Effect::None), |value| self.pick(value));
        }
        let picker = self
            .form
            .as_mut()
            .and_then(|form| form.picker.as_mut())
            .expect("open picker");
        match event.kind {
            MouseEventKind::ScrollDown if over => {
                picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1))
            }
            MouseEventKind::ScrollUp if over => picker.cursor = picker.cursor.saturating_sub(1),
            _ => (),
        }
        Ok(Effect::None)
    }
    /// A click highlights the result row under it, and a click on the highlighted row's
    /// "执行 ↵" acts as Enter (Requirement 17.6); the wheel over the palette moves the
    /// highlight (17.9). Category names and everything outside the rows ignore clicks
    /// (17.11). Hits come from the geometry the palette is drawn with (17.10).
    fn palette_mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        let results = self.palette_results();
        let geometry = view::CommandPaletteGeometry::new(area, self, &results);
        if event.kind == MouseEventKind::Down(MouseButton::Left) && geometry.exec_at(point) {
            return self.palette_enter();
        }
        let clicked = geometry
            .row_at(point)
            .and_then(|index| results.position(index));
        let over = geometry.popup.contains(point);
        let last = results.count().saturating_sub(1);
        let palette = self.command_palette.as_mut().expect("open Command_Palette");
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(position) = clicked {
                    palette.cursor = position;
                }
            }
            MouseEventKind::ScrollDown if over => palette.cursor = (palette.cursor + 1).min(last),
            MouseEventKind::ScrollUp if over => palette.cursor = palette.cursor.saturating_sub(1),
            _ => (),
        }
        Ok(Effect::None)
    }
    pub fn draw(&self, frame: &mut Frame) {
        view::draw(self, frame);
    }
    fn task_title(&self) -> String {
        if self.task_ids.len() == 1 {
            self.ctx
                .i18n
                .text(&format!("action.{}", self.task_ids[0]))
                .into()
        } else if self.task_ids.is_empty() {
            self.ctx.i18n.text("ui.logs").into()
        } else {
            self.ctx.i18n.format(
                "ui.batch_title",
                &[("count", &self.task_ids.len().to_string())],
            )
        }
    }
}
/// `error.cancelled` is a cancellation, never a failure (Requirement 11.4).
fn outcome(result: &Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Done,
        Err(e) if e.key == "error.cancelled" => Outcome::Cancelled,
        Err(_) => Outcome::Failed,
    }
}
fn perform(ctx: &Context, task: &Task) -> actions::BatchReport {
    match task {
        Task::Single {
            spec,
            params,
            reviewed,
        } => actions::BatchReport {
            steps: Vec::new(),
            result: if *reviewed {
                actions::execute_reviewed(ctx, spec, params)
            } else {
                actions::execute(ctx, spec, params)
            },
        },
        Task::Batch(jobs) => actions::batch_report(ctx, jobs),
    }
}
fn enter() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )
}
fn leave() -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = leave();
    }
}

pub fn run(ctx: Context, theme_flag: Option<&str>, preferences: &Preferences) -> Result<()> {
    let category_order = category_order_from_ids(&preferences.category_order)?;
    let background = detect::detect(preferences.detect_background.unwrap_or(true));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        previous(info);
    }));
    enter()?;
    let _guard = TerminalGuard;
    while event::poll(Duration::ZERO)? {
        let _ = event::read()?;
    }
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let monitor = Monitor::start(&ctx);
    let mut app = App::new(ctx);
    app.configure_theme(
        theme_flag,
        std::env::var("CQS_THEME").ok().as_deref(),
        background,
        preferences,
    );
    app.set_category_order(category_order);
    let result = (|| {
        loop {
            app.receive();
            app.snapshot = monitor.snapshot()?;
            if app.completed_since_refresh {
                monitor.refresh();
                app.completed_since_refresh = false;
            }
            if (app.exit_requested || app.ctx.cancel.load(Ordering::SeqCst)) && app.worker.is_none()
            {
                break;
            }
            app.viewport = terminal.get_frame().area();
            app.tick(Instant::now());
            terminal.draw(|f| app.draw(f))?;
            if !event::poll(app.frame_interval())? {
                continue;
            }
            let effect = match event::read()? {
                Input::Key(key) if key.kind == KeyEventKind::Press => app.key(key),
                Input::Mouse(mouse) => app.mouse(mouse, terminal.get_frame().area()),
                _ => Ok(Effect::None),
            };
            let outcome = match effect {
                Ok(Effect::Quit) => break,
                Ok(Effect::Run(task)) => app.start(&mut terminal, task),
                Ok(Effect::Refresh) => {
                    // Marked before signalling, so the next sample always clears it.
                    app.refreshing = Some(Instant::now());
                    monitor.refresh();
                    Ok(())
                }
                Ok(Effect::PublicIp) => monitor.query_public(),
                Ok(Effect::None) => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                app.fail_ui(error);
            }
        }
        Ok(())
    })();
    if let Some(worker) = app.worker.take() {
        app.ctx.cancel.store(true, Ordering::SeqCst);
        drop(app.confirmation.take());
        worker
            .join()
            .map_err(|_| Error::new("error.io", "worker panicked"))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn load_category_order(ctx: &Context) -> Result<Vec<usize>> {
        let preferences = Preferences::read(&ctx.paths.preferences, false)?;
        category_order_from_ids(&preferences.category_order)
    }
    fn app() -> App {
        App::new(Context::new(
            crate::config::Paths::from_env().unwrap(),
            crate::i18n::I18n::load(Some("en-US"), None).unwrap(),
            false,
        ))
    }

    #[test]
    fn language_switch_is_saved_and_preserves_other_preferences() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("locales")).unwrap();
        let mut ctx = crate::tests::context(temp.path());
        let preferences_path = ctx.paths.preferences.clone();
        crate::config::Preferences {
            language: None,
            locale_dir: Some(temp.path().join("locales")),
            category_order: vec!["kernel".into(), "system".into()],
            ..Default::default()
        }
        .write(&preferences_path)
        .unwrap();
        ctx.i18n = crate::i18n::I18n::load(Some("en-US"), None).unwrap();
        let mut app = App::new(ctx);

        assert!(matches!(
            app.key(key(KeyCode::Char('l'))).unwrap(),
            Effect::None
        ));

        assert_eq!(app.ctx.i18n.locale(), "zh-CN");
        let saved = crate::config::Preferences::read(&preferences_path, true).unwrap();
        assert_eq!(saved.language.as_deref(), Some("zh-CN"));
        assert_eq!(saved.locale_dir, Some(temp.path().join("locales")));
        assert_eq!(saved.category_order, ["kernel", "system"]);
        let restarted =
            crate::i18n::I18n::load(saved.language.as_deref(), saved.locale_dir.as_deref())
                .unwrap();
        assert_eq!(restarted.locale(), "zh-CN");
    }

    /// The 11 Reorderable_Category IDs in their default order.
    fn reorderable_ids() -> Vec<String> {
        DEFAULT_CATEGORY_ORDER
            .iter()
            .map(|id| (*id).to_owned())
            .collect()
    }

    #[test]
    fn category_order_defaults_for_legacy_preferences_and_rejects_invalid_ids() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        std::fs::write(&ctx.paths.preferences, "language = \"en-US\"\n").unwrap();
        assert_eq!(load_category_order(&ctx).unwrap(), default_category_order());
        assert_eq!(category_order_from_ids(&["kernel".into()]).unwrap()[0], 2);
        let mut defaults = default_category_order();
        defaults.sort_unstable();
        assert_eq!(
            defaults,
            (0..OPTIMIZATION_GROUP).collect::<Vec<_>>(),
            "every Reorderable_Category once, without optimization"
        );

        let mut unknown = reorderable_ids();
        unknown[0] = "unknown".into();
        let mut duplicate = reorderable_ids();
        duplicate[1] = duplicate[0].clone();
        // Repeated `optimization` entries are dropped; a repeated Reorderable_Category is not.
        let duplicate_after_optimization = ["optimization", "kernel", "optimization", "kernel"]
            .map(String::from)
            .to_vec();
        for ids in [unknown, duplicate, duplicate_after_optimization] {
            let error = category_order_from_ids(&ids).unwrap_err();
            assert_eq!(error.key, "error.config", "{ids:?}");
        }
    }

    #[test]
    fn legacy_category_order_listing_optimization_keeps_it_pinned() {
        let kernel = GROUPS.iter().position(|group| *group == "kernel").unwrap();
        let system = GROUPS.iter().position(|group| *group == "system").unwrap();
        // Older versions saved every category, optimization included, and let it move.
        let mut saved_default = reorderable_ids();
        saved_default.push("optimization".into());
        for (ids, first) in [
            (saved_default, default_category_order()[..2].to_vec()),
            (
                ["kernel", "optimization", "system"]
                    .map(String::from)
                    .to_vec(),
                vec![kernel, system],
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let ctx = crate::tests::context(temp.path());
            let quoted = ids.iter().map(|id| format!("{id:?}")).collect::<Vec<_>>();
            std::fs::write(
                &ctx.paths.preferences,
                format!("category_order = [{}]\n", quoted.join(", ")),
            )
            .unwrap();
            let mut app = App::new(ctx);
            app.set_category_order(load_category_order(&app.ctx).unwrap());

            assert_eq!(app.category_order[..2], first[..], "{ids:?}");
            assert_eq!(app.category_order.len(), DEFAULT_CATEGORY_ORDER.len());
            assert!(!app.category_order.contains(&OPTIMIZATION_GROUP));
            let display = app.display_order();
            assert_eq!(display[0], OPTIMIZATION_GROUP, "{ids:?}");
            assert_eq!(display[1..], app.category_order[..]);
        }
    }

    /// Writes `ids` with `Preferences::write`, reads the file back and loads the order.
    fn reload_category_order(ids: &[String]) -> Result<Vec<usize>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        crate::config::Preferences {
            category_order: ids.to_vec(),
            ..Default::default()
        }
        .write(&path)?;
        category_order_from_ids(&crate::config::Preferences::read(&path, true)?.category_order)
    }

    /// A permutation of the Reorderable_Category IDs, and the same list with one or more
    /// `optimization` entries inserted at arbitrary positions.
    fn permutation_with_optimization()
    -> impl proptest::strategy::Strategy<Value = (Vec<String>, Vec<String>)> {
        use proptest::prelude::*;
        (
            Just(reorderable_ids()).prop_shuffle(),
            prop::collection::vec(any::<prop::sample::Index>(), 1..=4),
        )
            .prop_map(|(ids, slots)| {
                let mut listed = ids.clone();
                for slot in slots {
                    let at = slot.index(listed.len() + 1);
                    listed.insert(at, GROUPS[OPTIMIZATION_GROUP].to_owned());
                }
                (ids, listed)
            })
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(100))]

        // Feature: tui-redesign, Property 1: category_order 往返
        /// **Validates: Requirements 3.6, 3.9**
        #[test]
        fn category_order_round_trips_through_config_toml(
            (ids, listed) in permutation_with_optimization()
        ) {
            let reloaded = |list: &[String]| {
                reload_category_order(list)
                    .unwrap()
                    .into_iter()
                    .map(|index| GROUPS[index])
                    .collect::<Vec<_>>()
            };
            proptest::prop_assert_eq!(reloaded(&ids), ids.clone());
            proptest::prop_assert_eq!(reloaded(&listed), ids);
        }
    }

    #[test]
    fn category_reordering_persists_stable_ids_and_preserves_other_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let locale_dir = temp.path().join("locales");
        std::fs::create_dir(&locale_dir).unwrap();
        let ctx = crate::tests::context(temp.path());
        let preferences_path = ctx.paths.preferences.clone();
        crate::config::Preferences {
            language: Some("en-US".into()),
            locale_dir: Some(locale_dir.clone()),
            category_order: Vec::new(),
            ..Default::default()
        }
        .write(&preferences_path)
        .unwrap();

        let mut app = App::new(ctx);
        let tools = GROUPS.iter().position(|group| *group == "tools").unwrap();
        app.group = tools;
        app.positions[tools] = 2;
        assert_eq!(app.category_order[6], tools);
        assert_eq!(
            app.category_position(),
            7,
            "the list starts with the pinned Optimization_Category"
        );
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT))
                .unwrap(),
            Effect::None
        ));

        assert_eq!(app.group, tools);
        assert_eq!(app.selected(), 2);
        assert_eq!(app.category_order[5], tools);
        let saved = crate::config::Preferences::read(&preferences_path, true).unwrap();
        assert_eq!(saved.language.as_deref(), Some("en-US"));
        assert_eq!(saved.locale_dir, Some(locale_dir));
        assert_eq!(
            saved.category_order[5], "tools",
            "the moved category is stored by stable ID"
        );
        assert_eq!(
            saved.category_order,
            app.category_order
                .iter()
                .map(|index| GROUPS[*index])
                .collect::<Vec<_>>(),
            "only the 11 Reorderable_Category IDs are saved"
        );

        let mut restarted = App::new(crate::tests::context(temp.path()));
        restarted.set_category_order(load_category_order(&restarted.ctx).unwrap());
        assert_eq!(restarted.category_order, app.category_order);
        assert_eq!(restarted.group, app.category_order[0]);
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn reordering_at_either_end_in_optimization_or_while_busy_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let preferences_path = ctx.paths.preferences.clone();
        let mut app = App::new(ctx);
        let order = app.category_order.clone();
        let (first, last) = (order[0], order[order.len() - 1]);
        let tools = GROUPS.iter().position(|group| *group == "tools").unwrap();
        for (group, code, busy) in [
            (first, KeyCode::Up, false),
            (last, KeyCode::Down, false),
            (OPTIMIZATION_GROUP, KeyCode::Up, false),
            (OPTIMIZATION_GROUP, KeyCode::Down, false),
            (tools, KeyCode::Up, true),
        ] {
            app.group = group;
            app.positions[group] = 1;
            if busy {
                app.worker = Some(thread::spawn(|| {}));
            }
            assert!(matches!(app.key(alt(code)).unwrap(), Effect::None));
            let label = format!("{} {code:?} busy={busy}", GROUPS[group]);
            assert_eq!(app.category_order, order, "{label}");
            assert_eq!(app.group, group, "{label}");
            assert_eq!(
                app.selected(),
                1,
                "the plain arrow does not act instead: {label}"
            );
            assert!(app.toast.is_none() && app.notice.is_none(), "{label}");
            if let Some(worker) = app.worker.take() {
                worker.join().unwrap();
            }
        }
        assert!(!preferences_path.exists(), "config.toml is never written");
    }

    #[test]
    fn a_failed_save_keeps_the_previous_order_and_shows_the_error() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        // config.toml cannot be written below a regular file.
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "").unwrap();
        ctx.paths.preferences = blocker.join("config.toml");
        let mut app = App::new(ctx);
        let order = app.category_order.clone();
        app.group = order[1];

        assert!(matches!(app.key(alt(KeyCode::Up)).unwrap(), Effect::None));

        assert_eq!(app.category_order, order);
        assert_eq!(app.group, order[1]);
        assert!(app.notice.is_some(), "the failure is shown");
        assert!(app.toast.is_none(), "no saved notice");
        assert_eq!(std::fs::read(&blocker).unwrap(), b"");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    /// Comparable identity of a row: its action ID, or a marker for the other kinds.
    fn row_id(row: &Row) -> &'static str {
        match row {
            Row::Action(spec) => spec.id,
            Row::Execute => "<execute>",
            Row::Section(_) => "<section>",
            Row::Rule => "<rule>",
        }
    }
    fn group(id: &str) -> usize {
        GROUPS.iter().position(|group| *group == id).unwrap()
    }

    #[test]
    fn digits_and_o_jump_to_categories_as_numbered_in_the_list() {
        let mut app = app();
        for n in 1..=9 {
            let digit = char::from_digit(n, 10).unwrap();
            app.key(key(KeyCode::Char(digit))).unwrap();
            assert_eq!(app.group, app.category_order[n as usize - 1], "{digit}");
            assert_eq!(app.group, app.display_order()[n as usize], "{digit}");
        }
        for code in [KeyCode::Char('o'), KeyCode::Char('O'), KeyCode::Char('0')] {
            app.key(key(KeyCode::Char('1'))).unwrap();
            app.key(key(code)).unwrap();
            assert!(app.optimization(), "{code:?}");
        }
        // Digits follow the loaded order, and every category keeps its row.
        let ids = ["tools", "ai", "system"].map(String::from);
        app.set_category_order(category_order_from_ids(&ids).unwrap());
        app.positions[group("ai")] = 2;
        app.key(key(KeyCode::Char('2'))).unwrap();
        assert_eq!(app.group, group("ai"));
        assert_eq!(app.selected(), 2);
        app.key(key(KeyCode::Char('3'))).unwrap();
        assert_eq!(app.group, group("system"));
    }

    #[test]
    fn esc_only_cancels_a_running_task() {
        let mut app = app();
        app.group = group("network");
        app.positions[app.group] = 2;
        assert!(matches!(app.key(key(KeyCode::Esc)).unwrap(), Effect::None));
        assert_eq!((app.group, app.selected()), (group("network"), 2));
        assert_eq!(app.phase, Phase::Idle);
        assert!(!app.ctx.cancel.load(Ordering::SeqCst));
        assert!(app.toast.is_none() && app.notice.is_none() && !app.help);

        app.worker = Some(thread::spawn(|| {}));
        app.phase = Phase::Running;
        assert!(matches!(app.key(key(KeyCode::Esc)).unwrap(), Effect::None));
        assert_eq!(app.phase, Phase::Cancelling);
        assert!(app.ctx.cancel.load(Ordering::SeqCst));
        assert!(!app.exit_requested, "Esc cancels without quitting");
        assert_eq!((app.group, app.selected()), (group("network"), 2));
        app.worker.take().unwrap().join().unwrap();
    }

    /// One Action_List movement key of Property 13.
    #[derive(Debug, Clone, Copy)]
    enum Move {
        Up,
        Down,
        Home,
        End,
    }
    impl Move {
        fn code(self) -> KeyCode {
            match self {
                Self::Up => KeyCode::Up,
                Self::Down => KeyCode::Down,
                Self::Home => KeyCode::Home,
                Self::End => KeyCode::End,
            }
        }
    }
    fn moves() -> impl proptest::strategy::Strategy<Value = Vec<Move>> {
        use proptest::prelude::*;
        prop::collection::vec(
            prop_oneof![
                4 => Just(Move::Up),
                4 => Just(Move::Down),
                1 => Just(Move::Home),
                1 => Just(Move::End),
            ],
            0..40,
        )
    }
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(100))]

        // Feature: tui-redesign, Property 13: 导航不落在 Section 上且可逆
        /// **Validates: Requirements 1.1, 1.2, 1.5, 1.6, 5.4**
        #[test]
        fn navigation_never_rests_on_a_section_and_is_reversible(
            // Positions in `display_order()`, so every category, the pinned one included,
            // can be drawn; B is always a different category from A.
            a in 0..GROUPS.len(),
            offset in 1..GROUPS.len(),
            in_a in moves(),
            in_b in moves(),
            forward in proptest::prelude::any::<bool>(),
        ) {
            let mut app = app();
            let order = app.display_order();
            let (a, b) = (order[a], order[(a + offset) % order.len()]);
            // Reach `a` by Category keys too, so switching is part of the sequence.
            while app.group != a {
                app.key(key(KeyCode::Tab)).unwrap();
            }
            for step in &in_a {
                app.key(key(step.code())).unwrap();
                let row = app.selected_row();
                proptest::prop_assert!(row.selectable(), "{} {step:?}: {row:?}", GROUPS[a]);
                // ↓ then ↑ comes back to the same row, wrapping at either end.
                let before = (app.selected(), row_id(&row));
                app.key(key(KeyCode::Down)).unwrap();
                proptest::prop_assert!(app.selected_row().selectable());
                app.key(key(KeyCode::Up)).unwrap();
                proptest::prop_assert_eq!((app.selected(), row_id(&app.selected_row())), before);
            }
            let kept = (app.selected(), row_id(&app.selected_row()));
            let (there, back) = if forward {
                (KeyCode::Right, KeyCode::Left)
            } else {
                (KeyCode::BackTab, KeyCode::Tab)
            };
            while app.group != b {
                app.key(key(there)).unwrap();
            }
            for step in &in_b {
                app.key(key(step.code())).unwrap();
                proptest::prop_assert!(app.selected_row().selectable());
            }
            while app.group != a {
                app.key(key(back)).unwrap();
            }
            proptest::prop_assert_eq!((app.selected(), row_id(&app.selected_row())), kept);
        }
    }
    #[test]
    fn cancellation_keys_work_while_task_logs_are_open() {
        for code in [KeyCode::Char('c'), KeyCode::Char('q'), KeyCode::Char('Q')] {
            let mut app = app();
            app.worker = Some(thread::spawn(|| {}));
            app.phase = Phase::Running;
            app.log_modal = true;
            let modifiers = if code == KeyCode::Char('c') {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            };
            assert!(matches!(
                app.key(KeyEvent::new(code, modifiers)).unwrap(),
                Effect::None
            ));
            assert!(app.ctx.cancel.load(Ordering::SeqCst), "{code:?}");
            assert!(app.exit_requested);
            assert_eq!(app.phase, Phase::Cancelling);
            app.worker.take().unwrap().join().unwrap();
            app.finish(Err(Error::new("error.cancelled", "")));
            assert!(app.exit_requested);
            assert!(!app.ctx.cancel.load(Ordering::SeqCst));
        }
    }

    /// Press the digit that Category_List shows beside category `id`.
    fn jump(app: &mut App, id: &str) {
        let n = app
            .category_order
            .iter()
            .position(|g| GROUPS[*g] == id)
            .unwrap()
            + 1;
        app.key(key(KeyCode::Char(char::from_digit(n as u32, 10).unwrap())))
            .unwrap();
        assert_eq!(GROUPS[app.group], id);
    }
    #[test]
    fn common_tools_and_ai_are_reachable_without_a_package_selector() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        jump(&mut app, "tools");
        // From the first row, ↓ walks the single-package installs under their Section title.
        app.key(key(KeyCode::Home)).unwrap();
        for package in crate::ops::tools::PACKAGES {
            match app.key(key(KeyCode::Enter)).unwrap() {
                Effect::Run(Task::Single {
                    spec,
                    params,
                    reviewed,
                }) => {
                    assert_eq!(spec.kind, Kind::ToolInstall(package));
                    assert!(params.is_empty());
                    assert!(!reviewed);
                }
                other => panic!("expected one package installation, got {other:?}"),
            }
            app.key(key(KeyCode::Down)).unwrap();
        }
        app.root = false;
        jump(&mut app, "ai");
        for _ in 0..3 {
            match app.key(key(KeyCode::Enter)).unwrap() {
                Effect::Run(Task::Single { spec, .. }) => assert!(!spec.external),
                other => panic!("expected a background binary installation, got {other:?}"),
            }
            app.key(key(KeyCode::Char('j'))).unwrap();
        }
        app.key(key(KeyCode::Char('o'))).unwrap();
        assert!(app.optimization());
        assert_eq!(GROUPS[app.group], "optimization");
    }
    #[test]
    fn ordinary_pages_never_select_or_dispatch_a_batch() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        // Leave the Optimization_View with the arrows, back on system.info.
        app.key(key(KeyCode::Char('0'))).unwrap();
        app.key(key(KeyCode::Right)).unwrap();
        assert_eq!(GROUPS[app.group], "system");
        assert!(matches!(app.selected_row(), Row::Action(spec) if spec.id == "system.info"));
        app.key(key(KeyCode::Char(' '))).unwrap();
        assert!(app.marked.is_empty());
        assert!(matches!(app.key(key(KeyCode::F(5))).unwrap(), Effect::None));
        app.marked.insert("bbr.enable".into()); // A saved optimization selection cannot leak into normal pages.
        assert!(matches!(app.key(key(KeyCode::F(5))).unwrap(), Effect::None));
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::Refresh
        ));
        assert!(app.overview);
    }
    #[test]
    fn optimization_enter_edits_and_saves_without_executing() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        app.key(key(KeyCode::Char('O'))).unwrap();
        // BBR, the fourth item, has a queue selector.
        for _ in 0..3 {
            app.key(key(KeyCode::Down)).unwrap();
        }
        assert!(matches!(app.selected_row(), Row::Action(spec) if spec.id == "bbr.enable"));
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        assert_eq!(app.form.as_ref().unwrap().mode, FormMode::Optimization);
        assert!(matches!(app.submit_form().unwrap(), Effect::None));
        assert!(app.marked.contains("bbr.enable"));
        match app.key(key(KeyCode::F(5))).unwrap() {
            Effect::Run(Task::Batch(jobs)) => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].0.id, "bbr.enable");
            }
            _ => panic!("expected optimization jobs"),
        }
    }
    #[test]
    fn arrows_move_actions_and_switch_categories_and_each_category_keeps_its_row() {
        let mut app = app();
        let system = group("system");
        assert_eq!(app.group, system);
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Char('j'))).unwrap();
        assert_eq!(app.selected(), 2, "↓ and j move the Action_List directly");
        app.key(key(KeyCode::Char('k'))).unwrap();
        app.key(key(KeyCode::Down)).unwrap();
        // → and Tab go down the Category_List, ← and Shift+Tab up; ↑/↓ never switch it.
        app.key(key(KeyCode::Right)).unwrap();
        assert_eq!(app.group, group("kernel"));
        app.key(key(KeyCode::End)).unwrap();
        let last = app.selectable_count(app.group) - 1;
        assert_eq!(app.selected(), last);
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(app.selected(), 0, "↓ wraps to the first row");
        app.key(key(KeyCode::Up)).unwrap();
        assert_eq!(app.selected(), last, "↑ wraps to the last row");
        app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
            .unwrap();
        assert_eq!(app.group, system);
        assert_eq!(app.selected(), 2, "system kept its row");
        app.key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.selected(), last, "kernel kept its row");

        // Both ends of `display_order()` wrap.
        app.key(key(KeyCode::Char('0'))).unwrap();
        app.key(key(KeyCode::Left)).unwrap();
        assert_eq!(app.group, *app.category_order.last().unwrap());
        app.key(key(KeyCode::Right)).unwrap();
        assert!(app.optimization());
        app.key(key(KeyCode::Right)).unwrap();
        assert_eq!(app.group, app.category_order[0]);
        // Home and End move the Action_List, not the Category_List.
        app.key(key(KeyCode::Home)).unwrap();
        assert_eq!((app.group, app.selected()), (system, 0));
    }
    #[test]
    fn rows_follow_the_section_table_and_the_selection_is_always_selectable() {
        let mut app = app();
        let rows = app.rows(OPTIMIZATION_GROUP);
        assert_eq!(rows.len(), actions::OPTIMIZATION.len() + 2);
        for (row, id) in rows.iter().zip(actions::OPTIMIZATION) {
            assert!(
                matches!(row, Row::Action(spec) if spec.id == *id),
                "{row:?}"
            );
        }
        assert!(matches!(&rows[rows.len() - 2..], [Row::Rule, Row::Execute]));
        // Grouped categories: each Section title, then its actions, in table order.
        for category in ["system", "clash", "network", "docker", "tools"] {
            let group = GROUPS.iter().position(|group| *group == category).unwrap();
            let expected = sections::of(category)
                .unwrap()
                .iter()
                .flat_map(|(section, ids)| std::iter::once(*section).chain(ids.iter().copied()))
                .collect::<Vec<_>>();
            let listed = app
                .rows(group)
                .into_iter()
                .map(|row| match row {
                    Row::Section(id) => id,
                    Row::Action(spec) => spec.id,
                    other => panic!("{category}: {other:?}"),
                })
                .collect::<Vec<_>>();
            assert_eq!(listed, expected);
        }
        for category in ["kernel", "ssh", "ai", "terminal", "tests", "app"] {
            let group = GROUPS.iter().position(|group| *group == category).unwrap();
            let ids = app
                .rows(group)
                .into_iter()
                .map(|row| match row {
                    Row::Action(spec) => spec.id,
                    other => panic!("{category} is not grouped: {other:?}"),
                })
                .collect::<Vec<_>>();
            let specs = actions::specs()
                .into_iter()
                .filter(|spec| spec.group == category)
                .map(|spec| spec.id)
                .collect::<Vec<_>>();
            assert_eq!(ids, specs);
        }
        for group in app.display_order() {
            app.group = group;
            for position in 0..app.selectable_count(group) {
                app.positions[group] = position;
                let row = app.selected_row();
                assert!(row.selectable(), "{} {position}: {row:?}", GROUPS[group]);
            }
        }

        // The Execute_Row is the last Selectable_Row; Enter on it runs the Checklist as F5 does.
        app.platform_error = None;
        app.root = true;
        app.group = OPTIMIZATION_GROUP;
        app.positions[OPTIMIZATION_GROUP] = actions::OPTIMIZATION.len();
        assert!(matches!(app.selected_row(), Row::Execute));
        app.marked.insert("bbr.enable".into());
        match app.key(key(KeyCode::Enter)).unwrap() {
            Effect::Run(Task::Batch(jobs)) => assert_eq!(jobs[0].0.id, "bbr.enable"),
            other => panic!("expected the Checklist to run, got {other:?}"),
        }
        assert!(app.form.is_none());
    }
    #[test]
    fn timezone_presets_cycle_and_custom_names_remain_editable() {
        let mut app = app();
        let spec = actions::find("system.timezone").unwrap();
        let choices = spec.fields[0].choices;
        for required in [
            "Asia/Singapore",
            "America/New_York",
            "America/Los_Angeles",
            "Europe/London",
        ] {
            assert!(choices.contains(&required));
        }
        app.form = Some(app.make_form(spec, FormMode::Optimization).unwrap());
        for choice in choices {
            assert_eq!(app.form.as_ref().unwrap().values[0].text, *choice);
            app.form_key(key(KeyCode::Right)).unwrap();
        }
        assert_eq!(app.form.as_ref().unwrap().values[0].text, "Asia/Shanghai");
        app.form_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        for c in "Africa/Johannesburg".chars() {
            app.form_key(key(KeyCode::Char(c))).unwrap();
        }
        app.form_key(key(KeyCode::Left)).unwrap();
        let value = &app.form.as_ref().unwrap().values[0];
        assert_eq!(value.text, "Africa/Johannesburg");
        assert_eq!(value.cursor, value.text.len() - 1);
        assert!(matches!(app.submit_form().unwrap(), Effect::None));
        assert_eq!(
            app.parameters["system.timezone"]["timezone"],
            "Africa/Johannesburg"
        );
        assert!(app.worker.is_none());
    }

    #[test]
    fn kernel_install_opens_the_standard_or_max_form_on_a_supported_host() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        app.group = 2;
        app.positions[2] = 1;
        assert!(matches!(app.activate().unwrap(), Effect::None));
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.spec.id, "kernel.install");
        assert_eq!(form.values[0].text, "standard");
        assert!(form.values[1].text.is_empty());
        assert_eq!(form.mode, FormMode::Run);
        assert!(app.worker.is_none());
    }
    #[test]
    fn platform_preview_keeps_overview_available_and_blocks_system_changes() {
        let mut app = app();
        app.platform_error = Some(Error::new("error.platform", "windows"));
        assert!(matches!(app.activate().unwrap(), Effect::Refresh));
        app.overview = false;
        app.positions[0] = 2;
        assert_eq!(app.activate().unwrap_err().key, "error.platform");
        assert!(app.worker.is_none());
    }
    #[test]
    fn ordinary_confirmation_defaults_to_cancel_and_typed_tokens_stay_exact() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.confirmation = Some(Confirmation {
            text: "test".into(),
            token: "YES".into(),
            input: InputBuffer::default(),
            reply: tx,
            scroll: 0,
            accept: false,
        });
        app.key(key(KeyCode::Enter)).unwrap();
        assert!(!rx.recv().unwrap());
        let (tx, rx) = mpsc::channel();
        app.confirmation = Some(Confirmation {
            text: "test".into(),
            token: "RESTORE".into(),
            input: InputBuffer::new("YES".into()),
            reply: tx,
            scroll: 0,
            accept: false,
        });
        app.key(key(KeyCode::Enter)).unwrap();
        assert!(rx.try_recv().is_err());
        app.confirmation
            .as_mut()
            .unwrap()
            .input
            .replace("RESTORE".into());
        app.key(key(KeyCode::Enter)).unwrap();
        assert!(rx.recv().unwrap());
    }
    #[test]
    fn mouse_form_buttons_save_or_cancel_without_background_clicks() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        app.group = OPTIMIZATION_GROUP;
        app.form = Some(
            app.make_form(actions::find("bbr.enable").unwrap(), FormMode::Optimization)
                .unwrap(),
        );
        let area = Rect::new(0, 0, 120, 36);
        let geometry = view::FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
        let click = |rect: Rect| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        let group = app.group;
        app.mouse(click(Rect::new(1, 3, 2, 1)), area).unwrap();
        assert_eq!(app.group, group);
        assert!(app.form.is_some());
        assert!(matches!(
            app.mouse(click(geometry.buttons[0]), area).unwrap(),
            Effect::None
        ));
        assert!(app.marked.contains("bbr.enable"));
        assert!(app.form.is_none());
        assert!(app.worker.is_none());
        app.form = Some(
            app.make_form(actions::find("tcp.tune").unwrap(), FormMode::Optimization)
                .unwrap(),
        );
        let geometry = view::FormGeometry::new(area, app.form.as_ref().unwrap(), &app);
        app.mouse(click(geometry.buttons[1]), area).unwrap();
        assert!(!app.marked.contains("tcp.tune"));
        assert!(app.form.is_none());
    }
    #[test]
    fn mouse_confirmation_requires_clicking_the_actual_button() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        let area = Rect::new(0, 0, 120, 36);
        app.confirmation = Some(Confirmation {
            text: "confirm".into(),
            token: "YES".into(),
            input: InputBuffer::default(),
            reply: tx,
            scroll: 0,
            accept: false,
        });
        let geometry = view::ConfirmationGeometry::new(
            area,
            app.confirmation.as_ref().unwrap(),
            &app.ctx.i18n,
        );
        let click = |rect: Rect| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click(geometry.sections[0]), area).unwrap();
        assert!(rx.try_recv().is_err());
        app.mouse(click(geometry.buttons[0]), area).unwrap();
        assert!(!rx.recv().unwrap());
    }
    #[test]
    fn completed_worker_keeps_every_log_and_completion_event() {
        for _ in 0..30 {
            let mut app = app();
            let (sender, receiver) = mpsc::channel();
            app.receiver = Some(receiver);
            app.worker = Some(thread::spawn(move || {
                sender.send(Event::Log("last message".into())).unwrap();
                sender.send(Event::Finished(Ok(()))).unwrap();
            }));
            while app.worker.is_some() {
                app.receive();
                thread::yield_now();
            }
            assert!(app.logs.iter().any(|l| l.text == "last message"));
            assert_eq!(app.phase, Phase::Completed);
            assert_eq!(app.logs.back().unwrap().tone, Tone::Success);
            assert_eq!(app.toast.as_ref().unwrap().tone, Tone::Success);
        }
    }
    #[test]
    fn cancellation_is_not_reported_as_failure() {
        let mut app = app();
        app.finish(Err(Error::new("error.cancelled", "")));
        assert_eq!(app.phase, Phase::Cancelled);
        assert_eq!(app.toast.as_ref().unwrap().tone, Tone::Warning);
    }
    #[test]
    fn single_task_results_are_recorded_and_cancellation_is_not_a_failure() {
        let job = |id| (actions::find(id).unwrap(), Parameters::new());
        let cases = [
            (Ok(()), Outcome::Done),
            (Err(Error::new("error.cancelled", "")), Outcome::Cancelled),
            (Err(Error::new("error.io", "disk full")), Outcome::Failed),
        ];
        for (result, expected) in cases {
            let mut app = app();
            let (spec, params) = job("tcp.tune");
            app.record_start(&Task::Single {
                spec,
                params,
                reviewed: false,
            });
            assert_eq!(app.records.recent, ["tcp.tune"]);
            assert!(app.records.last.is_empty());
            let (sender, receiver) = mpsc::channel();
            app.receiver = Some(receiver);
            app.worker = Some(thread::spawn(move || {
                sender.send(Event::Finished(result)).unwrap();
            }));
            while app.worker.is_some() {
                app.receive();
                thread::yield_now();
            }
            assert_eq!(app.records.last["tcp.tune"].outcome, expected);
            assert!(app.single_task.is_none());
        }
        // A batch is recorded from its Step events, never from Finished.
        let mut app = app();
        app.record_start(&Task::Batch(vec![job("tcp.tune"), job("bbr.enable")]));
        assert!(app.single_task.is_none());
        app.finish(Ok(()));
        assert!(app.records.recent.is_empty() && app.records.last.is_empty());
    }
    #[test]
    fn help_overlay_swallows_navigation_and_closes_without_side_effects() {
        let mut app = app();
        app.key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help);
        let group = app.group;
        let positions = app.positions;
        for code in [
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Tab,
            KeyCode::Char('3'),
            KeyCode::Char('o'),
        ] {
            assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
        }
        assert_eq!(app.group, group);
        assert_eq!(app.positions, positions);
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        assert_eq!(
            (app.group, app.positions),
            (group, positions),
            "Esc closed help only"
        );
        app.key(key(KeyCode::F(1))).unwrap();
        assert!(app.help);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 15,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click, Rect::new(0, 0, 120, 36)).unwrap();
        assert!(app.help);
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        assert_eq!(
            app.group, group,
            "background clicks and closing help do not reach the menu"
        );
    }
    #[test]
    fn empty_optimization_run_warns_without_blocking() {
        let mut app = app();
        app.group = OPTIMIZATION_GROUP;
        assert!(matches!(app.key(key(KeyCode::F(5))).unwrap(), Effect::None));
        assert!(app.notice.is_none());
        assert_eq!(app.toast.as_ref().unwrap().tone, Tone::Warning);
    }

    fn editable_optimization() -> App {
        let mut app = app();
        app.group = OPTIMIZATION_GROUP;
        app.platform_error = None;
        app.root = true;
        app
    }

    #[test]
    fn optimization_shortcuts_and_execute_row_always_dispatch_a_batch() {
        for code in [
            KeyCode::Char('x'),
            KeyCode::Char('X'),
            KeyCode::F(5),
            KeyCode::Enter,
        ] {
            let mut app = editable_optimization();
            app.marked.insert("bbr.enable".into());
            app.positions[OPTIMIZATION_GROUP] = actions::OPTIMIZATION.len();
            let Effect::Run(Task::Batch(jobs)) = app.key(key(code)).unwrap() else {
                panic!("{code:?} must dispatch a batch, even with one item");
            };
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].0.id, "bbr.enable");
            assert_eq!(jobs[0].1, jobs[0].0.defaults());
        }
        for (width, height) in [(80, 24), (120, 36)] {
            let mut app = editable_optimization();
            app.marked.insert("bbr.enable".into());
            app.positions[OPTIMIZATION_GROUP] = actions::OPTIMIZATION.len();
            let area = Rect::new(0, 0, width, height);
            let regions = view::Regions::new(area, &app);
            let rows = app.rows(OPTIMIZATION_GROUP);
            let row = row_index(&rows, app.selected());
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: regions.action_rows.x + 8,
                row: regions.action_rows.y + (row - regions.action_offset) as u16,
                modifiers: KeyModifiers::NONE,
            };
            assert!(
                matches!(app.mouse(click, area).unwrap(), Effect::Run(Task::Batch(jobs)) if jobs.len() == 1)
            );
        }
    }

    #[test]
    fn recommended_selection_is_exact_and_preserves_other_parameters() {
        for code in [KeyCode::Char('a'), KeyCode::Char('A')] {
            let mut app = editable_optimization();
            app.marked.insert("ssh.key".into());
            app.parameters.insert(
                "system.clean".into(),
                Parameters::from([("scope".into(), "full".into())]),
            );
            app.parameters.insert(
                "tcp.tune".into(),
                Parameters::from([("rtt".into(), "80".into())]),
            );
            app.parameters.insert(
                "ssh.port".into(),
                Parameters::from([("port".into(), "2222".into())]),
            );
            let mut expected = app.parameters.clone();
            expected.insert(
                "system.clean".into(),
                Parameters::from([("scope".into(), "standard".into())]),
            );
            assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
            assert_eq!(
                app.marked,
                RECOMMENDED.iter().map(|id| (*id).to_owned()).collect()
            );
            assert_eq!(app.parameters, expected);
            assert_eq!(app.toast.as_ref().unwrap().tone, Tone::Success);
            assert!(app.worker.is_none());
            let Effect::Run(Task::Batch(jobs)) = app.key(key(KeyCode::Char('x'))).unwrap() else {
                panic!("batch")
            };
            assert_eq!(
                jobs.iter().map(|(spec, _)| spec.id).collect::<Vec<_>>(),
                RECOMMENDED
            );
            assert_eq!(jobs[2].1["rtt"], "80");
            assert_eq!(jobs[2].1["bandwidth"], "1000");
        }
    }

    #[test]
    fn unavailable_recommendations_leave_the_checklist_and_parameters_untouched() {
        for platform in [false, true] {
            let mut app = editable_optimization();
            if platform {
                app.platform_error = Some(Error::new("error.platform", "unsupported"));
            } else {
                app.root = false;
            }
            app.marked.insert("ssh.port".into());
            app.parameters.insert(
                "system.clean".into(),
                Parameters::from([("scope".into(), "full".into())]),
            );
            let before = (app.marked.clone(), app.parameters.clone());
            let error = app.key(key(KeyCode::Char('a'))).unwrap_err();
            assert_eq!(
                error.key,
                if platform {
                    "error.platform"
                } else {
                    "error.root"
                }
            );
            app.fail_ui(error);
            assert!(app.notice.is_some());
            assert_eq!((app.marked, app.parameters), before);
            assert!(app.toast.is_none());
        }
    }

    #[test]
    fn optimization_keys_do_not_act_in_other_categories_or_while_busy() {
        let keys = [
            KeyCode::Char(' '),
            KeyCode::Char('x'),
            KeyCode::Char('X'),
            KeyCode::F(5),
            KeyCode::Char('a'),
            KeyCode::Char('A'),
        ];
        for group in default_category_order() {
            let mut app = editable_optimization();
            app.group = group;
            app.marked.insert("ssh.port".into());
            for code in keys {
                assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
                assert_eq!(app.marked, BTreeSet::from(["ssh.port".into()]));
                assert!(app.parameters.is_empty() && app.worker.is_none());
            }
        }
        let mut app = editable_optimization();
        app.worker = Some(thread::spawn(|| {}));
        for code in keys {
            assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
            assert!(app.marked.is_empty() && app.parameters.is_empty());
        }
        app.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn invalid_optimization_values_open_the_form_without_dispatching() {
        for (id, params) in [
            ("ssh.key", Parameters::new()),
            (
                "tcp.tune",
                Parameters::from([("rtt".into(), "invalid".into())]),
            ),
        ] {
            let mut app = editable_optimization();
            app.marked.insert(id.into());
            app.parameters.insert(id.into(), params);
            assert!(matches!(
                app.key(key(KeyCode::Char('x'))).unwrap(),
                Effect::None
            ));
            let form = app.form.as_ref().unwrap();
            assert_eq!(form.mode, FormMode::Optimization);
            assert_eq!(form.spec.id, id);
            assert!(form.error.is_some());
            assert!(app.worker.is_none());
        }
    }

    #[test]
    fn a_one_item_task_reaches_batch_confirmation_and_rejection_runs_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        ctx.yes = false;
        let (sender, receiver) = mpsc::channel();
        ctx.events = Some(sender);
        let spec = actions::find("system.update").unwrap();
        let task = Task::Batch(vec![(spec.clone(), spec.defaults())]);
        let worker = thread::spawn(move || perform(&ctx, &task));
        let Event::Confirm { text, token, reply } =
            receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("batch confirmation must precede any operation or Step event");
        };
        assert!(text.contains("1. Update packages"), "{text}");
        assert_eq!(token, "YES");
        reply.send(false).unwrap();
        let report = worker.join().unwrap();
        assert_eq!(report.result.unwrap_err().key, "error.cancelled");
        assert_eq!(report.steps.len(), 1);
        assert!(report.steps[0].outcome.is_none() && report.steps[0].elapsed.is_none());
        assert!(
            receiver
                .try_iter()
                .all(|event| !matches!(event, Event::Step(_)))
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn batch_records_follow_step_events_and_handoff_reports_without_inventing_skipped_runs() {
        use crate::runtime::{Step, StepState};
        let mut app = editable_optimization();
        let (sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        for state in [
            StepState::Started,
            StepState::Finished {
                outcome: Outcome::Cancelled,
                elapsed: Duration::from_secs(2),
            },
        ] {
            sender
                .send(Event::Step(Step {
                    index: 1,
                    total: 1,
                    id: "tcp.tune",
                    state,
                }))
                .unwrap();
            app.receive();
        }
        assert_eq!(app.records.recent, ["tcp.tune"]);
        assert_eq!(app.records.last["tcp.tune"].outcome, Outcome::Cancelled);
        let report = actions::BatchReport {
            steps: vec![
                actions::StepRecord {
                    id: "bbr.enable",
                    outcome: Some(Outcome::Failed),
                    elapsed: Some(Duration::from_secs(3)),
                },
                actions::StepRecord {
                    id: "ssh.port",
                    outcome: None,
                    elapsed: None,
                },
            ],
            result: Err(Error::new("error.batch", "bbr.enable")),
        };
        app.record_report(&report);
        assert_eq!(app.records.recent, ["bbr.enable", "tcp.tune"]);
        assert_eq!(app.records.last["bbr.enable"].outcome, Outcome::Failed);
        assert!(!app.records.last.contains_key("ssh.port"));
    }
    #[test]
    fn log_scrolling_is_clamped_to_the_visible_window() {
        let mut app = app();
        app.log_modal = true;
        for n in 0..200 {
            app.log(format!("line {n}"));
        }
        let body = view::log_body(&app, app.viewport);
        let max = 200 - body.height;
        for _ in 0..100 {
            app.key(key(KeyCode::PageUp)).unwrap();
        }
        assert_eq!(app.scroll, max);
        app.key(key(KeyCode::End)).unwrap();
        assert_eq!(app.scroll, 0);
        app.key(key(KeyCode::Home)).unwrap();
        assert_eq!(app.scroll, max);
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(app.scroll, max - 1);
    }
    #[test]
    fn gauges_glide_to_new_samples_and_frames_relax_when_idle() {
        let mut app = app();
        let start = Instant::now();
        app.tick(start);
        assert!(
            app.gauges.iter().all(Option::is_none),
            "nothing measured yet"
        );
        assert_eq!(
            app.frame_interval(),
            motion::SPINNER_FRAME,
            "readings are pending"
        );

        app.snapshot.local.updated = Some(start);
        app.snapshot.local.cpu_usage = crate::dashboard::Reading::Value(40.0);
        app.snapshot.local.memory =
            crate::dashboard::Reading::Value(crate::dashboard::Usage { used: 1, total: 4 });
        app.tick(start);
        let cpu = app.gauges[0].unwrap();
        assert_eq!(cpu.value(start), 0.0, "a new gauge grows from empty");
        assert_eq!(app.frame_interval(), motion::ANIMATION_FRAME);

        let settled = start + motion::GLIDE;
        app.tick(settled);
        assert!((app.gauges[1].unwrap().value(settled) - 25.0).abs() < 1e-9);
        assert_eq!(app.frame_interval(), motion::IDLE_FRAME);

        app.snapshot.local.cpu_usage = crate::dashboard::Reading::Value(80.0);
        app.tick(settled);
        assert!(app.gauges[0].unwrap().moving(settled));
        app.snapshot.local.memory = crate::dashboard::Reading::Loading;
        app.tick(settled);
        assert!(app.gauges[1].is_none(), "a lost reading drops its bar");
    }
    #[test]
    fn refresh_feedback_clears_once_a_newer_sample_arrives() {
        let mut app = app();
        let requested = Instant::now();
        app.refreshing = Some(requested);
        app.snapshot.local.updated = Some(requested - Duration::from_secs(1));
        app.tick(requested);
        assert!(app.refreshing.is_some());
        app.snapshot.local.updated = Some(requested + Duration::from_millis(5));
        app.tick(requested + Duration::from_millis(10));
        assert!(app.refreshing.is_none());
    }
    #[test]
    fn long_confirmations_page_within_their_text() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.confirmation = Some(Confirmation {
            text: (0..80)
                .map(|n| format!("item {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            token: "YES".into(),
            input: InputBuffer::default(),
            reply: tx,
            scroll: 0,
            accept: false,
        });
        let max = view::ConfirmationGeometry::new(
            app.viewport,
            app.confirmation.as_ref().unwrap(),
            &app.ctx.i18n,
        )
        .max_scroll();
        assert!(max > 0);
        for _ in 0..50 {
            app.key(key(KeyCode::PageDown)).unwrap();
        }
        assert_eq!(app.confirmation.as_ref().unwrap().scroll, max);
    }

    /// Wait for a form's items to arrive from the background reader.
    fn settle(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.loading.is_some() {
            assert!(Instant::now() < deadline, "the listing never arrived");
            app.receive();
            thread::sleep(Duration::from_millis(5));
        }
    }
    /// An app on a scratch data directory, positioned on action `id`.
    fn app_at(root: &std::path::Path, id: &str) -> App {
        let mut app = App::new(crate::tests::context(root));
        app.platform_error = None;
        let spec = actions::find(id).unwrap();
        app.group = GROUPS
            .iter()
            .position(|group| *group == spec.group)
            .unwrap();
        app.positions[app.group] = app
            .rows(app.group)
            .iter()
            .filter(|row| row.selectable())
            .position(|row| matches!(row, Row::Action(spec) if spec.id == id))
            .unwrap();
        app
    }
    fn save_profiles(ctx: &Context, names: &[&str], current: &str) {
        let profiles = names
            .iter()
            .map(|name| {
                format!(
                    "  - name: {name}\n    url: https://{name}.example/sub?token=secret\n    path: {name}.yaml\n"
                )
            })
            .collect::<String>();
        std::fs::create_dir_all(ctx.paths.clash_profiles.parent().unwrap()).unwrap();
        std::fs::write(
            &ctx.paths.clash_profiles,
            format!("use: {current}\nprofiles:\n{profiles}"),
        )
        .unwrap();
    }
    fn choice(value: &str, current: bool, children: Vec<Choice>) -> Choice {
        Choice {
            value: value.into(),
            detail: String::new(),
            current,
            children,
        }
    }

    #[test]
    fn saved_subscriptions_open_as_a_list_and_one_choice_submits() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = app_at(temp.path(), "clash.sub.use");
        save_profiles(&app.ctx, &["home", "work"], "work");
        app.key(key(KeyCode::Enter)).unwrap();
        settle(&mut app);
        let form = app.form.as_ref().expect("the form opened");
        let picker = form.picker.as_ref().expect("straight into the list");
        assert!(picker.with_form);
        assert_eq!(form.values[0].text, "work", "the selected subscription");
        assert_eq!(picker.cursor, 1);
        assert_eq!(
            form.choices(0)[0].detail,
            "home.example",
            "only the host: the URL carries a token"
        );
        app.key(key(KeyCode::Up)).unwrap();
        match app.key(key(KeyCode::Enter)).unwrap() {
            Effect::Run(Task::Single { spec, params, .. }) => {
                assert_eq!(spec.id, "clash.sub.use");
                assert_eq!(params["name"], "home");
            }
            other => panic!("expected the choice to run, got {other:?}"),
        }
        assert!(app.form.is_none());
    }

    #[test]
    fn typing_filters_the_list_and_esc_leaves_without_choosing() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = app_at(temp.path(), "clash.sub.update");
        save_profiles(&app.ctx, &["alpha", "beta", "gamma"], "alpha");
        app.key(key(KeyCode::Enter)).unwrap();
        settle(&mut app);
        for c in "MA".chars() {
            app.key(key(KeyCode::Char(c))).unwrap();
        }
        let shown = |app: &App| {
            let form = app.form.as_ref().unwrap();
            let picker = form.picker.as_ref().unwrap();
            let values = picker
                .matches(form.choices(0))
                .iter()
                .map(|choice| choice.value.clone())
                .collect::<Vec<_>>();
            (values, picker.cursor)
        };
        assert_eq!(
            shown(&app),
            (vec!["gamma".to_owned()], 0),
            "case-insensitive"
        );
        app.key(key(KeyCode::Backspace)).unwrap();
        app.key(key(KeyCode::Backspace)).unwrap();
        assert_eq!(shown(&app).0.len(), 3);
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(
            app.form.is_none(),
            "the list opened with its form, so both close"
        );
        assert!(!app.parameters.contains_key("clash.sub.update"));
    }

    #[test]
    fn empty_or_unreadable_listings_explain_instead_of_opening_a_form() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = app_at(temp.path(), "clash.sub.remove");
        app.key(key(KeyCode::Enter)).unwrap();
        settle(&mut app);
        assert!(app.form.is_none() && app.notice.is_none());
        let toast = app.toast.as_ref().expect("an empty list is explained");
        assert_eq!(toast.tone, Tone::Warning);
        assert_eq!(toast.text, app.ctx.i18n.text("toast.no_subscriptions"));

        std::fs::create_dir_all(app.ctx.paths.clash_profiles.parent().unwrap()).unwrap();
        std::fs::write(&app.ctx.paths.clash_profiles, "profiles: [unclosed\n").unwrap();
        app.key(key(KeyCode::Enter)).unwrap();
        settle(&mut app);
        assert!(app.form.is_none());
        let notice = app.notice.as_deref().expect("a read failure is reported");
        assert!(
            notice.contains("invalid subscription metadata YAML"),
            "{notice}"
        );
    }

    #[test]
    fn changing_a_group_resets_its_node_to_that_groups_current_member() {
        let mut app = app();
        let mut form = app
            .make_form(actions::find("clash.proxy.select").unwrap(), FormMode::Run)
            .unwrap();
        form.attach(vec![
            vec![
                choice(
                    "Proxy",
                    false,
                    vec![choice("HK", false, vec![]), choice("JP", true, vec![])],
                ),
                choice(
                    "Streaming",
                    false,
                    vec![choice("US", true, vec![]), choice("HK", false, vec![])],
                ),
            ],
            Vec::new(),
        ]);
        assert_eq!(form.values[0].text, "Proxy");
        assert_eq!(
            form.values[1].text, "JP",
            "a group starts at its current member"
        );
        app.form = Some(form);
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Left)).unwrap();
        assert_eq!(app.form.as_ref().unwrap().values[1].text, "HK");
        app.key(key(KeyCode::Up)).unwrap();
        app.key(key(KeyCode::Right)).unwrap();
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.values[0].text, "Streaming");
        assert_eq!(
            form.values[1].text, "US",
            "the old node was chosen in another group"
        );
    }

    /// A one-shot controller answering any request with `body`.
    fn controller(body: String) -> (String, JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let size = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                assert_ne!(size, 0);
                request.extend_from_slice(&buffer[..size]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (address, server)
    }

    #[test]
    fn proxy_selection_picks_a_group_then_one_of_its_nodes() {
        let body = serde_json::json!({"proxies": {
            "GLOBAL": {"type": "Selector", "all": ["Proxy", "Streaming"], "now": "Proxy"},
            "Proxy": {"type": "Selector", "all": ["HK", "JP"], "now": "HK"},
            "Streaming": {"type": "Selector", "all": ["US", "JP"], "now": "US"},
            "HK": {"type": "Vmess"}, "JP": {"type": "Trojan"}, "US": {"type": "Vless"}
        }})
        .to_string();
        let (address, server) = controller(body);
        let temp = tempfile::tempdir().unwrap();
        let mut app = app_at(temp.path(), "clash.proxy.select");
        std::fs::create_dir_all(&app.ctx.paths.clash_data).unwrap();
        std::fs::write(
            &app.ctx.paths.clash_config,
            format!("external-controller: {address}\n"),
        )
        .unwrap();
        app.key(key(KeyCode::Enter)).unwrap();
        settle(&mut app);
        let form = app.form.as_ref().expect("the form opened");
        assert_eq!(
            (form.values[0].text.as_str(), form.values[1].text.as_str()),
            ("Proxy", "HK")
        );
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Enter)).unwrap();
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.index, 1);
        assert_eq!(
            (form.values[0].text.as_str(), form.values[1].text.as_str()),
            ("Streaming", "US")
        );
        let picker = form.picker.as_ref().expect("the node list follows");
        assert!(!picker.with_form);
        assert_eq!(picker.cursor, 0, "on the group's current node");
        app.key(key(KeyCode::Down)).unwrap();
        match app.key(key(KeyCode::Enter)).unwrap() {
            Effect::Run(Task::Single { params, .. }) => {
                assert_eq!(params["proxy_group"], "Streaming");
                assert_eq!(params["proxy_node"], "JP");
            }
            other => panic!("expected the choice to run, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn enter_moves_through_fields_and_submits_from_the_last() {
        let mut app = app();
        app.form = Some(
            app.make_form(actions::find("tcp.tune").unwrap(), FormMode::Run)
                .unwrap(),
        );
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        assert_eq!(app.form.as_ref().unwrap().index, 1);
        match app.key(key(KeyCode::Enter)).unwrap() {
            Effect::Run(Task::Single { spec, .. }) => assert_eq!(spec.id, "tcp.tune"),
            other => panic!("expected the form to submit, got {other:?}"),
        }
    }

    #[test]
    fn esc_abandons_a_pending_form_and_its_read() {
        let mut app = app();
        let (tx, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.loading = Some(Loading {
            spec: actions::find("clash.sub.use").unwrap(),
            mode: FormMode::Run,
            receiver,
            cancel: cancel.clone(),
            started: Instant::now(),
        });
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        assert!(app.loading.is_some(), "other keys wait for the form");
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(app.loading.is_none());
        assert!(cancel.load(Ordering::SeqCst), "the read is told to stop");
        let _ = tx.send(Ok(vec![Vec::new()]));
        app.receive();
        assert!(app.form.is_none(), "a late result does not reopen the form");
    }

    /// An app on a scratch data directory, on a supported platform as root, whose
    /// Command_Palette is open with `query` typed into it.
    fn palette_app(root: &std::path::Path, query: &str) -> App {
        let mut app = App::new(crate::tests::context(root));
        app.platform_error = None;
        app.root = true;
        app.key(key(KeyCode::Char('/'))).unwrap();
        for c in query.chars() {
            app.key(key(KeyCode::Char(c))).unwrap();
        }
        app
    }
    fn highlighted(app: &App) -> Option<&'static str> {
        app.palette_highlighted().map(|spec| spec.id)
    }
    /// The query and its cursor.
    fn query(app: &App) -> (&str, usize) {
        let palette = app.command_palette.as_ref().expect("open Command_Palette");
        (&palette.query.text, palette.query.cursor)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn slash_opens_the_palette_and_printable_keys_edit_its_query() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "");
        assert_eq!(query(&app), ("", 0));
        assert_eq!(app.command_palette.as_ref().unwrap().cursor, 0);
        let before = (app.group, app.positions);
        // Digits and letter shortcuts are text here: none switches category, quits, opens
        // help, toggles the logs or switches the language.
        for c in "o1q?/gL ".chars() {
            let effect = app.key(key(KeyCode::Char(c))).unwrap();
            assert!(matches!(effect, Effect::None), "{c}: {effect:?}");
        }
        assert_eq!(query(&app), ("o1q?/gL ", 8));
        assert_eq!((app.group, app.positions), before);
        assert!(!app.help && !app.logs_open && app.notice.is_none());
        assert_eq!(app.ctx.i18n.locale(), "en-US");
        assert!(!app.ctx.paths.preferences.exists(), "nothing was saved");

        app.key(ctrl('u')).unwrap();
        assert_eq!(query(&app), ("", 0));
        for c in "bbr".chars() {
            app.key(key(KeyCode::Char(c))).unwrap();
        }
        app.key(key(KeyCode::Home)).unwrap();
        app.key(key(KeyCode::Delete)).unwrap();
        assert_eq!(query(&app), ("br", 0));
        app.key(key(KeyCode::Right)).unwrap();
        app.key(key(KeyCode::Char('x'))).unwrap();
        assert_eq!(query(&app), ("bxr", 2));
        app.key(key(KeyCode::Left)).unwrap();
        app.key(key(KeyCode::Backspace)).unwrap();
        assert_eq!(query(&app), ("xr", 0));
        app.key(key(KeyCode::End)).unwrap();
        app.key(key(KeyCode::Backspace)).unwrap();
        app.key(key(KeyCode::Backspace)).unwrap();
        for c in "bbr".chars() {
            app.key(key(KeyCode::Char(c))).unwrap();
        }
        assert_eq!(query(&app), ("bbr", 3));

        // ↑↓ wrap over the hits, kernel.install first; PgUp/PgDn stop at either end.
        let ids = [
            "kernel.install",
            "kernel.remove",
            "bbr.status",
            "bbr.enable",
            "bbr.restore",
        ];
        assert_eq!(highlighted(&app), Some(ids[0]));
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(highlighted(&app), Some(ids[1]));
        app.key(key(KeyCode::Up)).unwrap();
        app.key(key(KeyCode::Up)).unwrap();
        assert_eq!(highlighted(&app), Some(ids[4]), "↑ wraps to the last hit");
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(highlighted(&app), Some(ids[0]), "↓ wraps to the first hit");
        app.key(key(KeyCode::PageDown)).unwrap();
        assert_eq!(highlighted(&app), Some(ids[4]));
        app.key(key(KeyCode::PageUp)).unwrap();
        assert_eq!(highlighted(&app), Some(ids[0]));
        // A new query starts from its first result.
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Char('.'))).unwrap();
        assert_eq!(highlighted(&app), Some("bbr.status"));
        assert_eq!((app.group, app.positions), before);
    }

    #[test]
    fn palette_enter_activates_the_highlighted_action_as_the_action_list_does() {
        let temp = tempfile::tempdir().unwrap();
        for (id, from) in [
            ("bbr.status", group("system")),
            ("system.info", group("network")),
            ("system.public_ip", group("tools")),
            ("tcp.tune", OPTIMIZATION_GROUP),
        ] {
            let mut app = palette_app(temp.path(), id);
            app.group = from;
            app.positions[from] = 1;
            let before = (app.group, app.positions);
            assert_eq!(highlighted(&app), Some(id));
            let effect = app.key(key(KeyCode::Enter)).unwrap();
            assert!(
                app.command_palette.is_none(),
                "{id}: Enter closes the palette"
            );
            match (id, effect) {
                ("bbr.status", Effect::Run(Task::Single { spec, reviewed, .. })) => {
                    assert_eq!(spec.id, "bbr.status");
                    assert!(!reviewed);
                }
                ("system.info", Effect::Refresh) => assert!(app.overview),
                ("system.public_ip", Effect::PublicIp) => (),
                // From the Optimization_Category too, a Form_Action opens its Run-mode form.
                ("tcp.tune", Effect::None) => {
                    let form = app.form.as_ref().expect("the Run-mode form");
                    assert_eq!((form.spec.id, form.mode), ("tcp.tune", FormMode::Run));
                    assert!(app.marked.is_empty());
                }
                (id, effect) => panic!("{id}: {effect:?}"),
            }
            assert_eq!(
                (app.group, app.positions),
                before,
                "{id}: Enter leaves Current_Category and its selection alone"
            );
            assert!(app.notice.is_none(), "{id}");
        }
    }

    #[test]
    fn palette_enter_while_busy_shows_busy_and_stays_open() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "bbr.status");
        app.worker = Some(thread::spawn(|| {}));
        app.phase = Phase::Running;
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        let busy = app.ctx.i18n.error(&Error::new("error.busy", ""));
        assert_eq!(app.notice.as_deref(), Some(busy.as_str()));
        assert_eq!(query(&app).0, "bbr.status", "the palette stays open");
        assert_eq!(app.phase, Phase::Running, "the task is not cancelled");
        // Dismissing the notice returns to the palette.
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(app.notice.is_none());
        assert_eq!(highlighted(&app), Some("bbr.status"));
        app.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn palette_enter_on_an_unavailable_action_explains_without_running() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "bbr.restore");
        app.root = false;
        let reason = app.ctx.i18n.error(
            &app.available(&actions::find("bbr.restore").unwrap())
                .unwrap_err(),
        );
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        assert_eq!(app.notice.as_deref(), Some(reason.as_str()));
        assert_eq!(
            highlighted(&app),
            Some("bbr.restore"),
            "the palette stays open"
        );
        assert!(app.form.is_none() && app.worker.is_none() && app.records.recent.is_empty());
    }

    #[test]
    fn palette_tab_selects_the_action_in_its_category_and_esc_changes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "bbr");
        app.group = OPTIMIZATION_GROUP;
        app.positions[group("network")] = 0;
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Down)).unwrap();
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(highlighted(&app), Some("bbr.enable"));
        let positions = app.positions;
        assert!(matches!(app.key(key(KeyCode::Tab)).unwrap(), Effect::None));
        assert!(app.command_palette.is_none());
        assert_eq!(app.group, group("network"));
        assert!(matches!(app.selected_row(), Row::Action(spec) if spec.id == "bbr.enable"));
        assert!(
            app.form.is_none() && app.notice.is_none(),
            "Tab only selects"
        );
        let others = |positions: [usize; GROUPS.len()]| {
            let mut positions = positions;
            positions[group("network")] = 0;
            positions
        };
        assert_eq!(others(app.positions), others(positions));

        // Esc closes without changing anything, whatever was typed or highlighted.
        app.group = group("docker");
        app.positions[app.group] = 2;
        let before = (app.group, app.positions, app.marked.clone());
        app.key(key(KeyCode::Char('/'))).unwrap();
        for c in "tcp".chars() {
            app.key(key(KeyCode::Char(c))).unwrap();
        }
        app.key(key(KeyCode::Down)).unwrap();
        assert!(matches!(app.key(key(KeyCode::Esc)).unwrap(), Effect::None));
        assert!(app.command_palette.is_none());
        assert_eq!((app.group, app.positions, app.marked.clone()), before);
        assert!(app.form.is_none() && app.notice.is_none() && !app.overview);
    }

    #[test]
    fn an_empty_query_lists_recent_use_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "");
        assert_eq!(app.palette_results().count(), 0, "nothing has run yet");
        assert_eq!(highlighted(&app), None);
        // Enter and Tab with nothing highlighted keep the palette open.
        app.key(key(KeyCode::Enter)).unwrap();
        app.key(key(KeyCode::Tab)).unwrap();
        assert!(app.command_palette.is_some() && app.notice.is_none());

        app.records.started("tcp.tune");
        app.records.started("system.info");
        app.records.started("bbr.status");
        let ids = |app: &App| {
            let results = app.palette_results();
            (0..results.count())
                .map(|n| results.hit(n).unwrap().spec.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&app), ["bbr.status", "system.info", "tcp.tune"]);
        assert_eq!(highlighted(&app), Some("bbr.status"), "the newest first");
        app.key(key(KeyCode::Down)).unwrap();
        let effect = app.key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(effect, Effect::Refresh) && app.overview);
        // A query searches every Action instead.
        app.overview = false;
        app.key(key(KeyCode::Char('/'))).unwrap();
        app.key(key(KeyCode::Char('d'))).unwrap();
        assert!(app.palette_results().count() > 3);
    }

    #[test]
    fn palette_mouse_highlights_rows_runs_from_exec_and_scrolls() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = palette_app(temp.path(), "bbr");
        let area = Rect::new(0, 0, 120, 36);
        let mouse = |kind, point: Position| MouseEvent {
            kind,
            column: point.x,
            row: point.y,
            modifiers: KeyModifiers::NONE,
        };
        let click = |point| mouse(MouseEventKind::Down(MouseButton::Left), point);
        let geometry =
            |app: &App| view::CommandPaletteGeometry::new(area, app, &app.palette_results());
        // The point in the middle of result row `index`.
        let row = |app: &App, index: usize| {
            let rows = geometry(app).rows;
            Position::new(rows.x + 4, rows.y + (index - geometry(app).offset) as u16)
        };
        let exec = |app: &App| {
            let geometry = geometry(app);
            let rows = geometry.rows;
            (rows.y..rows.bottom())
                .flat_map(|y| (rows.x..rows.right()).map(move |x| Position::new(x, y)))
                .find(|point| geometry.exec_at(*point))
                .expect("the highlighted row's button")
        };
        // Rows: <kernel>, kernel.install, kernel.remove, <network>, bbr.status, ...
        app.mouse(click(row(&app, 4)), area).unwrap();
        assert_eq!(highlighted(&app), Some("bbr.status"));
        assert_eq!(
            exec(&app).y,
            row(&app, 4).y,
            "the button follows the highlight"
        );
        // A category name, the query row and the screen behind change nothing.
        for point in [
            row(&app, 3),
            geometry(&app).input.as_position(),
            Position::new(1, 1),
        ] {
            assert!(matches!(
                app.mouse(click(point), area).unwrap(),
                Effect::None
            ));
            assert_eq!(highlighted(&app), Some("bbr.status"), "{point:?}");
        }
        assert!(app.command_palette.is_some());
        // Clicking another row's place for the button only highlights that row.
        let mut beside = exec(&app);
        beside.y = row(&app, 1).y;
        app.mouse(click(beside), area).unwrap();
        assert_eq!(highlighted(&app), Some("kernel.install"));
        assert!(app.command_palette.is_some());

        // The wheel over the palette moves the highlight, stopping at either end.
        let over = row(&app, 1);
        let wheel = |down| {
            mouse(
                if down {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                },
                over,
            )
        };
        app.mouse(wheel(true), area).unwrap();
        assert_eq!(highlighted(&app), Some("kernel.remove"));
        app.mouse(wheel(false), area).unwrap();
        app.mouse(wheel(false), area).unwrap();
        assert_eq!(highlighted(&app), Some("kernel.install"));
        let outside = mouse(MouseEventKind::ScrollDown, Position::new(1, 1));
        app.mouse(outside, area).unwrap();
        assert_eq!(highlighted(&app), Some("kernel.install"));

        // "执行 ↵" acts as Enter: busy keeps the palette open, idle runs the Action.
        app.mouse(click(row(&app, 4)), area).unwrap();
        app.worker = Some(thread::spawn(|| {}));
        app.mouse(click(exec(&app)), area).unwrap();
        let busy = app.ctx.i18n.error(&Error::new("error.busy", ""));
        assert_eq!(app.notice.take().as_deref(), Some(busy.as_str()));
        assert!(app.command_palette.is_some());
        app.worker.take().unwrap().join().unwrap();
        match app.mouse(click(exec(&app)), area).unwrap() {
            Effect::Run(Task::Single { spec, .. }) => assert_eq!(spec.id, "bbr.status"),
            other => panic!("expected bbr.status to run, got {other:?}"),
        }
        assert!(app.command_palette.is_none());
    }
}
