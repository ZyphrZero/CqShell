//! Modern Sidebar-Shell TUI for CqShell.
//!
//! Architecture:
//! - `state`: Application state machine, navigation, forms, confirmations, batch progress
//! - `events`: Keyboard & mouse event router following the overlay priority stack
//! - `layout`: Four-tier responsive geometry (`Wide`, `Medium`, `Compact`, `Mobile`) shared by render & hit-testing
//! - `fx`: Pure-Ratatui buffer-level motion & visual effects (scrim, shadow, fades, gradients, pulse)
//! - `palette`: Enhanced Command Palette with recent actions, workbench commands, and fuzzy matching
//! - `theme_drawer`: Live theme & appearance drawer (transparent bg + reduced motion toggles)
//! - `render`: Layered-surface views (`topbar`, `sidebar`, `overview`, `workbench`, `logs`, `modals`)

mod clash;
mod clipboard;
mod detect;
mod docker;
mod events;
mod fx;
mod input;
mod layout;
mod palette;
mod preview;
mod related;
mod render;
mod sections;
mod selection;
mod session;
mod state;
mod theme_drawer;

#[cfg(test)]
mod snapshot;
#[cfg(test)]
mod tests;

pub use state::App;

/// Initial screen opened when launching the interactive TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTarget {
    Overview,
    Docker {
        tab: crate::ops::docker::model::DockerTab,
        fullscreen: bool,
    },
    Clash {
        tab: crate::ops::clash::model::ClashTab,
        fullscreen: bool,
    },
}

use crate::{
    actions,
    config::Preferences,
    dashboard::Monitor,
    error::{Error, Result},
    ops::{
        clash::{
            model::ClashTab,
            monitor::{ClashMonitor, ClashMonitorCmd},
        },
        docker::{
            actions as docker_actions,
            model::DockerTab,
            monitor::{DockerMonitor, LogStreamer, MonitorCmd},
        },
    },
    runtime::{Context, Event},
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as Input, KeyEventKind, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::DefaultTerminal;
use state::{Effect, Phase, Task, category_order_from_ids};
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

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
        Task::DockerOp(op) => actions::BatchReport {
            steps: Vec::new(),
            result: op.execute(ctx),
        },
    }
}

const MAX_DRAIN_EVENTS: usize = 4096;
const MAX_COALESCED_EVENTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollReady {
    Ready,
    Timeout,
    Disconnected,
}

/// Detect whether the interactive terminal on stdin has been hung up or closed (e.g. SSH disconnect
/// where `/dev/pts/N` becomes `(deleted)`). Must be checked before entering `crossterm::event::poll`
/// or `event::read`, because `crossterm 0.29`'s `mio` source busy-spins forever on `read() == 0` /
/// `EIO` from a hung-up PTY.
#[cfg(unix)]
fn tty_disconnected() -> bool {
    let mut pfd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return true;
        }
        if rc == 0 {
            return false;
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return true;
        }
        if pfd.revents & libc::POLLIN != 0 {
            let mut avail: libc::c_int = 0;
            let ioctl_rc =
                unsafe { libc::ioctl(libc::STDIN_FILENO, libc::FIONREAD as _, &mut avail) };
            if ioctl_rc == 0 && avail == 0 {
                return true;
            }
            if ioctl_rc < 0 {
                let err = io::Error::last_os_error();
                if matches!(
                    err.raw_os_error(),
                    Some(libc::EIO | libc::EBADF | libc::ENODEV | libc::ENXIO)
                ) {
                    return true;
                }
            }
        }
        return false;
    }
}

#[cfg(not(unix))]
fn tty_disconnected() -> bool {
    false
}

