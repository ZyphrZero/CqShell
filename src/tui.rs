mod input;
mod view;

use crate::{
    actions::{self, Kind, Parameters, Spec},
    dashboard::{Monitor, Snapshot},
    error::{Error, Result},
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
use ratatui::{DefaultTerminal, Frame, layout::Rect};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Write},
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
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
    "optimization",
];
const OPTIMIZATION_GROUP: usize = GROUPS.len() - 1;
type Jobs = Vec<(Spec, Parameters)>;

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
    positions: [usize; GROUPS.len()],
    focus: Focus,
    marked: BTreeSet<String>,
    parameters: BTreeMap<String, Parameters>,
    logs: VecDeque<String>,
    scroll: u16,
    logs_open: bool,
    log_modal: bool,
    phase: Phase,
    task_ids: Vec<String>,
    form: Option<Form>,
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
}
impl App {
    pub fn new(ctx: Context) -> Self {
        let platform_error = ctx.platform().err();
        #[cfg(unix)]
        let root = unsafe { libc::geteuid() } == 0;
        #[cfg(not(unix))]
        let root = false;
        Self {
            ctx,
            group: 0,
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
        }
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
        for line in text.as_ref().lines() {
            self.logs.push_back(crate::runtime::sanitize(line));
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
        if let Err(error) = result {
            self.log(self.ctx.i18n.error(&error));
        } else {
            let message = self.ctx.i18n.text("ui.done").to_owned();
            self.log(message);
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
            spec,
        })
    }
    fn activate(&mut self) -> Result<Effect> {
        let spec = self.items()[self.selected()].clone();
        self.available(&spec)?;
        if self.optimization() {
            if spec.fields.is_empty() {
                self.toggle_selected()?;
            } else {
                self.form = Some(self.make_form(spec, FormMode::Optimization)?);
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
                self.form = Some(self.make_form(spec, FormMode::Run)?);
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
    fn optimization_jobs(&mut self) -> Result<Effect> {
        if !self.optimization() {
            return Ok(Effect::None);
        }
        if self.marked.is_empty() {
            self.notice = Some(self.ctx.i18n.text("ui.no_selection").into());
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
            let next = match (current, forward) {
                (Some(current), true) => (current + 1) % choices.len(),
                (Some(current), false) => (current + choices.len() - 1) % choices.len(),
                (None, true) => 0,
                (None, false) => choices.len() - 1,
            };
            form.values[form.index].replace(choices[next].into());
        }
    }
    fn form_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let form = self.form.as_mut().expect("active form");
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
            KeyCode::Enter => form.index += 1,
            KeyCode::Left => self.cycle_field(false),
            KeyCode::Right => self.cycle_field(true),
            KeyCode::Char(' ') if form.index >= n => {
                if form.index == n {
                    return self.submit_form();
                }
                self.form = None;
            }
            _ if form.index >= n => (),
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
            KeyCode::PageUp => confirm.scroll = confirm.scroll.saturating_sub(5),
            KeyCode::PageDown => confirm.scroll = confirm.scroll.saturating_add(5),
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
    fn key(&mut self, key: KeyEvent) -> Result<Effect> {
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
        if self.form.is_some() {
            return self.form_key(key);
        }
        if self.notice.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.notice = None;
            }
            return Ok(Effect::None);
        }
        if self.overview {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overview = false,
                KeyCode::Down | KeyCode::PageDown => {
                    self.overview_scroll = self.overview_scroll.saturating_add(3)
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.overview_scroll = self.overview_scroll.saturating_sub(3)
                }
                KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
                KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
                _ => (),
            }
            return Ok(Effect::None);
        }
        if self.log_modal {
            match key.code {
                KeyCode::Esc | KeyCode::Char('g' | 'G' | 'q') | KeyCode::F(2) => {
                    self.log_modal = false
                }
                code => self.scroll_key(code),
            }
            return Ok(Effect::None);
        }
        if self.worker.is_some() && key.code == KeyCode::Esc {
            self.request_cancel(false);
            return Ok(Effect::None);
        }
        match key.code {
            KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
            KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
            KeyCode::Char('g' | 'G') => self.logs_open = !self.logs_open,
            KeyCode::F(2) => self.log_modal = !self.log_modal,
            KeyCode::Char('l' | 'L') => {
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
                self.ctx.i18n.select(&locales[next])?;
            }
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
                    self.group = 0;
                }
            }
            KeyCode::End => {
                if self.focus == Focus::Actions {
                    self.positions[self.group] = self.items().len() - 1;
                } else {
                    self.group = GROUPS.len() - 1;
                }
            }
            KeyCode::Char('q' | 'Q') if self.worker.is_none() => return Ok(Effect::Quit),
            KeyCode::Enter if self.focus == Focus::Categories => self.focus = Focus::Actions,
            KeyCode::Enter if self.worker.is_none() => return self.activate(),
            KeyCode::Char(' ') if self.worker.is_none() && self.focus == Focus::Actions => {
                self.toggle_selected()?
            }
            KeyCode::F(5) if self.worker.is_none() => return self.optimization_jobs(),
            code => self.scroll_key(code),
        }
        Ok(Effect::None)
    }
    fn move_selection(&mut self, forward: bool) {
        if self.focus == Focus::Categories {
            self.group = if forward {
                (self.group + 1) % GROUPS.len()
            } else {
                (self.group + GROUPS.len() - 1) % GROUPS.len()
            };
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
    fn scroll_key(&mut self, code: KeyCode) {
        let width = if self.log_modal {
            self.viewport.width.saturating_sub(6)
        } else {
            self.viewport.width.saturating_sub(2)
        };
        let rows = view::log_rows(&self.logs, width as usize).len();
        match code {
            KeyCode::PageUp => {
                self.scroll = self
                    .scroll
                    .saturating_add(5)
                    .min(rows.saturating_sub(1).min(u16::MAX as usize) as u16)
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(5),
            _ => (),
        }
    }
    fn mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point = (event.column, event.row).into();
        if let Some(confirm) = &mut self.confirmation {
            let geometry = view::ConfirmationGeometry::new(area, &self.ctx.i18n);
            match event.kind {
                MouseEventKind::ScrollUp => confirm.scroll = confirm.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => confirm.scroll = confirm.scroll.saturating_add(3),
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
        if let Some(form) = &mut self.form {
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
                    if !form.spec.fields[index].choices.is_empty() {
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
        if self.overview {
            match event.kind {
                MouseEventKind::ScrollUp => {
                    self.overview_scroll = self.overview_scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown => {
                    self.overview_scroll = self.overview_scroll.saturating_add(3)
                }
                _ => (),
            }
            return Ok(Effect::None);
        }
        if self.log_modal {
            match event.kind {
                MouseEventKind::ScrollUp => self.scroll_key(KeyCode::PageUp),
                MouseEventKind::ScrollDown => self.scroll_key(KeyCode::PageDown),
                _ => (),
            }
            return Ok(Effect::None);
        }
        let layout = view::Regions::new(area, self);
        let categories = layout.categories.inner(ratatui::layout::Margin::new(1, 1));
        let actions = layout.actions.inner(ratatui::layout::Margin::new(1, 1));
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if categories.contains(point) => {
                let row = event.row.saturating_sub(layout.categories.y + 1) as usize;
                let offset = view::list_offset(
                    self.group,
                    GROUPS.len(),
                    layout.categories.height.saturating_sub(2) as usize,
                );
                if row + offset < GROUPS.len() {
                    self.group = row + offset;
                    self.focus = Focus::Categories;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if actions.contains(point) => {
                let row = event.row.saturating_sub(layout.actions.y + 1) as usize;
                let offset = view::list_offset(
                    self.selected(),
                    self.items().len(),
                    layout.actions.height.saturating_sub(2) as usize,
                );
                if row + offset < self.items().len() {
                    self.positions[self.group] = row + offset;
                    self.focus = Focus::Actions;
                    if self.optimization()
                        && self.worker.is_none()
                        && event.column < layout.actions.x + 6
                    {
                        self.toggle_selected()?;
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if layout.logs.contains(point) =>
            {
                self.scroll_key(if event.kind == MouseEventKind::ScrollUp {
                    KeyCode::PageUp
                } else {
                    KeyCode::PageDown
                })
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if layout.actions.contains(point) =>
            {
                self.focus = Focus::Actions;
                self.move_selection(event.kind == MouseEventKind::ScrollDown);
            }
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
            terminal.draw(|f| app.draw(f))?;
            if !event::poll(Duration::from_millis(100))? {
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
        assert_eq!(app.group, 1);
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
        let geometry = view::ConfirmationGeometry::new(area, &app.ctx.i18n);
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
            assert!(app.logs.iter().any(|l| l == "last message"));
            assert_eq!(app.phase, Phase::Completed);
        }
    }
    #[test]
    fn cancellation_is_not_reported_as_failure() {
        let mut app = app();
        app.finish(Err(Error::new("error.cancelled", "")));
        assert_eq!(app.phase, Phase::Cancelled);
    }
}
