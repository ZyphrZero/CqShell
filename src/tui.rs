mod input;
mod motion;
mod view;

use crate::{
    actions::{self, Kind, Listing, Parameters, Source, Spec},
    dashboard::{Monitor, Snapshot},
    error::{Error, Result},
    ops::Choice,
    runtime::{Context, Event},
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
const OPTIMIZATION_GROUP: usize = GROUPS.len() - 1;
const DEFAULT_CATEGORY_ORDER: &[&str] = &[
    "system",
    "kernel",
    "network",
    "ssh",
    "docker",
    "clash",
    "tools",
    "ai",
    "terminal",
    "tests",
    "app",
    "optimization",
];
type Jobs = Vec<(Spec, Parameters)>;

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
    for index in 0..GROUPS.len() {
        if !order.contains(&index) {
            order.push(index);
        }
    }
    order
}

fn category_order_from_ids(ids: &[String]) -> Result<Vec<usize>> {
    if ids.is_empty() {
        return Ok(default_category_order());
    }
    if ids.len() > GROUPS.len() {
        return Err(Error::new(
            "error.config",
            "category_order contains too many categories",
        ));
    }
    let mut seen = [false; GROUPS.len()];
    let mut order = Vec::with_capacity(ids.len());
    for id in ids {
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

fn load_category_order(ctx: &Context) -> Result<Vec<usize>> {
    let preferences = crate::config::Preferences::read(&ctx.paths.preferences, false)?;
    category_order_from_ids(&preferences.category_order)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Categories,
    Actions,
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
}
#[derive(Debug)]
enum Effect {
    None,
    Quit,
    Run(Jobs),
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
    positions: [usize; GROUPS.len()],
    focus: Focus,
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
    hover: Option<Position>,
    gauges: [Option<motion::Tween>; 4],
    /// A user-requested refresh in flight; cleared when a newer sample arrives.
    refreshing: Option<Instant>,
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
            focus: Focus::Actions,
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
            hover: None,
            gauges: [None; 4],
            refreshing: None,
        }
    }
    fn elapsed(&self) -> Duration {
        self.now.saturating_duration_since(self.started)
    }
    fn spin(&self) -> &'static str {
        motion::spinner(self.elapsed())
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
        if moving || self.loading.is_some() {
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
    fn category_position(&self) -> usize {
        self.category_order
            .iter()
            .position(|index| *index == self.group)
            .unwrap_or(0)
    }
    fn set_category_order(&mut self, order: Vec<usize>) {
        self.group = order[0];
        self.category_order = order;
    }
    fn optimization(&self) -> bool {
        self.group == OPTIMIZATION_GROUP
    }
    fn selected(&self) -> usize {
        self.positions[self.group]
    }
    fn items(&self) -> Vec<Spec> {
        if self.optimization() {
            actions::OPTIMIZATION
                .iter()
                .map(|id| actions::find(id).expect("static action ID"))
                .collect()
        } else {
            actions::specs()
                .into_iter()
                .filter(|s| s.group == GROUPS[self.group])
                .collect()
        }
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
        for line in text.as_ref().lines() {
            self.logs.push_back(LogLine {
                text: crate::runtime::sanitize(line),
                tone,
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
        self.phase = match &result {
            Ok(()) => Phase::Completed,
            Err(e) if e.key == "error.cancelled" => Phase::Cancelled,
            Err(_) => Phase::Failed,
        };
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
                    self.confirmation = Some(Confirmation {
                        text,
                        token,
                        input: InputBuffer::default(),
                        reply,
                        scroll: 0,
                        accept: false,
                    })
                }
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
    fn start(&mut self, terminal: &mut DefaultTerminal, jobs: Jobs) -> Result<()> {
        if self.worker.is_some() {
            return Err(Error::new("error.busy", ""));
        }
        for (spec, p) in &jobs {
            self.available(spec)?;
            spec.parameters(p)?;
        }
        self.ctx.cancel.store(false, Ordering::SeqCst);
        self.logs.clear();
        self.scroll = 0;
        self.logs_open = true;
        self.phase = Phase::Running;
        self.task_ids = jobs.iter().map(|(s, _)| s.id.into()).collect();
        if jobs.iter().any(|(s, _)| s.external) {
            leave()?;
            println!("{}", self.ctx.i18n.text("ui.external"));
            let result = perform(&self.ctx, &jobs);
            if let Err(e) = &result {
                eprintln!("{}", self.ctx.i18n.error(e));
            }
            println!("{}", self.ctx.i18n.text("ui.return"));
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            enter()?;
            terminal.clear()?;
            self.finish(result);
        } else {
            let (tx, rx) = mpsc::channel();
            let mut ctx = self.ctx.clone();
            ctx.events = Some(tx.clone());
            self.receiver = Some(rx);
            self.worker = Some(thread::spawn(move || {
                let result = perform(&ctx, &jobs);
                let _ = tx.send(Event::Finished(result));
            }));
        }
        Ok(())
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
        let spec = self.items()[self.selected()].clone();
        self.available(&spec)?;
        if self.optimization() {
            if spec.fields.is_empty() {
                self.toggle_selected()?;
            } else {
                self.open_form(spec, FormMode::Optimization)?;
            }
            return Ok(Effect::None);
        }
        match spec.kind {
            Kind::Info => {
                self.overview = true;
                self.overview_scroll = 0;
                Ok(Effect::Refresh)
            }
            Kind::PublicIp => Ok(Effect::PublicIp),
            _ if spec.fields.is_empty() => Ok(Effect::Run(vec![(spec, Parameters::new())])),
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
        let spec = self.items()[self.selected()].clone();
        self.available(&spec)?;
        if !self.marked.remove(spec.id) {
            self.marked.insert(spec.id.into());
        }
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
        for spec in self
            .items()
            .into_iter()
            .filter(|s| self.marked.contains(s.id))
        {
            self.available(&spec)?;
            let values = self
                .parameters
                .get(spec.id)
                .cloned()
                .unwrap_or_else(|| spec.defaults());
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
        Ok(Effect::Run(jobs))
    }
    fn submit_form(&mut self) -> Result<Effect> {
        let form = self.form.as_ref().expect("active form");
        let values = form
            .spec
            .fields
            .iter()
            .zip(&form.values)
            .map(|(field, value)| (field.name.into(), value.text.clone()))
            .collect();
        let values = form.spec.parameters(&values)?;
        let spec = form.spec.clone();
        let mode = form.mode;
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
            Ok(Effect::Run(vec![(spec, values)]))
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
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
    fn reorder_category(&mut self, forward: bool) -> Result<()> {
        let position = self.category_position();
        let destination = if forward {
            position + 1
        } else {
            position.saturating_sub(1)
        };
        if destination >= self.category_order.len() || destination == position {
            return Ok(());
        }
        let mut order = self.category_order.clone();
        order.swap(position, destination);
        let mut preferences = crate::config::Preferences::read(&self.ctx.paths.preferences, false)?;
        preferences.category_order = order
            .iter()
            .map(|index| GROUPS[*index].to_owned())
            .collect();
        preferences.write(&self.ctx.paths.preferences)?;
        self.category_order = order;
        let message = self.ctx.i18n.text("toast.order_saved").to_owned();
        self.notify(message, Tone::Success);
        Ok(())
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
        if self.worker.is_some() && key.code == KeyCode::Esc {
            self.request_cancel(false);
            return Ok(Effect::None);
        }
        if self.worker.is_none()
            && self.focus == Focus::Categories
            && key.modifiers.contains(KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Up => {
                    self.reorder_category(false)?;
                    return Ok(Effect::None);
                }
                KeyCode::Down => {
                    self.reorder_category(true)?;
                    return Ok(Effect::None);
                }
                _ => (),
            }
        }
        match key.code {
            KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
            KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
            KeyCode::Char('g' | 'G') => self.logs_open = !self.logs_open,
            KeyCode::F(2) => self.log_modal = !self.log_modal,
            KeyCode::Char('l' | 'L') => self.cycle_language()?,
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::Actions {
                    Focus::Categories
                } else {
                    Focus::Actions
                }
            }
            KeyCode::Left | KeyCode::Esc => self.focus = Focus::Categories,
            KeyCode::Right => self.focus = Focus::Actions,
            KeyCode::Char('o' | 'O') => {
                self.group = OPTIMIZATION_GROUP;
                self.focus = Focus::Actions;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Home => {
                if self.focus == Focus::Actions {
                    self.positions[self.group] = 0;
                } else {
                    self.group = self.category_order[0];
                }
            }
            KeyCode::End => {
                if self.focus == Focus::Actions {
                    self.positions[self.group] = self.items().len() - 1;
                } else {
                    self.group = *self
                        .category_order
                        .last()
                        .expect("category order is not empty");
                }
            }
            KeyCode::Char('q' | 'Q') if self.worker.is_none() => return Ok(Effect::Quit),
            KeyCode::Enter if self.focus == Focus::Categories => self.focus = Focus::Actions,
            KeyCode::Enter if self.worker.is_none() => return self.activate(),
            KeyCode::Char(' ') if self.worker.is_none() && self.focus == Focus::Actions => {
                self.toggle_selected()?
            }
            KeyCode::F(5) if self.worker.is_none() => return self.optimization_jobs(),
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            code => self.log_key(code),
        }
        Ok(Effect::None)
    }
    fn move_selection(&mut self, forward: bool) {
        if self.focus == Focus::Categories {
            let position = self.category_position();
            let next = if forward {
                (position + 1) % self.category_order.len()
            } else {
                (position + self.category_order.len() - 1) % self.category_order.len()
            };
            self.group = self.category_order[next];
        } else {
            let n = self.items().len();
            let index = &mut self.positions[self.group];
            *index = if forward {
                (*index + 1) % n
            } else {
                (*index + n - 1) % n
            };
        }
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
        if let Some(form) = &mut self.form {
            if form.picker.is_some() {
                return self.picker_mouse(event, area);
            }
            let geometry = view::FormGeometry::new(area, form, &self.ctx.i18n);
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if geometry.buttons[0].contains(point) {
                    return self.submit_form();
                }
                if geometry.buttons[1].contains(point) {
                    self.form = None;
                    return Ok(Effect::None);
                }
                if let Some(index) = geometry.field_at(point, form.values.len()) {
                    form.index = index;
                    if form.listed(index) {
                        form.open_picker(String::new(), false);
                    } else if !form.spec.fields[index].choices.is_empty() {
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
            if matches!(event.kind, MouseEventKind::Down(_)) {
                self.help = false;
            }
            return Ok(Effect::None);
        }
        if self.overview {
            let max = view::OverviewGeometry::new(area, self).max_scroll();
            let scroll = self.overview_scroll.min(max);
            self.overview_scroll = match event.kind {
                MouseEventKind::ScrollUp => scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => scroll.saturating_add(3).min(max),
                _ => scroll,
            };
            return Ok(Effect::None);
        }
        if self.log_modal {
            match event.kind {
                MouseEventKind::ScrollUp => self.scroll_logs(3),
                MouseEventKind::ScrollDown => self.scroll_logs(-3),
                _ => (),
            }
            return Ok(Effect::None);
        }
        let layout = view::Regions::new(area, self);
        let (categories, actions) = (layout.category_rows, layout.action_rows);
        let scroll = matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        );
        let down = event.kind == MouseEventKind::ScrollDown;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if categories.contains(point) => {
                let offset = view::list_offset(
                    self.category_position(),
                    self.category_order.len(),
                    categories.height as usize,
                );
                let index = offset + usize::from(event.row - categories.y);
                if index < self.category_order.len() {
                    self.group = self.category_order[index];
                    self.focus = Focus::Categories;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if actions.contains(point) => {
                let count = self.items().len();
                let offset = view::list_offset(self.selected(), count, actions.height as usize);
                let index = offset + usize::from(event.row - actions.y);
                if index < count {
                    self.positions[self.group] = index;
                    self.focus = Focus::Actions;
                    // The checkbox sits after the two-cell selection marker.
                    if self.optimization() && self.worker.is_none() && event.column < actions.x + 5
                    {
                        self.toggle_selected()?;
                    }
                }
            }
            _ if scroll && layout.logs.contains(point) => {
                self.scroll_logs(if down { -3 } else { 3 })
            }
            _ if scroll && layout.categories.contains(point) => {
                self.focus = Focus::Categories;
                self.move_selection(down);
            }
            _ if scroll && layout.actions.contains(point) => {
                self.focus = Focus::Actions;
                self.move_selection(down);
            }
            _ => (),
        }
        Ok(Effect::None)
    }
    /// A click picks the row under it; the wheel moves the highlight. Clicks elsewhere are
    /// ignored, as in other dialogs.
    fn picker_mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        let (clicked, count) = {
            let form = self.form.as_ref().expect("active form");
            let picker = form.picker.as_ref().expect("open picker");
            let choices = form.choices(form.index);
            let matches = picker.matches(choices);
            let geometry =
                view::PickerGeometry::new(area, choices.len(), picker.cursor, matches.len());
            let clicked = geometry
                .row_at(point, matches.len())
                .map(|row| matches[row].value.clone());
            (clicked, matches.len())
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
            MouseEventKind::ScrollDown => {
                picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1))
            }
            MouseEventKind::ScrollUp => picker.cursor = picker.cursor.saturating_sub(1),
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
fn perform(ctx: &Context, jobs: &[(Spec, Parameters)]) -> Result<()> {
    if jobs.len() == 1 {
        actions::execute(ctx, &jobs[0].0, &jobs[0].1)
    } else {
        actions::batch(ctx, jobs)
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

pub fn run(ctx: Context) -> Result<()> {
    let category_order = load_category_order(&ctx)?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        previous(info);
    }));
    enter()?;
    let _guard = TerminalGuard;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let monitor = Monitor::start(&ctx);
    let mut app = App::new(ctx);
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
                Ok(Effect::Run(jobs)) => app.start(&mut terminal, jobs),
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

    #[test]
    fn category_order_defaults_for_legacy_preferences_and_rejects_invalid_ids() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        std::fs::write(&ctx.paths.preferences, "language = \"en-US\"\n").unwrap();
        assert_eq!(load_category_order(&ctx).unwrap(), default_category_order());
        assert_eq!(category_order_from_ids(&["kernel".into()]).unwrap()[0], 2);

        let mut unknown = DEFAULT_CATEGORY_ORDER
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        unknown[0] = "unknown".into();
        assert!(category_order_from_ids(&unknown).is_err());

        let mut duplicate = DEFAULT_CATEGORY_ORDER
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        duplicate[1] = duplicate[0].clone();
        assert!(category_order_from_ids(&duplicate).is_err());
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
        }
        .write(&preferences_path)
        .unwrap();

        let mut app = App::new(ctx);
        let tools = GROUPS.iter().position(|group| *group == "tools").unwrap();
        app.group = tools;
        app.focus = Focus::Categories;
        app.positions[tools] = 2;
        assert_eq!(app.category_position(), 6);
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

        let mut restarted = App::new(crate::tests::context(temp.path()));
        restarted.set_category_order(load_category_order(&restarted.ctx).unwrap());
        assert_eq!(restarted.category_order, app.category_order);
        assert_eq!(restarted.group, app.category_order[0]);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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

    #[test]
    fn common_tools_and_ai_are_reachable_without_a_package_selector() {
        let mut app = app();
        app.platform_error = None;
        app.root = true;
        app.group = GROUPS.iter().position(|group| *group == "tools").unwrap();
        for (index, package) in crate::ops::tools::PACKAGES.iter().enumerate() {
            app.positions[app.group] = index;
            match app.activate().unwrap() {
                Effect::Run(jobs) => {
                    assert_eq!(jobs.len(), 1);
                    assert_eq!(jobs[0].0.kind, Kind::ToolInstall(package));
                    assert!(jobs[0].1.is_empty());
                }
                other => panic!("expected one package installation, got {other:?}"),
            }
        }
        app.root = false;
        app.group = GROUPS.iter().position(|group| *group == "ai").unwrap();
        for index in 0..3 {
            app.positions[app.group] = index;
            match app.activate().unwrap() {
                Effect::Run(jobs) => assert!(!jobs[0].0.external),
                other => panic!("expected a background binary installation, got {other:?}"),
            }
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
        app.group = OPTIMIZATION_GROUP;
        app.positions[app.group] = 3; // BBR has a queue selector.
        assert!(matches!(
            app.key(key(KeyCode::Enter)).unwrap(),
            Effect::None
        ));
        assert_eq!(app.form.as_ref().unwrap().mode, FormMode::Optimization);
        assert!(matches!(app.submit_form().unwrap(), Effect::None));
        assert!(app.marked.contains("bbr.enable"));
        match app.key(key(KeyCode::F(5))).unwrap() {
            Effect::Run(jobs) => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].0.id, "bbr.enable");
            }
            _ => panic!("expected optimization jobs"),
        }
    }
    #[test]
    fn category_navigation_has_explicit_focus_and_remembers_positions() {
        let mut app = app();
        app.positions[0] = 2;
        app.key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.focus, Focus::Categories);
        app.key(key(KeyCode::Down)).unwrap();
        assert_eq!(
            app.group,
            GROUPS.iter().position(|group| *group == "kernel").unwrap()
        );
        app.key(key(KeyCode::Enter)).unwrap();
        assert_eq!(app.focus, Focus::Actions);
        app.key(key(KeyCode::Esc)).unwrap();
        app.key(key(KeyCode::Up)).unwrap();
        assert_eq!(app.selected(), 2);
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
        let geometry = view::FormGeometry::new(area, app.form.as_ref().unwrap(), &app.ctx.i18n);
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
        let geometry = view::FormGeometry::new(area, app.form.as_ref().unwrap(), &app.ctx.i18n);
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
    fn help_overlay_swallows_navigation_and_closes_without_side_effects() {
        let mut app = app();
        app.key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help);
        let group = app.group;
        for code in [KeyCode::Down, KeyCode::Tab, KeyCode::Char('o')] {
            assert!(matches!(app.key(key(code)).unwrap(), Effect::None));
        }
        assert_eq!(app.group, group);
        assert_eq!(app.focus, Focus::Actions);
        app.key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help);
        assert_eq!(app.focus, Focus::Actions, "Esc closed help only");
        app.key(key(KeyCode::F(1))).unwrap();
        assert!(app.help);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 15,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click, Rect::new(0, 0, 120, 36)).unwrap();
        assert!(!app.help);
        assert_eq!(
            app.group, group,
            "the closing click does not reach the menu"
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
        app.positions[app.group] = app.items().iter().position(|s| s.id == id).unwrap();
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
            Effect::Run(jobs) => {
                assert_eq!(jobs[0].0.id, "clash.sub.use");
                assert_eq!(jobs[0].1["name"], "home");
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
            Effect::Run(jobs) => {
                assert_eq!(jobs[0].1["proxy_group"], "Streaming");
                assert_eq!(jobs[0].1["proxy_node"], "JP");
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
            Effect::Run(jobs) => assert_eq!(jobs[0].0.id, "tcp.tune"),
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
}