/// Wait for a terminal input event without ever blocking inside `crossterm`'s `mio` poll loop on
/// Unix, so a PTY hangup (`POLLHUP`) or `SIGHUP` (`cancel`) wakes immediately instead of spinning.
fn poll_terminal_event(timeout: Duration, cancel: &AtomicBool) -> io::Result<PollReady> {
    if cancel.load(Ordering::SeqCst) || tty_disconnected() {
        return Ok(PollReady::Disconnected);
    }
    // Flush any already-parsed events buffered inside crossterm's internal queue.
    if event::poll(Duration::ZERO)? {
        return Ok(PollReady::Ready);
    }
    #[cfg(unix)]
    {
        if cancel.load(Ordering::SeqCst) || tty_disconnected() {
            return Ok(PollReady::Disconnected);
        }
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(if cancel.load(Ordering::SeqCst) || tty_disconnected() {
                    PollReady::Disconnected
                } else {
                    PollReady::Timeout
                });
            }
            return Err(err);
        }
        if rc == 0 {
            return Ok(if cancel.load(Ordering::SeqCst) || tty_disconnected() {
                PollReady::Disconnected
            } else {
                PollReady::Timeout
            });
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 || tty_disconnected()
        {
            return Ok(PollReady::Disconnected);
        }
        if event::poll(Duration::ZERO)? {
            Ok(PollReady::Ready)
        } else {
            Ok(PollReady::Timeout)
        }
    }
    #[cfg(not(unix))]
    {
        if event::poll(timeout)? {
            Ok(PollReady::Ready)
        } else {
            Ok(PollReady::Timeout)
        }
    }
}

