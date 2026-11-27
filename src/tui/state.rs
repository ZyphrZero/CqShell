//! Core TUI application state machine: navigation, overview quick cards, action lists,
//! optimization checklist, forms/pickers, confirmations, batch progress, session records,
//! theme drawer, and frame clock.

use super::{
    fx,
    input::InputBuffer,
    palette, preview, related, render, sections, selection,
    session::SessionRecords,
    theme_drawer::{DrawerTab, STYLE_CHOICES, ThemeDrawer},
};
use crate::{
    actions::{self, Kind, Listing, Parameters, Source, Spec},
    config::Preferences,
    dashboard::{Reading, Snapshot},
    error::{Error, Result},
    ops::{Choice, clash::State as ClashState},
    runtime::{Context, Event, Outcome, Step, StepState},
    theme::{self, Background, Palette, StyleSelection, Theme, ThemeSource, ThemeState, builtin},
};
use ratatui::{
    Frame,
    layout::{Position, Rect},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) const GROUPS: &[&str] = &[
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

pub(super) const OPTIMIZATION_GROUP: usize = GROUPS.len() - 1;
pub(super) const DOCKER_GROUP: usize = 4;
pub(super) const CLASH_GROUP: usize = 10;

pub(super) const DEFAULT_CATEGORY_ORDER: &[&str] = &[
    "system", "kernel", "network", "ssh", "docker", "clash", "tools", "ai", "terminal", "tests",
    "app",
];

pub(super) const RECOMMENDED: &[&str] =
    &["system.update", "system.clean", "tcp.tune", "bbr.enable"];

pub(super) fn default_category_order() -> Vec<usize> {
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

pub(super) fn category_order_from_ids(ids: &[String]) -> Result<Vec<usize>> {
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

/// Top-level sidebar navigation destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavTarget {
    Overview,
    Optimization,
    Group(usize),
}

/// A row in a module or optimization list.
#[derive(Debug, Clone)]
pub(super) enum Row {
    Spacer,
    Section(&'static str, usize),
    Rule,
    Action(Spec),
    Execute,
}

impl Row {
    pub fn selectable(&self) -> bool {
        matches!(self, Self::Action(_) | Self::Execute)
    }
}

pub(super) fn row_index(rows: &[Row], position: usize) -> usize {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.selectable())
        .nth(position)
        .map(|(index, _)| index)
        .expect("position names a selectable row")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Idle,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn key(self) -> &'static str {
        match self {
            Self::Idle => "ui.ready",
            Self::Running => "ui.running",
            Self::Cancelling => "ui.cancelling",
            Self::Completed => "ui.done",
            Self::Failed => "ui.failed",
            Self::Cancelled => "ui.cancelled",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            Self::Idle => Tone::Muted,
            Self::Running => Tone::Info,
            Self::Cancelling | Self::Cancelled => Tone::Warning,
            Self::Completed => Tone::Success,
            Self::Failed => Tone::Danger,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Plain,
    Muted,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

pub(super) struct Toast {
    pub text: String,
    pub tone: Tone,
    pub shown: Instant,
}

#[derive(Debug, Clone)]
pub(super) struct LogLine {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

#[derive(Debug)]
pub(super) enum Task {
    Single {
        spec: Spec,
        params: Parameters,
        reviewed: bool,
    },
    Batch(Vec<(Spec, Parameters)>),
    DockerOp(crate::ops::docker::actions::DockerOp),
}

impl Task {
    pub fn jobs(&self) -> Vec<(&Spec, &Parameters)> {
        match self {
            Self::Single { spec, params, .. } => vec![(spec, params)],
            Self::Batch(jobs) => jobs.iter().map(|(spec, params)| (spec, params)).collect(),
            Self::DockerOp(_) => Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(super) enum Effect {
    None,
    Quit,
    Run(Task),
    Refresh,
    DockerRefresh,
    DockerExec(crate::ops::docker::actions::ExecSpec),
    ClashRefresh,
    ClashCmd(crate::ops::clash::monitor::ClashMonitorCmd),
    PublicIp,
    Copy(String),
    PlainView { title: String, content: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormMode {
    Run,
    Optimization,
}

pub(super) struct Form {
    pub spec: Spec,
    pub values: Vec<InputBuffer>,
    pub index: usize,
    pub mode: FormMode,
    pub error: Option<String>,
    pub options: Vec<Vec<Choice>>,
    pub picker: Option<Picker>,
}

impl Form {
    pub fn parameters(&self) -> Parameters {
        self.spec
            .fields
            .iter()
            .zip(&self.values)
            .map(|(field, value)| (field.name.to_owned(), value.text.clone()))
            .collect()
    }

    pub fn chip_field(&self, index: usize) -> bool {
        let field = &self.spec.fields[index];
        !field.choices.is_empty() && !field.allow_custom
    }

    pub fn preset_chips(&self, index: usize) -> bool {
        let field = &self.spec.fields[index];
        !field.choices.is_empty() && field.allow_custom
    }

    pub fn listed(&self, index: usize) -> bool {
        self.spec.fields[index].source.is_some()
    }

    pub fn choices(&self, index: usize) -> &[Choice] {
        match self.spec.fields[index].source {
            None => &[],
            Some(Source::Listing(_)) => &self.options[index],
            Some(Source::Children(parent)) => {
                let parent = self
                    .spec
                    .fields
                    .iter()
                    .position(|field| field.name == parent)
                    .expect("dependent field names an earlier field");
                let chosen = &self.values[parent].text;
                self.choices(parent)
                    .iter()
                    .find(|choice| choice.value == *chosen)
                    .map(|choice| choice.children.as_slice())
                    .unwrap_or_default()
            }
        }
    }

    pub fn attach(&mut self, options: Vec<Vec<Choice>>) {
        self.options = options;
        for index in 0..self.values.len() {
            if self.listed(index) {
                let value = initial_choice(self.choices(index), &self.values[index].text);
                self.values[index].replace(value);
            }
        }
    }

    pub fn choose(&mut self, index: usize, value: String) {
        self.values[index].replace(value);
        let name = self.spec.fields[index].name;
        for dependent in 0..self.values.len() {
            if self.spec.fields[dependent].source == Some(Source::Children(name)) {
                let value = initial_choice(self.choices(dependent), "");
                self.choose(dependent, value);
            }
        }
    }

    pub fn open_picker(&mut self, filter: String, with_form: bool) {
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

fn initial_choice(choices: &[Choice], held: &str) -> String {
    choices
        .iter()
        .find(|choice| choice.value == held)
        .or_else(|| choices.iter().find(|choice| choice.current))
        .or_else(|| choices.first())
        .map(|choice| choice.value.clone())
        .unwrap_or_default()
}

pub(super) struct Picker {
    pub filter: InputBuffer,
    pub cursor: usize,
    pub with_form: bool,
}

impl Picker {
    pub fn matches<'a>(&self, choices: &'a [Choice]) -> Vec<&'a Choice> {
        let filter = self.filter.text.to_lowercase();
        choices
            .iter()
            .filter(|choice| choice.value.to_lowercase().contains(&filter))
            .collect()
    }
}

pub(super) struct Loading {
    pub spec: Spec,
    pub mode: FormMode,
    pub receiver: Receiver<Result<Vec<Vec<Choice>>>>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
}

/// Transient in-memory modal for copying `cqs-socks5` credentials inside TUI only.
/// Passwords are always masked on screen (copy-only via OSC 52); cleared from memory when closed.
#[derive(Debug, Clone)]
pub(super) struct SocksSecretModal {
    pub creds: crate::ops::clash::socks::RevealedCredentials,
    pub scroll: u16,
}

/// Interactive terminal prompt preview studio state (`terminal.preview`).
#[derive(Debug, Clone)]
pub(super) struct PromptPreview {
    pub style_idx: usize,
    pub color_idx: usize,
    pub time: bool,
    pub git: bool,
    pub shell_idx: usize,
    pub show_toml: bool,
    pub toml_scroll: u16,
}

impl PromptPreview {
    pub fn from_options(opts: &crate::ops::terminal::Options) -> Self {
        use crate::ops::terminal::{COLORS, SHELLS, STYLES};
        Self {
            style_idx: STYLES.iter().position(|s| *s == opts.style).unwrap_or(2),
            color_idx: COLORS.iter().position(|c| *c == opts.color).unwrap_or(3),
            time: opts.time == "on",
            git: opts.git != "off",
            shell_idx: SHELLS.iter().position(|s| *s == opts.shell).unwrap_or(0),
            show_toml: false,
            toml_scroll: 0,
        }
    }

    pub fn style(&self) -> &'static str {
        crate::ops::terminal::STYLES
            .get(self.style_idx)
            .copied()
            .unwrap_or("box")
    }

    pub fn color(&self) -> &'static str {
        crate::ops::terminal::COLORS
            .get(self.color_idx)
            .copied()
            .unwrap_or("tokyo-night")
    }

    pub fn shell(&self) -> &'static str {
        crate::ops::terminal::SHELLS
            .get(self.shell_idx)
            .copied()
            .unwrap_or("bash")
    }

    pub fn options(&self) -> crate::ops::terminal::Options {
        crate::ops::terminal::Options {
            shell: self.shell().into(),
            style: self.style().into(),
            color: self.color().into(),
            time: if self.time { "on" } else { "off" }.into(),
            git: if self.git { "on" } else { "off" }.into(),
        }
    }

    pub fn parameters(&self) -> Parameters {
        let opts = self.options();
        Parameters::from([
            ("shell".into(), opts.shell),
            ("style".into(), opts.style),
            ("color".into(), opts.color),
            ("time".into(), opts.time),
            ("git".into(), opts.git),
        ])
    }
}

pub(super) struct Confirmation {
    pub text: String,
    pub danger: bool,
    pub reply: Sender<bool>,
    pub scroll: u16,
    pub accept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepView {
    Pending,
    Running(Instant),
    Done(Duration),
    Failed(Duration),
    Cancelled(Duration),
    Skipped,
}

impl StepView {
    fn finished(outcome: Outcome, elapsed: Duration) -> Self {
        match outcome {
            Outcome::Done => Self::Done(elapsed),
            Outcome::Failed => Self::Failed(elapsed),
            Outcome::Cancelled => Self::Cancelled(elapsed),
        }
    }
}

#[derive(Debug)]
pub(super) struct Progress {
    pub started: Instant,
    pub steps: Vec<StepView>,
}

impl Progress {
    pub fn new(total: usize, started: Instant) -> Self {
        Self {
            started,
            steps: vec![StepView::Pending; total],
        }
    }

    pub fn apply(&mut self, step: &Step, now: Instant) {
        if step.index < self.steps.len() {
            self.steps[step.index] = match step.state {
                StepState::Started => StepView::Running(now),
                StepState::Finished { outcome, elapsed } => StepView::finished(outcome, elapsed),
            };
        }
    }

    pub fn finish(&mut self, outcome: Outcome, now: Instant) {
        for step in &mut self.steps {
            *step = match *step {
                StepView::Pending => StepView::Skipped,
                StepView::Running(started) => {
                    StepView::finished(outcome, now.saturating_duration_since(started))
                }
                other => other,
            };
        }
    }

    pub fn current(&self) -> Option<usize> {
        self.steps
            .iter()
            .position(|step| matches!(step, StepView::Running(_)))
    }
}

#[derive(Debug, Clone)]
pub(super) enum QuickAction {
    Optimization,
    EnableBbr,
    PublicIp,
    CleanSystem,
    Mihomo,
    OpenPalette,
}

#[derive(Debug, Clone)]
pub(super) struct QuickCard {
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub badge_tone: Tone,
    pub action: QuickAction,
}

pub(super) fn step(current: Option<usize>, count: usize, forward: bool) -> usize {
    if count == 0 {
        return 0;
    }
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
        Listing::DelayGroups => "toast.no_delay_groups",
        Listing::BootKernels => "toast.no_boot_kernels",
        Listing::RemovableKernels => "toast.no_removable_kernels",
    }
}

#[derive(Debug, Clone)]
pub(super) struct SocksCacheEntry {
    pub key: Option<(u64, Option<std::time::SystemTime>, u64)>,
    pub config: Option<crate::ops::clash::socks::SocksConfig>,
}

pub struct App {
    pub(super) ctx: Context,
    /// Active navigation destination (starts on Overview Dashboard).
    pub(super) nav: NavTarget,
    /// User's custom or default order of the 11 reorderable groups.
    pub(super) category_order: Vec<usize>,
    /// Selected row position per group (counted among selectable rows only).
    pub(super) positions: [usize; GROUPS.len()],
    /// Active section sub-tab filter per group (`None` = All sections, `Some(i)` = section `i`).
    pub(super) section_tabs: [Option<usize>; GROUPS.len()],
    /// Selected quick card (0..4) on the Overview Dashboard.
    pub(super) quick_cursor: usize,
    /// Whether the user manually collapsed the sidebar on wide/medium viewports.
    pub(super) sidebar_collapsed: bool,
    /// Whether the compact-viewport sidebar drawer overlay is currently open.
    pub(super) sidebar_overlay: bool,
    /// Scroll offset for the right inspector panel (when focused or scrolled).
    pub(super) inspector_scroll: u16,
    pub(super) max_inspector_scroll: Cell<u16>,
    /// Checked action IDs in One-Click Optimization.
    pub(super) marked: BTreeSet<String>,
    /// Saved parameters per action ID for this session.
    pub(super) parameters: BTreeMap<String, Parameters>,
    pub(super) logs: VecDeque<LogLine>,
    pub(super) log_scroll: u16,
    pub(super) logs_open: bool,
    pub(super) log_modal: bool,
    pub(super) phase: Phase,
    pub(super) task_ids: Vec<String>,
    pub(super) form: Option<Form>,
    pub(super) prompt_preview: Option<PromptPreview>,
    pub(super) socks_secret: Option<SocksSecretModal>,
    pub(super) loading: Option<Loading>,
    pub(super) confirmation: Option<Confirmation>,
    pub(super) notice: Option<String>,
    pub(super) help: bool,
    pub(super) help_scroll: u16,
    pub(super) command_palette: Option<palette::CommandPalette>,
    pub(super) theme_drawer: Option<ThemeDrawer>,
    pub(super) worker: Option<JoinHandle<()>>,
    pub(super) exit_requested: bool,
    pub(super) receiver: Option<Receiver<Event>>,
    pub snapshot: Snapshot,
    pub(super) platform_error: Option<Error>,
    pub(super) root: bool,
    pub(super) completed_since_refresh: bool,
    pub(super) viewport: Rect,
    pub(super) now: Instant,
    pub(super) started: Instant,
    pub(super) toast: Option<Toast>,
    pub(super) dialog_opened: Option<Instant>,
    pub(super) dialog_kind: u8,
    pub(super) view_changed: Instant,
    pub(super) hover: Option<Position>,
    pub(super) last_click: Option<(Instant, Position)>,
    pub(super) gauges: [Option<fx::Tween>; 4],
    pub(super) refreshing: Option<Instant>,
    pub(super) theme: ThemeState,
    pub(super) palette_morph: Option<PaletteMorph>,
    pub(super) records: SessionRecords,
    pub(super) single_task: Option<(&'static str, Instant)>,
    pub(super) progress: Option<Progress>,
    /// Active border-free in-TUI mouse drag selection (clamped to a single content pane).
    pub(super) drag_select: Option<selection::DragSelection>,
    /// Text panes rendered in the most recent frame for border-free drag extraction.
    pub(super) selectable_panes: RefCell<Vec<selection::SelectablePane>>,
    /// Rendered rect of the Inspector `$ cqs ...` command strip, updated each frame for click-to-copy.
    pub(super) last_cmd_rect: Cell<Option<Rect>>,
    /// Dedicated Docker TUI management workspace state.
    pub(super) docker: super::docker::state::DockerState,
    /// Dedicated Mihomo TUI management workspace state.
    pub(super) clash: super::clash::state::ClashState,
    /// Optional title key when a `Task::DockerOp` is running.
    pub(super) docker_task_title: Option<&'static str>,
    /// Stat-fingerprinted cache of `cqs-socks5` config so rendering never re-parses `mixin.yaml`
    /// from disk every frame.
    pub(super) socks_cache: RefCell<Option<SocksCacheEntry>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PaletteMorph {
    pub from: Palette,
    pub to: Palette,
    pub started: Instant,
}

fn initial_theme() -> ThemeState {
    let background = Background::Unavailable("background detection not run".into());
    ThemeState {
        active: theme::resolve(&builtin::auto(), StyleSelection::Auto, &background, false),
        selected_id: "auto".into(),
        style_selection: StyleSelection::Auto,
        source: ThemeSource::Default,
        style_source: ThemeSource::Default,
        background,
        transparent_bg: false,
        reduced_motion: false,
        error: None,
    }
}

fn default_section_tabs() -> [Option<usize>; GROUPS.len()] {
    let mut tabs = [None; GROUPS.len()];
    for (idx, group) in GROUPS.iter().enumerate() {
        if idx == OPTIMIZATION_GROUP {
            continue;
        }
        if let Some(secs) = sections::of(group) {
            let total: usize = secs.iter().map(|(_, ids)| ids.len()).sum();
            // Long multi-section modules (such as Mihomo's 37 actions across 8 sections)
            // open on their first section tab instead of an overwhelming flat scroll wall.
            if secs.len() >= 2 && total > 15 {
                tabs[idx] = Some(0);
            }
        }
    }
    tabs
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
            nav: NavTarget::Overview,
            category_order: default_category_order(),
            positions: [0; GROUPS.len()],
            section_tabs: default_section_tabs(),
            quick_cursor: 0,
            sidebar_collapsed: false,
            sidebar_overlay: false,
            inspector_scroll: 0,
            max_inspector_scroll: Cell::new(0),
            marked: BTreeSet::new(),
            parameters: BTreeMap::new(),
            logs: VecDeque::new(),
            log_scroll: 0,
            logs_open: false,
            log_modal: false,
            phase: Phase::Idle,
            task_ids: Vec::new(),
            form: None,
            prompt_preview: None,
            socks_secret: None,
            loading: None,
            confirmation: None,
            notice: None,
            help: false,
            help_scroll: 0,
            command_palette: None,
            theme_drawer: None,
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
            dialog_opened: None,
            dialog_kind: 0,
            view_changed: now - fx::VIEW_FADE,
            hover: None,
            last_click: None,
            gauges: [None; 4],
            refreshing: None,
            theme: initial_theme(),
            palette_morph: None,
            records: SessionRecords::default(),
            single_task: None,
            progress: None,
            drag_select: None,
            selectable_panes: RefCell::new(Vec::new()),
            last_cmd_rect: Cell::new(None),
            docker: super::docker::state::DockerState::default(),
            clash: super::clash::state::ClashState::default(),
            docker_task_title: None,
            socks_cache: RefCell::new(None),
        }
    }

    pub fn configure_theme(
        &mut self,
        flag: Option<&str>,
        environment: Option<&str>,
        style_flag: Option<&str>,
        style_env: Option<&str>,
        background: Background,
        preferences: &Preferences,
    ) {
        let (id, source) =
            theme::resolve::resolve_id(flag, environment, preferences.theme.as_deref());
        let builtins = builtin::all();
        let customs = theme::file::load_dir(&self.ctx.paths.themes);
        let (definition, theme_error) = theme::resolve::select(&id, source, &builtins, &customs);
        let (style_selection, style_source, style_error) =
            theme::resolve::select_style(style_flag, style_env, preferences.style.as_deref());
        let error = match (theme_error, style_error) {
            (Some(a), Some(b)) => Some(format!("{a}; {b}")),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let transparent_bg = preferences.transparent_bg.unwrap_or(false);
        let reduced_motion = preferences.reduced_motion.unwrap_or(false);
        self.sidebar_collapsed = preferences.sidebar_collapsed.unwrap_or(false);
        self.clash.hide_ip = preferences.clash_hide_ip.unwrap_or(false);
        self.clash.service_order = preferences.clash_service_order.clone();
        self.clash.disabled_services = preferences.clash_disabled_services.clone();
        let active = theme::resolve(&definition, style_selection, &background, transparent_bg);
        if let Some(detail) = &error {
            let message = self
                .ctx
                .i18n
                .format("toast.theme_error", &[("detail", detail)]);
            self.notify(message, Tone::Warning);
        }
        self.palette_morph = None;
        self.theme = ThemeState {
            active,
            selected_id: definition.id,
            style_selection,
            source,
            style_source,
            background,
            transparent_bg,
            reduced_motion,
            error,
        };
    }

    /// Preview a specific built-in theme (using its default UI style) for visual QA exports.
    pub fn preview_theme(&mut self, id: &str, background: Background) -> Result<()> {
        self.preview_theme_with_style(id, StyleSelection::Auto, background)
    }

    /// Preview a specific built-in palette paired with an explicit [`StyleSelection`].
    pub fn preview_theme_with_style(
        &mut self,
        id: &str,
        style_selection: StyleSelection,
        background: Background,
    ) -> Result<()> {
        let builtins = builtin::all();
        let definition = builtins
            .into_iter()
            .find(|def| def.id == id)
            .ok_or_else(|| Error::new("error.config", format!("unknown theme: {id}")))?;
        self.palette_morph = None;
        self.theme.active = theme::resolve(
            &definition,
            style_selection,
            &background,
            self.theme.transparent_bg,
        );
        self.theme.selected_id = definition.id;
        self.theme.style_selection = style_selection;
        self.theme.background = background;
        Ok(())
    }

    pub fn draw(&self, frame: &mut Frame) {
        render::draw(self, frame);
    }

    pub(super) fn set_category_order(&mut self, order: Vec<usize>) {
        self.category_order = order;
    }

    pub(super) fn reduced_motion(&self) -> bool {
        self.theme.reduced_motion || self.theme.active.flags.ascii
    }

    pub(super) fn spin(&self) -> &'static str {
        fx::spinner(
            self.now.saturating_duration_since(self.started),
            self.theme.active.flags.ascii,
        )
    }

    pub(super) fn nav_order(&self) -> Vec<NavTarget> {
        let mut order = Vec::with_capacity(2 + self.category_order.len());
        order.push(NavTarget::Overview);
        order.push(NavTarget::Optimization);
        order.extend(self.category_order.iter().copied().map(NavTarget::Group));
        order
    }

    pub(super) fn goto_nav(&mut self, target: NavTarget) {
        if self.nav != target {
            self.nav = target;
            self.inspector_scroll = 0;
            self.max_inspector_scroll.set(0);
            self.docker.logs.searching = false;
            self.docker.filtering = false;
            self.clash.filtering = false;
            self.view_changed = self.now;
            if target == NavTarget::Group(DOCKER_GROUP) {
                self.docker.active_ever = true;
            }
            if target == NavTarget::Group(CLASH_GROUP) {
                self.clash.active_ever = true;
            }
        }
        self.sidebar_overlay = false;
    }

    pub(super) fn is_docker_view(&self) -> bool {
        self.nav == NavTarget::Group(DOCKER_GROUP)
    }

    pub(super) fn is_clash_view(&self) -> bool {
        self.nav == NavTarget::Group(CLASH_GROUP)
    }

    pub(super) fn open_clash(
        &mut self,
        tab: crate::ops::clash::model::ClashTab,
        fullscreen: bool,
        standalone: bool,
    ) {
        self.clash.active_ever = true;
        self.clash.set_tab(tab);
        self.clash.fullscreen = fullscreen;
        self.clash.launched_standalone = standalone;
        self.goto_nav(NavTarget::Group(CLASH_GROUP));
    }

    pub(super) fn open_docker(
        &mut self,
        tab: crate::ops::docker::model::DockerTab,
        fullscreen: bool,
        standalone: bool,
    ) {
        self.docker.active_ever = true;
        self.docker.set_tab(tab);
        self.docker.fullscreen = fullscreen;
        self.docker.focus_inspector = false;
        self.docker.launched_standalone = standalone;
        self.goto_nav(NavTarget::Group(DOCKER_GROUP));
    }

    pub(super) fn cycle_nav(&mut self, forward: bool) {
        let order = self.nav_order();
        let current = order.iter().position(|item| *item == self.nav);
        let next = order[step(current, order.len(), forward)];
        self.goto_nav(next);
    }

    pub(super) fn active_group(&self) -> Option<usize> {
        match self.nav {
            NavTarget::Overview => None,
            NavTarget::Optimization => Some(OPTIMIZATION_GROUP),
            NavTarget::Group(group) => Some(group),
        }
    }

    pub(super) fn optimization(&self) -> bool {
        self.nav == NavTarget::Optimization
    }

    pub(super) fn group_sections(
        &self,
        group: usize,
    ) -> Option<&'static [(&'static str, &'static [&'static str])]> {
        if group == OPTIMIZATION_GROUP {
            return None;
        }
        sections::of(GROUPS[group]).filter(|secs| secs.len() >= 2)
    }

    pub(super) fn section_tab(&self, group: usize) -> Option<usize> {
        let secs = self.group_sections(group)?;
        self.section_tabs[group].filter(|&idx| idx < secs.len())
    }

    pub(super) fn set_section_tab(&mut self, group: usize, tab: Option<usize>) {
        let clamped = self
            .group_sections(group)
            .and_then(|secs| tab.filter(|&idx| idx < secs.len()));
        if self.section_tabs[group] != clamped {
            self.section_tabs[group] = clamped;
            self.positions[group] = 0;
            self.inspector_scroll = 0;
        }
    }

    pub(super) fn cycle_section_tab(&mut self, group: usize, forward: bool) -> bool {
        let Some(secs) = self.group_sections(group) else {
            return false;
        };
        // Tab sequence: 0 => None (All), 1..=N => Some(idx - 1)
        let total = secs.len() + 1;
        let cur = match self.section_tab(group) {
            None => 0,
            Some(idx) => idx + 1,
        };
        let next = step(Some(cur), total, forward);
        let tab = if next == 0 { None } else { Some(next - 1) };
        self.set_section_tab(group, tab);
        true
    }

    /// Jump across section boundaries via PageUp / PageDown when logs are closed.
    pub(super) fn jump_section(&mut self, group: usize, forward: bool) {
        if let Some(secs) = self.group_sections(group) {
            if self.section_tab(group).is_some() {
                self.cycle_section_tab(group, forward);
                return;
            }
            // In All view, compute the selectable start index of each section.
            let mut starts = Vec::with_capacity(secs.len());
            let mut offset = 0usize;
            for (_, ids) in secs {
                starts.push(offset);
                offset += ids.len();
            }
            let cur = self.selected_in(group);
            let target = if forward {
                starts
                    .iter()
                    .copied()
                    .find(|&s| s > cur)
                    .unwrap_or(starts[0])
            } else {
                starts
                    .iter()
                    .copied()
                    .rev()
                    .find(|&s| s < cur)
                    .unwrap_or(*starts.last().unwrap_or(&0))
            };
            self.positions[group] = target;
            self.inspector_scroll = 0;
            return;
        }
        // Ungrouped or Optimization list: jump half a page.
        let count = self.selectable_count(group);
        if count == 0 {
            return;
        }
        let cur = self.selected_in(group);
        let jump = 6usize;
        self.positions[group] = if forward {
            (cur + jump).min(count - 1)
        } else {
            cur.saturating_sub(jump)
        };
        self.inspector_scroll = 0;
    }

    pub(super) fn rows(&self, group: usize) -> Vec<Row> {
        let specs = actions::specs_slice();
        let action =
            |id: &str| Row::Action(actions::find_ref(id).expect("static action ID").clone());
        if group == OPTIMIZATION_GROUP {
            return actions::OPTIMIZATION
                .iter()
                .map(|id| action(id))
                .chain([Row::Rule, Row::Execute])
                .collect();
        }
        if let Some(secs) = self.group_sections(group) {
            if let Some(sec_idx) = self.section_tab(group) {
                return secs[sec_idx].1.iter().map(|id| action(id)).collect();
            }
            let mut out = Vec::new();
            for (idx, (sec_id, ids)) in secs.iter().enumerate() {
                if idx > 0 {
                    out.push(Row::Spacer);
                }
                out.push(Row::Section(sec_id, ids.len()));
                out.extend(ids.iter().map(|id| action(id)));
            }
            return out;
        }
        sections::blocks(GROUPS[group], specs)
            .into_iter()
            .flat_map(|(section, ids)| {
                let len = ids.len();
                section
                    .map(|sec| Row::Section(sec, len))
                    .into_iter()
                    .chain(ids.into_iter().map(&action))
            })
            .collect()
    }

    pub(super) fn selectable_count(&self, group: usize) -> usize {
        if group == OPTIMIZATION_GROUP {
            return actions::OPTIMIZATION.len() + 1; // +1 for Row::Execute
        }
        if let Some(secs) = self.group_sections(group) {
            return match self.section_tab(group) {
                Some(sec_idx) => secs.get(sec_idx).map_or(0, |(_, ids)| ids.len()),
                None => secs.iter().map(|(_, ids)| ids.len()).sum(),
            };
        }
        actions::specs_slice()
            .iter()
            .filter(|spec| spec.group == GROUPS[group])
            .count()
    }

    pub(super) fn selected_in(&self, group: usize) -> usize {
        let count = self.selectable_count(group);
        if count == 0 {
            0
        } else {
            self.positions[group].min(count - 1)
        }
    }

    pub(super) fn selected_row(&self) -> Option<Row> {
        let group = self.active_group()?;
        self.rows(group)
            .into_iter()
            .filter(Row::selectable)
            .nth(self.selected_in(group))
    }

    pub(super) fn move_row(&mut self, forward: bool) {
        let Some(group) = self.active_group() else {
            return;
        };
        let count = self.selectable_count(group);
        self.positions[group] = step(Some(self.selected_in(group)), count, forward);
        self.inspector_scroll = 0;
    }

    pub(super) fn jump_to_action(&mut self, spec: &Spec) {
        if let Some(group) = GROUPS.iter().position(|g| *g == spec.group) {
            self.goto_nav(NavTarget::Group(group));
            if let Some(secs) = self.group_sections(group)
                && let Some(active) = self.section_tab(group)
                && !secs[active].1.contains(&spec.id)
                && let Some(owner) = secs.iter().position(|(_, ids)| ids.contains(&spec.id))
            {
                self.section_tabs[group] = Some(owner);
            }
            let pos = self
                .rows(group)
                .into_iter()
                .filter(Row::selectable)
                .position(|row| matches!(row, Row::Action(item) if item.id == spec.id));
            if let Some(pos) = pos {
                self.positions[group] = pos;
            }
        }
    }

    pub(super) fn quick_cards(&self) -> [QuickCard; 4] {
        let i = &self.ctx.i18n;
        let opt_count = if self.marked.is_empty() {
            RECOMMENDED.len()
        } else {
            self.marked.len()
        };
        let card_opt = QuickCard {
            title: i.text("overview.card_opt_title").into(),
            subtitle: i.format(
                "overview.card_opt_desc",
                &[("count", &opt_count.to_string())],
            ),
            badge: format!("{opt_count}"),
            badge_tone: Tone::Accent,
            action: QuickAction::Optimization,
        };

        // Contextual recommendation card based on live system telemetry
        let local = &self.snapshot.local;
        let card_ctx = if matches!(local.bbr_available, Reading::Value(true))
            && matches!(&local.congestion, Reading::Value(algo) if algo != "bbr")
        {
            let algo = match &local.congestion {
                Reading::Value(v) => v.as_str(),
                _ => "cubic",
            };
            QuickCard {
                title: i.text("overview.card_bbr_title").into(),
                subtitle: i.format("overview.card_bbr_desc", &[("algo", algo)]),
                badge: i.text("status.disabled").into(),
                badge_tone: Tone::Warning,
                action: QuickAction::EnableBbr,
            }
        } else if self.snapshot.public.updated.is_none() {
            QuickCard {
                title: i.text("overview.card_public_title").into(),
                subtitle: i.text("overview.card_public_desc").into(),
                badge: "N".into(),
                badge_tone: Tone::Info,
                action: QuickAction::PublicIp,
            }
        } else {
            let pct = match &local.disk {
                Reading::Value(u) => format!("{:.0}%", u.percent()),
                _ => "--".into(),
            };
            QuickCard {
                title: i.text("overview.card_clean_title").into(),
                subtitle: i.format("overview.card_clean_desc", &[("pct", &pct)]),
                badge: pct,
                badge_tone: Tone::Success,
                action: QuickAction::CleanSystem,
            }
        };

        let (mihomo_badge, mihomo_tone) = match &local.mihomo_state {
            Reading::Value(ClashState::Running(_)) => {
                (i.text("dash.running").into(), Tone::Success)
            }
            Reading::Value(ClashState::Stopped) => (i.text("dash.stopped").into(), Tone::Warning),
            Reading::Value(ClashState::NotInstalled) => {
                (i.text("dash.not_installed").into(), Tone::Muted)
            }
            _ => (i.text("dash.unknown").into(), Tone::Muted),
        };
        let card_mihomo = QuickCard {
            title: i.text("overview.card_mihomo_title").into(),
            subtitle: i.text("overview.card_mihomo_desc").into(),
            badge: mihomo_badge,
            badge_tone: mihomo_tone,
            action: QuickAction::Mihomo,
        };

        let recent_sub = if self.records.recent.is_empty() {
            i.text("overview.card_recent_empty").into()
        } else {
            let names = self
                .records
                .recent
                .iter()
                .take(3)
                .map(|id| i.text(&format!("action.{id}")))
                .collect::<Vec<_>>()
                .join(" · ");
            i.format("overview.card_recent_hint", &[("items", &names)])
        };
        let card_recent = QuickCard {
            title: i.text("overview.card_recent_title").into(),
            subtitle: recent_sub,
            badge: "/".into(),
            badge_tone: Tone::Info,
            action: QuickAction::OpenPalette,
        };

        [card_opt, card_ctx, card_mihomo, card_recent]
    }

    pub(super) fn activate_quick_card(&mut self, index: usize) -> Result<Effect> {
        let cards = self.quick_cards();
        let Some(card) = cards.get(index) else {
            return Ok(Effect::None);
        };
        match card.action {
            QuickAction::Optimization => {
                self.goto_nav(NavTarget::Optimization);
                Ok(Effect::None)
            }
            QuickAction::EnableBbr => {
                let spec = actions::find("bbr.enable")?;
                self.jump_to_action(&spec);
                self.activate_action(spec)
            }
            QuickAction::PublicIp => Ok(Effect::PublicIp),
            QuickAction::CleanSystem => {
                let spec = actions::find("system.clean")?;
                self.jump_to_action(&spec);
                self.activate_action(spec)
            }
            QuickAction::Mihomo => {
                let clash = GROUPS
                    .iter()
                    .position(|g| *g == "clash")
                    .expect("clash group");
                self.goto_nav(NavTarget::Group(clash));
                Ok(Effect::None)
            }
            QuickAction::OpenPalette => {
                self.command_palette = Some(palette::CommandPalette::default());
                Ok(Effect::None)
            }
        }
    }

    pub(super) fn prompt_preview_max_scroll(&self, area: Rect) -> u16 {
        let Some(preview) = &self.prompt_preview else {
            return 0;
        };
        let geom = super::layout::PromptPreviewGeometry::compute_for(
            area,
            &self.ctx.i18n,
            preview.style_idx,
            preview.color_idx,
        );
        let body_h = geom.preview_box.height.saturating_sub(1) as usize;
        let line_count = crate::ops::terminal::render(&preview.options())
            .map(|t| t.lines().count())
            .unwrap_or(0);
        line_count.saturating_sub(body_h).min(u16::MAX as usize) as u16
    }

    pub(super) fn toggle_clash_hide_ip(&mut self) {
        self.clash.hide_ip = !self.clash.hide_ip;
        if let Ok(mut prefs) = Preferences::read(&self.ctx.paths.preferences, false) {
            prefs.clash_hide_ip = Some(self.clash.hide_ip);
            let _ = prefs.write(&self.ctx.paths.preferences);
        }
    }

    pub(super) fn persist_clash_service_prefs(&self) {
        if let Ok(mut prefs) = Preferences::read(&self.ctx.paths.preferences, false) {
            prefs.clash_service_order = self.clash.service_order.clone();
            prefs.clash_disabled_services = self.clash.disabled_services.clone();
            let _ = prefs.write(&self.ctx.paths.preferences);
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        if super::layout::Breakpoint::of(self.viewport.width).uses_overlay_sidebar()
            || (self.is_docker_view() && self.docker.fullscreen)
            || (self.is_clash_view() && self.clash.fullscreen)
        {
            self.sidebar_overlay = !self.sidebar_overlay;
        } else {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            if let Ok(mut prefs) = Preferences::read(&self.ctx.paths.preferences, false) {
                prefs.sidebar_collapsed = Some(self.sidebar_collapsed);
                let _ = prefs.write(&self.ctx.paths.preferences);
            }
        }
    }

    pub(super) fn reorder_category(&mut self, down: bool) {
        let NavTarget::Group(group) = self.nav else {
            return;
        };
        let Some(pos) = self.category_order.iter().position(|id| *id == group) else {
            return;
        };
        let target = if down {
            if pos + 1 >= self.category_order.len() {
                return;
            }
            pos + 1
        } else {
            if pos == 0 {
                return;
            }
            pos - 1
        };
        self.category_order.swap(pos, target);
        let ids = self
            .category_order
            .iter()
            .map(|idx| GROUPS[*idx].to_owned())
            .collect::<Vec<_>>();
        let result = (|| -> Result<()> {
            let mut prefs = Preferences::read(&self.ctx.paths.preferences, false)?;
            prefs.category_order = ids;
            prefs.write(&self.ctx.paths.preferences)
        })();
        if let Err(error) = result {
            self.category_order.swap(pos, target);
            self.fail_ui(error);
        }
    }

    fn apply_resolved_theme(&mut self, next: Theme, animate: bool) {
        let from = self.theme.active.palette;
        let to = next.palette;
        let can_morph = animate
            && !self.theme.reduced_motion
            && !next.flags.ascii
            && !self.theme.active.flags.ascii
            && from != to;
        self.theme.active = next;
        if can_morph {
            self.palette_morph = Some(PaletteMorph {
                from,
                to,
                started: self.now,
            });
            self.theme.active.palette = from;
        } else {
            self.palette_morph = None;
        }
    }

    pub(super) fn open_theme_drawer(&mut self) {
        if self.theme_drawer.is_some() {
            self.cancel_theme_drawer();
            return;
        }
        let prefs = Preferences::read(&self.ctx.paths.preferences, false).ok();
        let saved_id = prefs.as_ref().and_then(|p| p.theme.clone());
        let saved_style = prefs.as_ref().and_then(|p| p.style.clone());
        // Capture settled target palette if a morph was still running
        if let Some(morph) = self.palette_morph.take() {
            self.theme.active.palette = morph.to;
        }
        self.theme_drawer = Some(ThemeDrawer::open(
            &self.ctx.paths.themes,
            &self.theme.active,
            self.theme.style_selection,
            saved_id,
            saved_style,
            self.theme.transparent_bg,
            self.theme.reduced_motion,
            self.now,
        ));
    }

    pub(super) fn switch_theme_drawer_tab(&mut self, tab: DrawerTab) {
        if let Some(drawer) = &mut self.theme_drawer {
            drawer.tab = tab;
        }
    }

    pub(super) fn preview_theme_cursor(&mut self, cursor: usize) {
        let resolved = {
            let Some(drawer) = &mut self.theme_drawer else {
                return;
            };
            match drawer.tab {
                DrawerTab::Palette => {
                    drawer.cursor = cursor.min(drawer.entries.len().saturating_sub(1));
                }
                DrawerTab::Style => {
                    drawer.style_cursor = cursor.min(STYLE_CHOICES.len().saturating_sub(1));
                }
            }
            drawer.resolve_selected(&self.theme.background, self.theme.transparent_bg)
        };
        if let Some(resolved) = resolved {
            self.apply_resolved_theme(resolved, true);
        }
    }

    pub(super) fn toggle_transparent_bg(&mut self, persist: bool) {
        self.theme.transparent_bg = !self.theme.transparent_bg;
        self.re_resolve_active_theme();
        if persist {
            if let Ok(mut prefs) = Preferences::read(&self.ctx.paths.preferences, false) {
                prefs.transparent_bg = Some(self.theme.transparent_bg);
                let _ = prefs.write(&self.ctx.paths.preferences);
            }
            let key = if self.theme.transparent_bg {
                "toast.transparent_on"
            } else {
                "toast.transparent_off"
            };
            self.notify(self.ctx.i18n.text(key).into(), Tone::Info);
        }
    }

    pub(super) fn toggle_reduced_motion(&mut self, persist: bool) {
        self.theme.reduced_motion = !self.theme.reduced_motion;
        if self.theme.reduced_motion
            && let Some(morph) = self.palette_morph.take()
        {
            self.theme.active.palette = morph.to;
        }
        if persist {
            if let Ok(mut prefs) = Preferences::read(&self.ctx.paths.preferences, false) {
                prefs.reduced_motion = Some(self.theme.reduced_motion);
                let _ = prefs.write(&self.ctx.paths.preferences);
            }
            let key = if self.theme.reduced_motion {
                "toast.motion_reduced"
            } else {
                "toast.motion_full"
            };
            self.notify(self.ctx.i18n.text(key).into(), Tone::Info);
        }
    }

    fn re_resolve_active_theme(&mut self) {
        if let Some(drawer) = &self.theme_drawer
            && let Some(resolved) =
                drawer.resolve_selected(&self.theme.background, self.theme.transparent_bg)
        {
            self.apply_resolved_theme(resolved, false);
            return;
        }
        let builtins = builtin::all();
        let customs = theme::file::load_dir(&self.ctx.paths.themes);
        let (def, _) = theme::resolve::select(
            &self.theme.active.id,
            self.theme.source,
            &builtins,
            &customs,
        );
        let resolved = theme::resolve(
            &def,
            self.theme.style_selection,
            &self.theme.background,
            self.theme.transparent_bg,
        );
        self.apply_resolved_theme(resolved, false);
    }

    pub(super) fn cancel_theme_drawer(&mut self) {
        if let Some(drawer) = self.theme_drawer.take() {
            self.theme.style_selection = drawer.previous_style_selection;
            self.theme.transparent_bg = drawer.previous_transparent;
            self.theme.reduced_motion = drawer.previous_motion;
            self.apply_resolved_theme(drawer.previous_theme, true);
        }
    }

    pub(super) fn save_theme_drawer(&mut self) {
        let Some(drawer) = self.theme_drawer.clone() else {
            return;
        };
        let Some(resolved) =
            drawer.resolve_selected(&self.theme.background, self.theme.transparent_bg)
        else {
            return;
        };
        let id = resolved.id.clone();
        let style_sel = drawer.selected_style();
        let transparent_bg = self.theme.transparent_bg;
        let reduced_motion = self.theme.reduced_motion;
        let result = (|| -> Result<()> {
            let mut prefs = Preferences::read(&self.ctx.paths.preferences, false)?;
            prefs.theme = Some(id.clone());
            prefs.style = match style_sel {
                StyleSelection::Auto => None,
                StyleSelection::Fixed(style) => Some(style.id().into()),
            };
            prefs.transparent_bg = Some(transparent_bg);
            prefs.reduced_motion = Some(reduced_motion);
            prefs.write(&self.ctx.paths.preferences)
        })();
        if let Err(error) = result {
            self.theme.style_selection = drawer.previous_style_selection;
            self.theme.transparent_bg = drawer.previous_transparent;
            self.theme.reduced_motion = drawer.previous_motion;
            self.apply_resolved_theme(drawer.previous_theme, false);
            self.fail_ui(error);
        } else {
            let pal_display = drawer.selected().display_name(&self.ctx.i18n);
            let style_display = self.ctx.i18n.text(resolved.style.name_key()).to_owned();
            self.palette_morph = None;
            self.theme.active = resolved;
            self.theme.selected_id = id;
            self.theme.style_selection = style_sel;
            self.theme_drawer = None;
            let msg = self.ctx.i18n.format(
                "toast.theme_saved",
                &[("theme", &format!("{pal_display} · {style_display}"))],
            );
            self.notify(msg, Tone::Success);
        }
    }

    pub(super) fn available(&self, spec: &Spec) -> Result<()> {
        if matches!(
            spec.kind,
            Kind::Info
                | Kind::PublicIp
                | Kind::GithubProxy
                | Kind::TerminalPreview
                | Kind::SelfUpdateCheck
        ) {
            return Ok(());
        }
        if spec.kind == Kind::SelfUpdate {
            if !cfg!(target_os = "linux") {
                return Err(Error::new(
                    "error.self_update_platform",
                    std::env::consts::OS,
                ));
            }
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

    pub(super) fn notify(&mut self, text: String, tone: Tone) {
        self.toast = Some(Toast {
            text,
            tone,
            shown: Instant::now(),
        });
    }

    pub(in crate::tui) fn toast(&mut self, text: String, tone: Tone) {
        self.notify(text, tone);
    }

    pub(super) fn log(&mut self, text: impl AsRef<str>) {
        self.log_styled(text, Tone::Plain, false);
    }

    pub(super) fn log_tone(&mut self, text: impl AsRef<str>, tone: Tone) {
        self.log_styled(text, tone, false);
    }

    pub(super) fn log_styled(&mut self, text: impl AsRef<str>, tone: Tone, bold: bool) {
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

    pub(super) fn fail_ui(&mut self, error: Error) {
        let text = self.ctx.i18n.error(&error);
        if let Some(form) = &mut self.form {
            form.error = Some(text);
        } else {
            self.notice = Some(text);
        }
    }

    pub(super) fn overlay_kind(&self) -> u8 {
        if self.confirmation.is_some() {
            1
        } else if self.loading.is_some() {
            2
        } else if self.socks_secret.is_some() {
            12
        } else if self.form.as_ref().is_some_and(|form| form.picker.is_some()) {
            3
        } else if self.form.is_some() {
            4
        } else if self.prompt_preview.is_some() {
            10
        } else if self.docker.modal.is_some() {
            11
        } else if self.clash.modal.is_some() {
            13
        } else if self.notice.is_some() {
            5
        } else if self.help {
            6
        } else if self.log_modal {
            7
        } else if self.command_palette.is_some() {
            8
        } else if self.sidebar_overlay {
            9
        } else {
            0
        }
    }

    pub(super) fn dialog_fading(&self) -> bool {
        !self.reduced_motion()
            && self
                .dialog_opened
                .is_some_and(|opened| self.now.saturating_duration_since(opened) < fx::DIALOG_FADE)
    }

    pub(super) fn view_fading(&self) -> bool {
        !self.reduced_motion()
            && self.now.saturating_duration_since(self.view_changed) < fx::VIEW_FADE
    }

    pub(super) fn drawer_sliding(&self) -> bool {
        !self.reduced_motion()
            && self
                .theme_drawer
                .as_ref()
                .is_some_and(|d| self.now.saturating_duration_since(d.opened) < fx::DRAWER_SLIDE)
    }

    pub(super) fn tick(&mut self, now: Instant) {
        self.now = now;
        let kind = self.overlay_kind();
        if kind != self.dialog_kind {
            self.dialog_kind = kind;
            self.dialog_opened = (kind != 0).then_some(now);
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| now.saturating_duration_since(toast.shown) >= fx::TOAST_LIFETIME)
        {
            self.toast = None;
        }
        if let (Some(requested), Some(updated)) = (self.refreshing, self.snapshot.local.updated)
            && updated >= requested
        {
            self.refreshing = None;
        }
        let reduced = self.reduced_motion();
        if let Some(morph) = self.palette_morph {
            let age = now.saturating_duration_since(morph.started);
            let factor = fx::palette_morph_factor(age, reduced);
            self.theme.active.palette = fx::lerp_palette(morph.from, morph.to, factor);
            if factor >= 0.995 {
                self.theme.active.palette = morph.to;
                self.palette_morph = None;
            }
        }
        for (slot, target) in self.gauges.iter_mut().zip(gauge_targets(&self.snapshot)) {
            match (slot.as_mut(), target) {
                (_, None) => *slot = None,
                (None, Some(to)) => *slot = Some(fx::Tween::new(to, now)),
                (Some(tween), Some(to)) => tween.retarget(to, now, reduced),
            }
        }
    }

    pub(super) fn has_ambient_motion(&self) -> bool {
        !self.reduced_motion()
            && fx::style_has_ambient_motion(self.theme.active.style)
            && self.theme.active.palette.accent2.rgb().is_some()
    }

    pub(super) fn frame_interval(&self) -> Duration {
        let reduced = self.reduced_motion();
        let sheen_active = !reduced
            && self
                .snapshot
                .local
                .updated
                .is_some_and(|at| self.now.saturating_duration_since(at) < fx::GAUGE_SHEEN);
        let moving = self
            .gauges
            .iter()
            .flatten()
            .any(|tween| tween.moving(self.now, reduced))
            || self.palette_morph.is_some()
            || self.drawer_sliding()
            || sheen_active
            || self.toast.as_ref().is_some_and(|toast| {
                fx::toast_fading(self.now.saturating_duration_since(toast.shown))
            })
            || self.dialog_fading()
            || self.view_fading();

        if moving || self.loading.is_some() {
            fx::ANIMATION_FRAME
        } else if self.worker.is_some()
            || self.refreshing.is_some()
            || snapshot_pending(&self.snapshot)
            || (self.is_docker_view() && self.docker.desired_log_target().is_some())
            || (self.is_clash_view() && self.clash.is_animating())
        {
            fx::SPINNER_FRAME
        } else if self.has_ambient_motion() {
            fx::AMBIENT_FRAME
        } else {
            fx::IDLE_FRAME
        }
    }

    /// Whether the current frame has active motion, spinners, or fading toasts that require
    /// continuous redraws even without user input or new telemetry snapshots.
    pub(super) fn needs_continuous_redraw(&self) -> bool {
        let reduced = self.reduced_motion();
        let sheen_active = !reduced
            && self
                .snapshot
                .local
                .updated
                .is_some_and(|at| self.now.saturating_duration_since(at) < fx::GAUGE_SHEEN);
        let moving = self
            .gauges
            .iter()
            .flatten()
            .any(|tween| tween.moving(self.now, reduced))
            || self.palette_morph.is_some()
            || self.drawer_sliding()
            || sheen_active
            || self.toast.as_ref().is_some_and(|toast| {
                let age = self.now.saturating_duration_since(toast.shown);
                fx::toast_fading(age) || age >= fx::TOAST_LIFETIME
            })
            || self.dialog_fading()
            || self.view_fading();
        moving
            || self.loading.is_some()
            || self.worker.is_some()
            || self.refreshing.is_some()
            || snapshot_pending(&self.snapshot)
            || (self.is_docker_view() && self.docker.desired_log_target().is_some())
            || (self.is_clash_view() && self.clash.is_animating())
            || self.has_ambient_motion()
    }

    pub(super) fn task_title(&self) -> String {
        if let Some(key) = self.docker_task_title {
            return self.ctx.i18n.text(key).into();
        }
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

    pub(super) fn active_step_summary(&self) -> Option<String> {
        let progress = self.progress.as_ref()?;
        let current = progress.current()?;
        let action_id = self.task_ids.get(current)?;
        let elapsed = format_mm_ss(self.now.saturating_duration_since(progress.started));
        Some(format!(
            "{}/{} · {} · {elapsed}",
            current + 1,
            progress.steps.len(),
            self.ctx.i18n.text(&format!("action.{action_id}"))
        ))
    }

    pub(super) fn single_elapsed(&self) -> Option<Duration> {
        self.single_task
            .map(|(_, started)| self.now.saturating_duration_since(started))
    }

    pub(super) fn finish(&mut self, result: Result<()>) {
        self.confirmation = None;
        let outcome = task_outcome(&result);
        if let Some(progress) = &mut self.progress {
            progress.finish(outcome, Instant::now());
        }
        self.phase = match outcome {
            Outcome::Done => Phase::Completed,
            Outcome::Failed => Phase::Failed,
            Outcome::Cancelled => Phase::Cancelled,
        };
        if let Some((id, started)) = self.single_task.take() {
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(started);
            self.records.finished(id, outcome, elapsed, now);
        }
        if result.is_ok()
            && self.task_ids.iter().any(|id| id == "github.proxy")
            && let Ok(prefs) = Preferences::read(&self.ctx.paths.preferences, false)
            && let Ok(proxy) = crate::github_proxy::from_preferences(&prefs)
        {
            self.ctx.github_proxy = proxy;
        }
        let tone = self.phase.tone();
        let message = match result {
            Err(error) => self.ctx.i18n.error(&error),
            Ok(()) => self.ctx.i18n.text("ui.done").to_owned(),
        };
        self.log_tone(message, tone);
        let summary = format!(
            "{} · {}",
            self.ctx.i18n.text(self.phase.key()),
            self.task_title()
        );
        self.notify(summary, tone);
        self.socks_cache.borrow_mut().take();
        crate::ops::clash::runtime_cache::invalidate_all();
        self.completed_since_refresh = true;
        self.ctx.cancel.store(false, Ordering::SeqCst);
    }

    pub(super) fn receive(&mut self) -> bool {
        let mut changed = false;
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
        if !events.is_empty() {
            changed = true;
        }
        for event in events {
            match event {
                Event::Log(text) => self.log(text),
                Event::Confirm {
                    text,
                    danger,
                    reply,
                } => {
                    self.dialog_opened = Some(Instant::now());
                    self.confirmation = Some(Confirmation {
                        text,
                        danger,
                        reply,
                        scroll: 0,
                        accept: false,
                    });
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
            changed = true;
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            self.finish(Err(Error::new(
                "error.io",
                "worker exited without a result",
            )));
            self.receiver = None;
        }
        changed |= self.receive_choices();
        changed
    }

    pub(super) fn record_start(&mut self, task: &Task) {
        self.docker_task_title = match task {
            Task::DockerOp(op) => Some(op.title_key()),
            _ => None,
        };
        self.task_ids = task
            .jobs()
            .iter()
            .map(|(spec, _)| spec.id.to_owned())
            .collect();
        self.progress = match task {
            Task::Batch(jobs) => Some(Progress::new(jobs.len(), Instant::now())),
            Task::Single { .. } | Task::DockerOp(_) => None,
        };
        self.single_task = match task {
            Task::Single { spec, .. } => {
                self.records.started(spec.id);
                Some((spec.id, Instant::now()))
            }
            Task::DockerOp(op) => Some((op.title_key(), Instant::now())),
            Task::Batch(_) => None,
        };
    }

    pub(super) fn record_step(&mut self, step: &Step) {
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
                self.records.finished(step.id, outcome, elapsed, now);
            }
        }
        if let Some(progress) = &mut self.progress {
            progress.apply(step, now);
        }
    }

    pub(super) fn record_report(&mut self, report: &actions::BatchReport) {
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

    pub(super) fn request_cancel(&mut self, exit: bool) {
        self.exit_requested |= exit;
        if self.worker.is_some() {
            self.phase = Phase::Cancelling;
            self.ctx.cancel.store(true, Ordering::SeqCst);
            if let Some(confirm) = self.confirmation.take() {
                let _ = confirm.reply.send(false);
            }
        }
    }

    pub(super) fn make_form(&self, spec: Spec, mode: FormMode) -> Result<Form> {
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
        if spec.id == "docker.mirror" && !self.parameters.contains_key(spec.id) {
            let existing = self
                .docker
                .snapshot
                .engine
                .as_ref()
                .map(|e| e.daemon_mirrors.clone())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| crate::ops::docker::configured_mirrors(&self.ctx.paths.docker));
            if !existing.is_empty() {
                values.insert("mirror".into(), existing.join(", "));
            }
        }
        if spec.id == "clash.socks.configure"
            && !self.parameters.contains_key(spec.id)
            && let Ok(cfg) = crate::ops::clash::socks::inspect(&self.ctx)
            && cfg.enabled
        {
            values = BTreeMap::from([
                ("socks_enabled".into(), "on".into()),
                ("socks_port".into(), cfg.port.to_string()),
                ("socks_listen".into(), cfg.listen),
                (
                    "socks_udp".into(),
                    if cfg.udp { "on" } else { "off" }.into(),
                ),
                ("socks_out".into(), cfg.outbound),
                (
                    "socks_auth".into(),
                    if cfg.has_auth { "keep" } else { "off" }.into(),
                ),
                (
                    "socks_user".into(),
                    cfg.username.unwrap_or_else(|| "cqs".into()),
                ),
            ]);
        }
        if spec.id == "github.proxy" {
            let saved_url = self.ctx.github_proxy.clone().unwrap_or_else(|| {
                Preferences::read(&self.ctx.paths.preferences, false)
                    .ok()
                    .map(|prefs| crate::github_proxy::configured_url(&prefs))
                    .unwrap_or_else(|| crate::github_proxy::DEFAULT_URL.to_owned())
            });
            values = BTreeMap::from([
                (
                    "gh_proxy_state".into(),
                    if self.ctx.github_proxy.is_some() {
                        "on".into()
                    } else {
                        "off".into()
                    },
                ),
                ("gh_proxy_url".into(), saved_url),
            ]);
        } else if let Some(saved) = self.parameters.get(spec.id) {
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

    pub(super) fn open_form(&mut self, spec: Spec, mode: FormMode) -> Result<()> {
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
        let mut ctx = self.ctx.silent();
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

    fn receive_choices(&mut self) -> bool {
        let Some(loading) = &self.loading else {
            return false;
        };
        let result = match loading.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err(Error::new(
                "error.io",
                "choice loader exited without a result",
            )),
        };
        let loading = self.loading.take().expect("pending form");
        if let Err(error) = result.and_then(|options| self.show_form(loading, options)) {
            self.fail_ui(error);
        }
        true
    }

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
        if form.listed(0) {
            form.open_picker(String::new(), true);
        }
        self.form = Some(form);
        Ok(())
    }

    pub(super) fn cancel_loading(&mut self) {
        if let Some(loading) = self.loading.take() {
            loading.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub(super) fn activate(&mut self) -> Result<Effect> {
        if self.nav == NavTarget::Overview {
            return self.activate_quick_card(self.quick_cursor);
        }
        let Some(row) = self.selected_row() else {
            return Ok(Effect::None);
        };
        let spec = match row {
            Row::Action(spec) => spec,
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

    pub(super) fn activate_action(&mut self, spec: Spec) -> Result<Effect> {
        self.available(&spec)?;
        match spec.kind {
            Kind::Info => {
                self.goto_nav(NavTarget::Overview);
                Ok(Effect::Refresh)
            }
            Kind::PublicIp => Ok(Effect::PublicIp),
            Kind::TerminalPreview => {
                self.open_prompt_preview();
                Ok(Effect::None)
            }
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

    pub(super) fn toggle_selected(&mut self) -> Result<()> {
        if !self.optimization() {
            return Ok(());
        }
        let Some(Row::Action(spec)) = self.selected_row() else {
            return Ok(());
        };
        self.available(&spec)?;
        if !self.marked.remove(spec.id) {
            self.marked.insert(spec.id.into());
        }
        self.progress = None;
        Ok(())
    }

    pub(super) fn cycle_language(&mut self) -> Result<()> {
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
        let mut preferences = Preferences::read(&self.ctx.paths.preferences, false)?;
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

    pub(super) fn optimization_jobs(&mut self) -> Result<Effect> {
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

    pub(super) fn checklist(&self) -> Vec<(Spec, Parameters)> {
        self.rows(OPTIMIZATION_GROUP)
            .iter()
            .filter_map(|row| match row {
                Row::Action(spec) if self.marked.contains(spec.id) => {
                    Some((spec.clone(), self.saved_parameters(spec)))
                }
                _ => None,
            })
            .collect()
    }

    pub(super) fn saved_parameters(&self, spec: &Spec) -> Parameters {
        let mut params = spec.defaults();
        if let Some(saved) = self.parameters.get(spec.id) {
            params.extend(saved.clone());
        }
        params
    }

    pub(super) fn recommend(&mut self) -> Result<()> {
        if !self.optimization() {
            return Ok(());
        }
        for id in RECOMMENDED {
            self.available(&actions::find(id)?)?;
        }
        let marked: BTreeSet<String> = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
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

    pub(super) fn submit_form(&mut self) -> Result<Effect> {
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

    pub(super) fn cycle_field(&mut self, forward: bool) {
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

    pub(super) fn advance_form(&mut self) -> Result<Effect> {
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

    pub(super) fn pick_choice(&mut self, value: String) -> Result<Effect> {
        let form = self.form.as_mut().expect("active form");
        form.picker = None;
        form.choose(form.index, value);
        self.advance_form()
    }

    pub(super) fn close_picker(&mut self) {
        let form = self.form.as_mut().expect("active form");
        if form.picker.take().is_some_and(|picker| picker.with_form) {
            self.form = None;
        }
    }

    pub(super) fn scroll_logs(&mut self, delta: i32) {
        let body = super::layout::log_body(self, self.viewport);
        if body.height == 0 {
            return;
        }
        let rows = render::logs::wrapped_line_count(self, body.width as usize);
        let max = rows
            .saturating_sub(body.height as usize)
            .min(u16::MAX as usize) as i64;
        self.log_scroll = (i64::from(self.log_scroll) + i64::from(delta)).clamp(0, max) as u16;
    }

    pub(super) fn open_github_proxy_form(&mut self) -> Result<Effect> {
        let spec = actions::find("github.proxy")?;
        self.activate_action(spec)
    }

    pub(super) fn preview_host(&self) -> &str {
        match &self.snapshot.local.host {
            Reading::Value(host) if !host.trim().is_empty() => host.trim(),
            _ => "vps-sg-01",
        }
    }

    pub(super) fn terminal_preview_options(&self) -> crate::ops::terminal::Options {
        if let Some(saved) = self.parameters.get("terminal.apply") {
            let defaults = crate::ops::terminal::Options::default();
            let opts = crate::ops::terminal::Options {
                shell: saved.get("shell").cloned().unwrap_or(defaults.shell),
                style: saved.get("style").cloned().unwrap_or(defaults.style),
                color: saved.get("color").cloned().unwrap_or(defaults.color),
                time: saved.get("time").cloned().unwrap_or(defaults.time),
                git: saved.get("git").cloned().unwrap_or(defaults.git),
            };
            if opts.validate().is_ok() {
                return opts;
            }
        }
        let saved_file = self.ctx.paths.state.join("terminal/options.json");
        if saved_file.exists()
            && let Ok(opts) = crate::ops::terminal::options(&self.ctx)
        {
            return opts;
        }
        // Showcase the modern Box + Tokyo Night preset when no prompt has been saved yet.
        crate::ops::terminal::Options {
            shell: "bash".into(),
            style: "box".into(),
            color: "tokyo-night".into(),
            time: "on".into(),
            git: "on".into(),
        }
    }

    pub(super) fn open_prompt_preview(&mut self) {
        let opts = self.terminal_preview_options();
        self.prompt_preview = Some(PromptPreview::from_options(&opts));
    }

    pub(super) fn apply_prompt_preview(&mut self) -> Result<Effect> {
        let Some(preview) = self.prompt_preview.clone() else {
            return Ok(Effect::None);
        };
        let params = preview.parameters();
        self.parameters
            .insert("terminal.apply".into(), params.clone());
        let apply_spec = actions::find("terminal.apply")?;
        if self.available(&apply_spec).is_err() {
            let msg = self.ctx.i18n.text("ui.prompt_apply_unavailable").to_owned();
            self.notify(msg, Tone::Warning);
            return Ok(Effect::None);
        }
        self.prompt_preview = None;
        Ok(Effect::Run(Task::Single {
            spec: apply_spec,
            params,
            reviewed: true,
        }))
    }

    pub(super) fn current_command_text(&self) -> Option<String> {
        match self.selected_row()? {
            Row::Action(spec) => {
                let params = self.saved_parameters(&spec);
                let words = preview::single_words(&spec, &params);
                Some(
                    words
                        .iter()
                        .map(|w| w.shown().into_owned())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
            Row::Execute => {
                let jobs = self.checklist();
                if jobs.is_empty() {
                    return None;
                }
                let words = preview::batch_words(&jobs);
                Some(
                    words
                        .iter()
                        .map(|w| w.shown().into_owned())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
            _ => None,
        }
    }

    pub(super) fn current_inspector_text(&self) -> Option<String> {
        let i = &self.ctx.i18n;
        match self.selected_row()? {
            Row::Action(spec) => {
                let mut out = Vec::new();
                out.push(format!("{} ({})", i.text(&spec.label()), spec.id));
                out.push(i.text(&spec.description()).to_owned());
                if let Some(rel) = self.related_for(&spec)
                    && !rel.state.is_empty()
                {
                    out.push(String::new());
                    out.push(i.text("ui.current_state").to_owned());
                    for line in rel.state {
                        out.push(line.to_string());
                    }
                }
                if let Some(cmd) = self.current_command_text() {
                    out.push(String::new());
                    out.push(cmd);
                }
                Some(out.join("\n"))
            }
            Row::Execute => self.current_command_text(),
            _ => None,
        }
    }

    pub(super) fn logs_text(&self) -> Option<String> {
        if self.logs.is_empty() {
            return None;
        }
        Some(
            self.logs
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    pub(super) fn overview_summary_text(&self) -> String {
        let l = &self.snapshot.local;
        let p = &self.snapshot.public;
        let mut out = Vec::new();
        if let Reading::Value(host) = &l.host {
            out.push(format!("Host: {host}"));
        }
        if let Reading::Value(os) = &l.os {
            out.push(format!("OS: {os}"));
        }
        if let Reading::Value(kernel) = &l.kernel {
            out.push(format!("Kernel: {kernel} ({})", l.arch));
        }
        if let Reading::Value(cpu) = l.cpu_usage {
            out.push(format!("CPU: {:.0}%", cpu.clamp(0.0, 100.0)));
        }
        if let Reading::Value(mem) = &l.memory
            && mem.total > 0
        {
            out.push(format!("Memory: {:.0}%", mem.percent()));
        }
        if let Reading::Value(disk) = &l.disk
            && disk.total > 0
        {
            out.push(format!("Disk: {:.0}%", disk.percent()));
        }
        if let Reading::Value(ipv4) = &p.ipv4 {
            out.push(format!("IPv4: {ipv4}"));
        }
        if let Reading::Value(ipv6) = &p.ipv6 {
            out.push(format!("IPv6: {ipv6}"));
        }
        if let Reading::Value(isp) = &p.isp {
            out.push(format!("ISP: {isp}"));
        }
        if let Reading::Value(state) = l.mihomo_state {
            let ver = match &l.mihomo_version {
                Reading::Value(v) => format!(" {v}"),
                _ => String::new(),
            };
            let status = match state {
                ClashState::Running(pid) => format!("Running (PID {pid})"),
                ClashState::Stopped => "Stopped".into(),
                ClashState::NotInstalled => "Not installed".into(),
                ClashState::Unknown => "Unknown".into(),
            };
            out.push(format!("Mihomo: {status}{ver}"));
        }
        if let Some(proxy) = &self.ctx.github_proxy {
            out.push(format!("GitHub Proxy: {proxy}"));
        }
        if out.is_empty() {
            format!("CqShell v{}", env!("CARGO_PKG_VERSION"))
        } else {
            out.join("\n")
        }
    }

    pub(super) fn copy_current_command(&mut self) -> Effect {
        if self.nav == NavTarget::Overview {
            let text = self.overview_summary_text();
            let msg = self.ctx.i18n.text("toast.copied_text").to_owned();
            self.notify(msg, Tone::Success);
            return Effect::Copy(text);
        }
        if let Some(cmd) = self.current_command_text() {
            let msg = self
                .ctx
                .i18n
                .format("toast.copied_cmd", &[("detail", &cmd)]);
            self.notify(msg, Tone::Success);
            Effect::Copy(cmd)
        } else {
            let msg = self.ctx.i18n.text("ui.no_selection").to_owned();
            self.notify(msg, Tone::Warning);
            Effect::None
        }
    }

    pub(super) fn copy_current_inspector(&mut self) -> Effect {
        if self.nav == NavTarget::Overview {
            return self.copy_current_command();
        }
        if let Some(text) = self.current_inspector_text() {
            let msg = self.ctx.i18n.text("toast.copied_text").to_owned();
            self.notify(msg, Tone::Success);
            Effect::Copy(text)
        } else {
            let msg = self.ctx.i18n.text("ui.no_selection").to_owned();
            self.notify(msg, Tone::Warning);
            Effect::None
        }
    }

    pub(super) fn copy_logs(&mut self) -> Effect {
        if let Some(text) = self.logs_text() {
            let count = self.logs.len().to_string();
            let msg = self
                .ctx
                .i18n
                .format("toast.copied_logs", &[("count", &count)]);
            self.notify(msg, Tone::Success);
            Effect::Copy(text)
        } else {
            let msg = self.ctx.i18n.text("toast.no_logs_to_copy").to_owned();
            self.notify(msg, Tone::Warning);
            Effect::None
        }
    }

    pub(super) fn copy_plain(&mut self, text: String) -> Effect {
        let msg = self.ctx.i18n.text("toast.copied_text").to_owned();
        self.notify(msg, Tone::Success);
        Effect::Copy(text)
    }

    pub(super) fn copy_selection(&mut self, text: String) -> Effect {
        let lines = text.lines().count().max(1);
        let msg = if lines > 1 {
            self.ctx.i18n.format(
                "toast.copied_selection_lines",
                &[("count", &lines.to_string())],
            )
        } else {
            self.ctx
                .i18n
                .format("toast.copied_selection", &[("detail", &text)])
        };
        self.notify(msg, Tone::Success);
        Effect::Copy(text)
    }

    pub(super) fn open_plain_view_logs(&mut self) -> Effect {
        if let Some(content) = self.logs_text() {
            let title = self.ctx.i18n.text("ui.logs").to_owned();
            Effect::PlainView { title, content }
        } else {
            let msg = self.ctx.i18n.text("toast.no_logs_to_copy").to_owned();
            self.notify(msg, Tone::Warning);
            Effect::None
        }
    }

    pub(super) fn open_plain_view_context(&mut self) -> Effect {
        if self.log_modal || (self.logs_open && !self.logs.is_empty()) {
            return self.open_plain_view_logs();
        }
        if self.nav == NavTarget::Overview {
            let title = self.ctx.i18n.text("nav.overview").to_owned();
            let content = self.overview_summary_text();
            return Effect::PlainView { title, content };
        }
        if let Some(content) = self.current_inspector_text() {
            let title = self.ctx.i18n.text("ui.details").to_owned();
            Effect::PlainView { title, content }
        } else {
            let msg = self.ctx.i18n.text("ui.no_selection").to_owned();
            self.notify(msg, Tone::Warning);
            Effect::None
        }
    }

    pub(super) fn open_socks_secret_modal(&mut self) -> Result<Effect> {
        let public_v4 = match &self.snapshot.public.ipv4 {
            Reading::Value(ip) if !ip.is_empty() => Some(ip.as_str()),
            _ => None,
        };
        match crate::ops::clash::socks::reveal(&self.ctx, public_v4) {
            Ok(Some(creds)) => {
                self.socks_secret = Some(SocksSecretModal { creds, scroll: 0 });
            }
            Ok(None) => {
                let msg = self.ctx.i18n.text("toast.socks_not_configured").to_owned();
                self.notify(msg, Tone::Warning);
            }
            Err(err) => {
                let msg = self.ctx.i18n.error(&err);
                self.notify(msg, Tone::Danger);
            }
        }
        Ok(Effect::None)
    }

    pub(super) fn close_socks_secret(&mut self) {
        self.socks_secret = None;
    }

    pub(super) fn copy_socks_secret_field(&mut self, value: String, field_key: &str) -> Effect {
        let field = self.ctx.i18n.text(field_key);
        let msg = self
            .ctx
            .i18n
            .format("toast.copied_socks_field", &[("field", field)]);
        self.notify(msg, Tone::Success);
        Effect::Copy(value)
    }

    pub(super) fn copy_socks_secret_password(&mut self) -> Effect {
        let password = self
            .socks_secret
            .as_ref()
            .and_then(|m| m.creds.password.clone());
        match password {
            Some(pw) => self.copy_socks_secret_field(pw, "ui.socks_secret_field_password"),
            None => {
                let msg = self.ctx.i18n.text("toast.socks_no_password").to_owned();
                self.notify(msg, Tone::Warning);
                Effect::None
            }
        }
    }

    pub(super) fn run_workbench_command(
        &mut self,
        cmd: palette::WorkbenchCommand,
    ) -> Result<Effect> {
        use palette::WorkbenchCommand;
        match cmd {
            WorkbenchCommand::Overview => self.goto_nav(NavTarget::Overview),
            WorkbenchCommand::Optimization => self.goto_nav(NavTarget::Optimization),
            WorkbenchCommand::Docker => {
                self.open_docker(self.docker.tab, self.docker.fullscreen, false);
            }
            WorkbenchCommand::DockerFocus => {
                let next = if self.is_docker_view() {
                    !self.docker.fullscreen
                } else {
                    true
                };
                self.open_docker(self.docker.tab, next, false);
            }
            WorkbenchCommand::Mihomo => {
                self.open_clash(self.clash.tab, self.clash.fullscreen, false);
            }
            WorkbenchCommand::MihomoFocus => {
                let next = if self.is_clash_view() {
                    !self.clash.fullscreen
                } else {
                    true
                };
                self.open_clash(self.clash.tab, next, false);
            }
            WorkbenchCommand::GithubProxy => return self.open_github_proxy_form(),
            WorkbenchCommand::SocksSecret => return self.open_socks_secret_modal(),
            WorkbenchCommand::Theme => self.open_theme_drawer(),
            WorkbenchCommand::TransparentBg => self.toggle_transparent_bg(true),
            WorkbenchCommand::ReducedMotion => self.toggle_reduced_motion(true),
            WorkbenchCommand::Sidebar => self.toggle_sidebar(),
            WorkbenchCommand::Language => self.cycle_language()?,
            WorkbenchCommand::Refresh => return Ok(Effect::Refresh),
            WorkbenchCommand::PublicIp => return Ok(Effect::PublicIp),
            WorkbenchCommand::Logs => self.logs_open = !self.logs_open,
            WorkbenchCommand::FullLogs => self.log_modal = !self.log_modal,
            WorkbenchCommand::CopyCommand => return Ok(self.copy_current_command()),
            WorkbenchCommand::CopyLogs => return Ok(self.copy_logs()),
            WorkbenchCommand::PlainView => return Ok(self.open_plain_view_context()),
        }
        Ok(Effect::None)
    }

    pub(super) fn cached_socks_config(&self) -> Option<crate::ops::clash::socks::SocksConfig> {
        let stat_key = std::fs::metadata(&self.ctx.paths.clash_mixin)
            .ok()
            .map(|meta| {
                #[cfg(unix)]
                let ino = {
                    use std::os::unix::fs::MetadataExt;
                    meta.ino()
                };
                #[cfg(not(unix))]
                let ino = 0u64;
                (meta.len(), meta.modified().ok(), ino)
            });
        if let Some(entry) = self.socks_cache.borrow().as_ref()
            && entry.key == stat_key
        {
            return entry.config.clone();
        }
        let parsed = crate::ops::clash::socks::inspect(&self.ctx).ok();
        *self.socks_cache.borrow_mut() = Some(SocksCacheEntry {
            key: stat_key,
            config: parsed.clone(),
        });
        parsed
    }

    pub(super) fn related_for(&self, spec: &Spec) -> Option<related::Related> {
        if spec.id == "github.proxy" {
            return Some(related::github_proxy(
                self.ctx.github_proxy.as_deref(),
                &self.ctx.i18n,
            ));
        }
        if matches!(
            spec.id,
            "clash.socks.status"
                | "clash.socks.configure"
                | "clash.socks.auth.generate"
                | "clash.socks.auth.set"
        ) {
            let cached = self.cached_socks_config();
            return Some(related::clash_socks(
                cached.as_ref(),
                &self.snapshot,
                &self.ctx.i18n,
            ));
        }
        related::related(spec, &self.snapshot, &self.ctx.i18n, self.now)
    }
}

pub(super) fn task_outcome(result: &Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Done,
        Err(e) if e.key == "error.cancelled" => Outcome::Cancelled,
        Err(_) => Outcome::Failed,
    }
}

pub(super) fn format_mm_ss(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

pub(super) fn gauge_targets(snapshot: &Snapshot) -> [Option<f64>; 4] {
    let local = &snapshot.local;
    let pct = |reading: &Reading<crate::dashboard::Usage>| match reading {
        Reading::Value(u) if u.total > 0 => Some(u.percent()),
        _ => None,
    };
    let cpu = match local.cpu_usage {
        Reading::Value(v) => Some(f64::from(v)),
        _ => None,
    };
    [cpu, pct(&local.memory), pct(&local.disk), pct(&local.swap)]
}

pub(super) fn snapshot_pending(snapshot: &Snapshot) -> bool {
    let l = &snapshot.local;
    let p = &snapshot.public;
    matches!(l.cpu_usage, Reading::Loading)
        || matches!(l.memory, Reading::Loading)
        || matches!(l.disk, Reading::Loading)
        || matches!(l.host, Reading::Loading)
        || matches!(p.ipv4, Reading::Loading)
        || matches!(p.ipv6, Reading::Loading)
        || matches!(p.isp, Reading::Loading)
}