/// Zero-CPU kernel-driven PTY hangup guard (`poll(STDIN, events=0)`).
///
/// Even if the PTY closes in the narrow window while the main thread is inside a third-party
/// terminal query (`terminal-colorsaurus`, `cursor::position`, or `crossterm::event::read`),
/// this thread wakes immediately on `POLLHUP`, flips `cancel`, waits for clean shutdown (giving
/// any active transactional worker time to roll back), and forces `_exit(129)` if the main thread
/// remains wedged inside an upstream busy-spin.
#[cfg(unix)]
struct HangupWatchdog {
    stop_write: libc::c_int,
    handle: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl HangupWatchdog {
    fn start(cancel: Arc<AtomicBool>, worker_active: Arc<AtomicBool>) -> Option<Self> {
        let mut fds = [0 as libc::c_int; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        for fd in fds {
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
        }
        let stop_read = fds[0];
        let stop_write = fds[1];
        let handle = thread::spawn(move || {
            let mut poll_fds = [
                libc::pollfd {
                    fd: libc::STDIN_FILENO,
                    events: 0, // Wakes ONLY on POLLHUP | POLLERR | POLLNVAL (zero idle wakeups).
                    revents: 0,
                },
                libc::pollfd {
                    fd: stop_read,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let mut hung_up = false;
            loop {
                let rc = unsafe { libc::poll(poll_fds.as_mut_ptr(), 2, -1) };
                if rc < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if poll_fds[1].revents
                    & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                    != 0
                {
                    break;
                }
                if poll_fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                    hung_up = true;
                    break;
                }
            }
            if hung_up {
                cancel.store(true, Ordering::SeqCst);
                let hangup_at = Instant::now();
                let mut idle_since: Option<Instant> = None;
                let mut stop_pfd = libc::pollfd {
                    fd: stop_read,
                    events: libc::POLLIN,
                    revents: 0,
                };
                loop {
                    let rc = unsafe { libc::poll(&mut stop_pfd, 1, 100) };
                    if rc > 0
                        && stop_pfd.revents
                            & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                            != 0
                    {
                        break;
                    }
                    if worker_active.load(Ordering::SeqCst) {
                        idle_since = None;
                        if hangup_at.elapsed() >= Duration::from_secs(60) {
                            unsafe { libc::_exit(129) };
                        }
                    } else {
                        let started = *idle_since.get_or_insert_with(Instant::now);
                        if started.elapsed() >= Duration::from_millis(1500) {
                            unsafe { libc::_exit(129) };
                        }
                    }
                }
            }
            unsafe {
                libc::close(stop_read);
            }
        });
        Some(Self {
            stop_write,
            handle: Some(handle),
        })
    }
}

#[cfg(unix)]
impl Drop for HangupWatchdog {
    fn drop(&mut self) {
        let byte = [1u8; 1];
        unsafe {
            let _ = libc::write(self.stop_write, byte.as_ptr().cast(), 1);
            libc::close(self.stop_write);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(unix))]
struct HangupWatchdog;

#[cfg(not(unix))]
impl HangupWatchdog {
    fn start(_cancel: Arc<AtomicBool>, _worker_active: Arc<AtomicBool>) -> Option<Self> {
        None
    }
}

#[cfg(not(unix))]
impl Drop for HangupWatchdog {
    fn drop(&mut self) {}
}

fn flush_tty_input() {
    #[cfg(unix)]
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

fn drain_terminal_events() {
    for _ in 0..MAX_DRAIN_EVENTS {
        if tty_disconnected() {
            return;
        }
        if !event::poll(Duration::ZERO).unwrap_or(false) {
            break;
        }
        if tty_disconnected() || event::read().is_err() {
            break;
        }
    }
    flush_tty_input();
}

fn disable_raw_mode_retry() -> io::Result<()> {
    loop {
        match terminal::disable_raw_mode() {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

fn disable_mouse_capture() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture).and_then(|()| stdout.flush())
}

fn enter() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(err) = execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )
    .and_then(|()| stdout.flush())
    {
        let _ = leave();
        return Err(err);
    }
    Ok(())
}

fn leave() -> io::Result<()> {
    if tty_disconnected() {
        let _ = disable_raw_mode_retry();
        return Ok(());
    }
    // Disable mouse capture and leave the alternate screen while raw mode (ECHO off)
    // is still active so in-flight mouse packets are never echoed. DisableMouseCapture
    // is emitted both before and after LeaveAlternateScreen for terminals that track
    // DEC mouse modes per screen buffer.
    let mut stdout = io::stdout();
    let screen_res = execute!(
        stdout,
        DisableMouseCapture,
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    )
    .and_then(|()| stdout.flush());
    drain_terminal_events();
    let raw_res = disable_raw_mode_retry();
    flush_tty_input();
    screen_res.and(raw_res)
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = leave();
    }
}

/// Restores raw mode, alternate screen, and mouse capture even if an external I/O operation fails.
struct ExternalSessionGuard<'a> {
    terminal: &'a mut DefaultTerminal,
}

impl Drop for ExternalSessionGuard<'_> {
    fn drop(&mut self) {
        if tty_disconnected() {
            return;
        }
        let _ = enter();
        drain_terminal_events();
        if !tty_disconnected() {
            let _ = self.terminal.clear();
        }
    }
}

fn is_coalescible_input(input: &Input) -> bool {
    match input {
        Input::Mouse(mouse) => {
            matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
        }
        Input::Resize(_, _) | Input::FocusGained | Input::FocusLost => true,
        Input::Key(key) => key.kind != KeyEventKind::Press,
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

fn dispatch_input(app: &mut App, input: Input) -> Result<Effect> {
    match input {
        Input::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        Input::Mouse(mouse) => app.handle_mouse(mouse, app.viewport),
        Input::Resize(cols, rows) => {
            app.viewport = ratatui::layout::Rect::new(0, 0, cols, rows);
            if !layout::Breakpoint::of(cols).uses_overlay_sidebar()
                && !app.sidebar_collapsed
                && !(app.is_docker_view() && app.docker.fullscreen)
                && !(app.is_clash_view() && app.clash.fullscreen)
            {
                app.sidebar_overlay = false;
            }
            Ok(Effect::None)
        }
        _ => Ok(Effect::None),
    }
}

fn drain_input_batch<I>(
    app: &mut App,
    first: Input,
    mut next_ready: I,
) -> io::Result<Result<Effect>>
where
    I: FnMut() -> io::Result<Option<Input>>,
{
    let mut current = first;
    for step in 0..MAX_COALESCED_EVENTS {
        let coalescible = is_coalescible_input(&current);
        let effect = dispatch_input(app, current);
        if !coalescible || !matches!(effect, Ok(Effect::None)) || step + 1 == MAX_COALESCED_EVENTS {
            return Ok(effect);
        }
        let Some(next) = next_ready()? else {
            return Ok(Ok(Effect::None));
        };
        current = next;
    }
    Ok(Ok(Effect::None))
}

/// Borderless, unwrapped pure-text copy screen inside the Alternate Screen.
/// Mouse capture is disabled while this screen is active so native terminal
/// mouse drag-selection works across any number of lines with ZERO `│` borders
/// and ZERO trailing padding spaces.
fn run_plain_view(
    terminal: &mut DefaultTerminal,
    title: &str,
    content: &str,
    i18n: &crate::i18n::I18n,
    cancel: &AtomicBool,
) -> io::Result<()> {
    struct MouseRestoreGuard;
    impl Drop for MouseRestoreGuard {
        fn drop(&mut self) {
            if tty_disconnected() {
                return;
            }
            drain_terminal_events();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, EnableMouseCapture, crossterm::cursor::Hide)
                .and_then(|()| stdout.flush());
        }
    }

    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, crossterm::cursor::Hide)?;
    stdout.flush()?;
    drain_terminal_events();
    let _restore = MouseRestoreGuard;

    let lines: Vec<&str> = if content.is_empty() {
        vec![""]
    } else {
        content.lines().collect()
    };
    let mut top = 0usize;
    let mut copied = false;

    loop {
        let (cols, rows) = terminal::size().unwrap_or((120, 36));
        let cols = usize::from(cols.max(20));
        let body_rows = usize::from(rows.saturating_sub(2)).max(1);

        // Compute how many logical lines fit without overflowing the screen height.
        let mut visible: Vec<&str> = Vec::new();
        let mut used_rows = 0usize;
        for line in lines.iter().skip(top) {
            let w = unicode_width::UnicodeWidthStr::width(*line);
            let needed = w.div_ceil(cols).max(1);
            if !visible.is_empty() && used_rows + needed > body_rows {
                break;
            }
            visible.push(line);
            used_rows += needed;
            if used_rows >= body_rows {
                break;
            }
        }

        let mut out = io::stdout();
        // Reset attributes, clear screen, move to top-left.
        write!(out, "\x1b[0m\x1b[2J\x1b[H")?;
        let status = if copied {
            i18n.text("toast.copied_text")
        } else {
            i18n.text("ui.plain_view_hint")
        };
        let header = format!("-- {title} ({}/{}) | {status} --", top + 1, lines.len());
        let clipped_header = render::clip_ascii_safe(&header, cols, false);
        // Dim header line (no box-drawing borders anywhere).
        write!(out, "\x1b[7m{clipped_header}\x1b[0m\r\n")?;
        for line in &visible {
            write!(out, "{line}\r\n")?;
        }
        out.flush()?;

        match poll_terminal_event(Duration::from_millis(250), cancel)? {
            PollReady::Timeout => continue,
            PollReady::Disconnected => break,
            PollReady::Ready => {}
        }
        if let Input::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            use crossterm::event::{KeyCode, KeyModifiers};
            let max_top = lines.len().saturating_sub(1);
            match key.code {
                KeyCode::Esc
                | KeyCode::Enter
                | KeyCode::Char('q' | 'Q' | 'v' | 'V' | 'm' | 'M') => break,
                KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('c' | 'C' | 'y' | 'Y') => {
                    let _ = clipboard::copy(content);
                    copied = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    top = top.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    top = (top + 1).min(max_top);
                }
                KeyCode::PageUp => {
                    top = top.saturating_sub(body_rows.saturating_sub(1).max(1));
                }
                KeyCode::PageDown => {
                    top = (top + body_rows.saturating_sub(1).max(1)).min(max_top);
                }
                KeyCode::Home => top = 0,
                KeyCode::End => {
                    let mut acc = 0usize;
                    let mut idx = lines.len();
                    while idx > 0 {
                        let w = unicode_width::UnicodeWidthStr::width(lines[idx - 1]);
                        let needed = w.div_ceil(cols).max(1);
                        if idx < lines.len() && acc + needed > body_rows {
                            break;
                        }
                        idx -= 1;
                        acc += needed;
                        if acc >= body_rows {
                            break;
                        }
                    }
                    top = idx;
                }
                _ => {}
            }
        }
    }

    drop(_restore);
    if !tty_disconnected() {
        terminal.clear()?;
    }
    Ok(())
}

impl App {
    fn start_task(&mut self, terminal: &mut DefaultTerminal, task: Task) -> Result<()> {
        if self.worker.is_some() {
            return Err(Error::new("error.busy", ""));
        }
        let jobs = task.jobs();
        for (spec, params) in &jobs {
            self.available(spec)?;
            spec.parameters(params)?;
        }
        self.ctx.cancel.store(false, Ordering::SeqCst);
        self.logs.clear();
        self.log_scroll = 0;
        self.logs_open = true;
        self.phase = Phase::Running;
        self.record_start(&task);

        if jobs.iter().any(|(spec, _)| spec.external) {
            leave()?;
            let io_res = {
                let _restore = ExternalSessionGuard { terminal };
                println!("{}", self.ctx.i18n.text("ui.external"));
                let report = perform(&self.ctx, &task);
                self.record_report(&report);
                if let Err(error) = &report.result {
                    eprintln!("{}", self.ctx.i18n.error(error));
                }
                println!("{}", self.ctx.i18n.text("ui.return"));
                let flush_res = io::stdout().flush();
                let mut line = String::new();
                let read_res = io::stdin().read_line(&mut line);
                self.finish(report.result);
                flush_res.and(read_res.map(|_| ()))
            };
            io_res?;
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
}

pub fn run(
    ctx: Context,
    theme_flag: Option<&str>,
    style_flag: Option<&str>,
    preferences: &Preferences,
) -> Result<()> {
    run_with_target(
        ctx,
        theme_flag,
        style_flag,
        preferences,
        LaunchTarget::Overview,
    )
}

pub fn run_with_target(
    ctx: Context,
    theme_flag: Option<&str>,
    style_flag: Option<&str>,
    preferences: &Preferences,
    target: LaunchTarget,
) -> Result<()> {
    let category_order = category_order_from_ids(&preferences.category_order)?;
    let worker_active = Arc::new(AtomicBool::new(false));
    let _hangup_watchdog = HangupWatchdog::start(ctx.cancel.clone(), worker_active.clone());
    if ctx.cancel.load(Ordering::SeqCst) || tty_disconnected() {
        return Ok(());
    }
    let background = detect::detect(preferences.detect_background.unwrap_or(true));
    if ctx.cancel.load(Ordering::SeqCst) || tty_disconnected() {
        return Ok(());
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        previous(info);
    }));

    enter()?;
    let _guard = TerminalGuard;

    // Drain any residual terminal query replies before entering the interactive loop.
    drain_terminal_events();
    if ctx.cancel.load(Ordering::SeqCst) || tty_disconnected() {
        return Ok(());
    }

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    let bg_ctx = ctx.silent();
    let monitor = Monitor::start(&bg_ctx);
    let docker_monitor = DockerMonitor::start(&bg_ctx);
    let clash_monitor = ClashMonitor::start(&bg_ctx);
    let mut log_streamer: Option<LogStreamer> = None;
    let mut was_in_docker = false;
    let mut last_docker_tab = DockerTab::Containers;
    let mut last_stats_on = false;
    let mut was_in_clash = false;
    let mut last_clash_tab = ClashTab::Overview;
    let mut last_clash_log_stream = false;

    let mut app = App::new(ctx);
    app.configure_theme(
        theme_flag,
        std::env::var("CQS_THEME").ok().as_deref(),
        style_flag,
        std::env::var("CQS_STYLE").ok().as_deref(),
        background,
        preferences,
    );
    app.set_category_order(category_order);
    match target {
        LaunchTarget::Overview => {}
        LaunchTarget::Docker { tab, fullscreen } => {
            app.open_docker(tab, fullscreen, true);
        }
        LaunchTarget::Clash { tab, fullscreen } => {
            app.open_clash(tab, fullscreen, true);
        }
    }

    let mut snapshot_gen = 0u64;
    let mut dirty = true;
    let mut last_draw = Instant::now();
    let result = (|| {
        loop {
            if app.receive() {
                dirty = true;
            }
            if let Some(snap) = monitor.snapshot_if_changed(&mut snapshot_gen)? {
                app.snapshot = snap;
                dirty = true;
            }

            // Drain Docker background monitor & log stream events
            for evt in docker_monitor.drain() {
                app.docker.apply_event(evt);
                dirty = true;
            }
            // Drain Mihomo background monitor events
            for evt in clash_monitor.drain() {
                let toast = app.clash.apply_event(evt, &app.ctx.i18n);
                if let Some((msg, tone)) = toast {
                    app.toast(msg, tone);
                }
                dirty = true;
            }
            if let Some(streamer) = &log_streamer {
                for evt in streamer.drain() {
                    app.docker.apply_log_event(evt);
                    dirty = true;
                }
            }

            if app.completed_since_refresh {
                monitor.refresh();
                app.docker.invalidate_inspects();
                if app.is_docker_view() {
                    docker_monitor.send(MonitorCmd::ForceRefresh);
                } else if app.docker.active_ever {
                    docker_monitor.send(MonitorCmd::RefreshOnce);
                }
                if app.is_clash_view() {
                    clash_monitor.send(ClashMonitorCmd::ForceRefresh);
                } else if app.clash.active_ever {
                    clash_monitor.send(ClashMonitorCmd::RefreshOnce);
                }
                app.completed_since_refresh = false;
                dirty = true;
            }

            // Sync lazy Mihomo monitor wake/sleep and tab switches
            let in_clash = app.is_clash_view();
            if in_clash {
                if !was_in_clash {
                    clash_monitor.send(ClashMonitorCmd::SetPinnedServices(
                        app.clash.pinned_service_targets(4),
                    ));
                    clash_monitor.send(ClashMonitorCmd::Wake(app.clash.tab));
                    last_clash_tab = app.clash.tab;
                } else if app.clash.tab != last_clash_tab {
                    clash_monitor.send(ClashMonitorCmd::SetTab(app.clash.tab));
                    last_clash_tab = app.clash.tab;
                }
            } else if was_in_clash {
                clash_monitor.send(ClashMonitorCmd::LeaveClash);
            }
            let clash_log_stream =
                in_clash && app.clash.tab == ClashTab::Tools && app.clash.tools_log_mode;
            if clash_log_stream != last_clash_log_stream {
                clash_monitor.send(ClashMonitorCmd::SetLogStream(clash_log_stream));
                last_clash_log_stream = clash_log_stream;
            }
            was_in_clash = in_clash;

            // Sync lazy Docker monitor wake/sleep, tab switches, stats, debounced inspect, and log stream
            let in_docker = app.is_docker_view();
            if in_docker {
                if !was_in_docker {
                    docker_monitor.send(MonitorCmd::Wake(app.docker.tab));
                    last_docker_tab = app.docker.tab;
                } else if app.docker.tab != last_docker_tab {
                    docker_monitor.send(MonitorCmd::SetTab(app.docker.tab));
                    last_docker_tab = app.docker.tab;
                }
                let stats_on = app.docker.stats_needed();
                if stats_on != last_stats_on {
                    docker_monitor.send(MonitorCmd::EnableStats(stats_on));
                    last_stats_on = stats_on;
                }
                sync_docker_inspect(&mut app, &docker_monitor);
            } else if was_in_docker {
                docker_monitor.send(MonitorCmd::LeaveDocker);
                if last_stats_on {
                    docker_monitor.send(MonitorCmd::EnableStats(false));
                    last_stats_on = false;
                }
            }
            was_in_docker = in_docker;

            let desired_logs = if in_docker {
                app.docker.desired_log_target()
            } else {
                None
            };
            if log_streamer.as_ref().map(|s| &s.target) != desired_logs.as_ref() {
                if let Some(old) = log_streamer.take() {
                    old.stop();
                }
                app.docker.logs.reset_for(desired_logs.clone());
                if let Some(target) = desired_logs {
                    log_streamer = Some(LogStreamer::start(&bg_ctx, target, 200));
                }
                dirty = true;
            }

            worker_active.store(app.worker.is_some(), Ordering::SeqCst);
            if (app.exit_requested || app.ctx.cancel.load(Ordering::SeqCst)) && app.worker.is_none()
            {
                break;
            }
            if tty_disconnected() {
                break;
            }
            if let Ok(sz) = terminal.size() {
                let next_vp = ratatui::layout::Rect::new(0, 0, sz.width, sz.height);
                if next_vp != app.viewport {
                    app.viewport = next_vp;
                    dirty = true;
                }
            }
            let now = Instant::now();
            let had_toast = app.toast.is_some();
            app.tick(now);
            if had_toast && app.toast.is_none() {
                dirty = true;
            }
            // Skip redundant full-frame redraws when idle with no animations, spinners, or state
            // changes; refresh relative age readouts at most once per second.
            let age_tick = (app.nav == state::NavTarget::Overview
                || app.is_docker_view()
                || app.is_clash_view())
                && now.saturating_duration_since(last_draw) >= Duration::from_secs(1);
            if dirty || age_tick || app.needs_continuous_redraw() {
                if let Err(err) = terminal.draw(|frame| app.draw(frame)) {
                    if tty_disconnected() || app.ctx.cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    return Err(err.into());
                }
                dirty = false;
                last_draw = now;
            }

            match poll_terminal_event(app.frame_interval(), &app.ctx.cancel)? {
                PollReady::Timeout => continue,
                PollReady::Disconnected => break,
                PollReady::Ready => {}
            }
            dirty = true;
            let first = event::read()?;
            let cancel_flag = app.ctx.cancel.clone();
            let effect = drain_input_batch(&mut app, first, || {
                if tty_disconnected() || cancel_flag.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                if event::poll(Duration::ZERO)? {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            })?;
            let outcome = match effect {
                Ok(Effect::Quit) => break,
                Ok(Effect::Run(task)) => {
                    let res = app.start_task(&mut terminal, task);
                    worker_active.store(app.worker.is_some(), Ordering::SeqCst);
                    res
                }
                Ok(Effect::Refresh) => {
                    app.refreshing = Some(Instant::now());
                    monitor.refresh();
                    if app.is_docker_view() {
                        app.docker.invalidate_inspects();
                        docker_monitor.send(MonitorCmd::ForceRefresh);
                    }
                    if app.is_clash_view() {
                        clash_monitor.send(ClashMonitorCmd::ForceRefresh);
                    }
                    Ok(())
                }
                Ok(Effect::DockerRefresh) => {
                    app.refreshing = Some(Instant::now());
                    monitor.refresh();
                    app.docker.invalidate_inspects();
                    docker_monitor.send(MonitorCmd::ForceRefresh);
                    Ok(())
                }
                Ok(Effect::ClashRefresh) => {
                    app.refreshing = Some(Instant::now());
                    monitor.refresh();
                    clash_monitor.send(ClashMonitorCmd::ForceRefresh);
                    Ok(())
                }
                Ok(Effect::ClashCmd(cmd)) => {
                    clash_monitor.send(cmd);
                    Ok(())
                }
                Ok(Effect::DockerExec(spec)) => {
                    if let Some(old) = log_streamer.take() {
                        old.stop();
                    }
                    leave()?;
                    let exec_res = {
                        let _restore = ExternalSessionGuard {
                            terminal: &mut terminal,
                        };
                        docker_actions::run_interactive_exec(&app.ctx, &spec)
                    };
                    app.docker.invalidate_inspects();
                    docker_monitor.send(MonitorCmd::ForceRefresh);
                    exec_res
                }
                Ok(Effect::PublicIp) => monitor.query_public(),
                Ok(Effect::Copy(text)) => clipboard::copy(&text).map(|_| ()).map_err(Error::from),
                Ok(Effect::PlainView { title, content }) => run_plain_view(
                    &mut terminal,
                    &title,
                    &content,
                    &app.ctx.i18n,
                    &app.ctx.cancel,
                )
                .map_err(Error::from),
                Ok(Effect::None) => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                app.fail_ui(error);
            }
        }
        Ok(())
    })();

    // Stop terminal mouse reporting immediately before joining background workers/monitors,
    // while keeping raw mode (ECHO off) active until TerminalGuard drops last.
    if !tty_disconnected() {
        let _ = disable_mouse_capture();
        drain_terminal_events();
    }

    if let Some(streamer) = log_streamer.take() {
        streamer.stop();
    }
    if let Some(worker) = app.worker.take() {
        app.ctx.cancel.store(true, Ordering::SeqCst);
        drop(app.confirmation.take());
        let join_res = worker
            .join()
            .map_err(|_| Error::new("error.io", "worker panicked"));
        worker_active.store(false, Ordering::SeqCst);
        join_res?;
    } else {
        worker_active.store(false, Ordering::SeqCst);
    }
    drop(app);
    drop(clash_monitor);
    drop(docker_monitor);
    drop(monitor);
    drop(terminal);
    drop(_guard);
    drop(_hangup_watchdog);
    result
}

fn sync_docker_inspect(app: &mut App, monitor: &DockerMonitor) {
    if !app.docker.snapshot.availability.is_ready() {
        return;
    }
    let now = Instant::now();
    let cur_key = app.docker.current_item_key();
    if cur_key != app.docker.last_selected_key {
        app.docker.last_selected_key = cur_key.clone();
        app.docker.pending_inspect = cur_key.map(|k| (now, k));
    }
    let Some((since, key)) = app.docker.pending_inspect.clone() else {
        return;
    };
    if now.saturating_duration_since(since) < docker::state::INSPECT_DEBOUNCE {
        return;
    }
    app.docker.pending_inspect = None;
    match app.docker.tab {
        DockerTab::Containers => {
            if !app.docker.container_inspects.contains_key(&key)
                && let Some(c) = app.docker.selected_container()
            {
                monitor.send(MonitorCmd::FetchContainerInspect {
                    id: c.id.clone(),
                    running: c.state.is_running(),
                });
            }
        }
        DockerTab::Images => {
            if !app.docker.image_inspects.contains_key(&key) {
                monitor.send(MonitorCmd::FetchImageInspect { id: key });
            }
        }
        DockerTab::Compose => {
            if let Some(p) = app.docker.selected_compose()
                && let Some(path) = p.config_files.first()
                && !app.docker.compose_yamls.contains_key(path)
            {
                monitor.send(MonitorCmd::FetchComposeYaml { path: path.clone() });
            }
        }
        _ => {}
    }
}
