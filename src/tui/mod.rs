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
}

use crate::{
    actions,
    config::Preferences,
    dashboard::Monitor,
    error::{Error, Result},
    ops::docker::{
        actions as docker_actions,
        model::DockerTab,
        monitor::{DockerMonitor, LogStreamer, MonitorCmd},
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
    sync::{atomic::Ordering, mpsc},
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

fn flush_tty_input() {
    #[cfg(unix)]
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

fn drain_terminal_events() {
    for _ in 0..MAX_DRAIN_EVENTS {
        if !event::poll(Duration::ZERO).unwrap_or(false) {
            break;
        }
        if event::read().is_err() {
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
        let _ = enter();
        drain_terminal_events();
        let _ = self.terminal.clear();
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
) -> io::Result<()> {
    struct MouseRestoreGuard;
    impl Drop for MouseRestoreGuard {
        fn drop(&mut self) {
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

        if !event::poll(Duration::from_millis(250))? {
            continue;
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
    terminal.clear()?;
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
    let background = detect::detect(preferences.detect_background.unwrap_or(true));

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave();
        previous(info);
    }));

    enter()?;
    let _guard = TerminalGuard;

    // Drain any residual terminal query replies before entering the interactive loop.
    drain_terminal_events();

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    let monitor = Monitor::start(&ctx);
    let docker_monitor = DockerMonitor::start(&ctx);
    let mut log_streamer: Option<LogStreamer> = None;
    let mut was_in_docker = false;
    let mut last_docker_tab = DockerTab::Containers;
    let mut last_stats_on = false;

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
    if let LaunchTarget::Docker { tab, fullscreen } = target {
        app.open_docker(tab, fullscreen, true);
    }

    let result = (|| {
        loop {
            app.receive();
            app.snapshot = monitor.snapshot()?;

            // Drain Docker background monitor & log stream events
            for evt in docker_monitor.drain() {
                app.docker.apply_event(evt);
            }
            if let Some(streamer) = &log_streamer {
                for evt in streamer.drain() {
                    app.docker.apply_log_event(evt);
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
                app.completed_since_refresh = false;
            }

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
                    log_streamer = Some(LogStreamer::start(&app.ctx, target, 200));
                }
            }

            if (app.exit_requested || app.ctx.cancel.load(Ordering::SeqCst)) && app.worker.is_none()
            {
                break;
            }
            if let Ok(sz) = terminal.size() {
                app.viewport = ratatui::layout::Rect::new(0, 0, sz.width, sz.height);
            }
            app.tick(Instant::now());
            terminal.draw(|frame| app.draw(frame))?;

            if !event::poll(app.frame_interval())? {
                continue;
            }
            let first = event::read()?;
            let effect = drain_input_batch(&mut app, first, || {
                if event::poll(Duration::ZERO)? {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            })?;
            let outcome = match effect {
                Ok(Effect::Quit) => break,
                Ok(Effect::Run(task)) => app.start_task(&mut terminal, task),
                Ok(Effect::Refresh) => {
                    app.refreshing = Some(Instant::now());
                    monitor.refresh();
                    if app.is_docker_view() {
                        app.docker.invalidate_inspects();
                        docker_monitor.send(MonitorCmd::ForceRefresh);
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
                Ok(Effect::PlainView { title, content }) => {
                    run_plain_view(&mut terminal, &title, &content, &app.ctx.i18n)
                        .map_err(Error::from)
                }
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
    let _ = disable_mouse_capture();
    drain_terminal_events();

    if let Some(streamer) = log_streamer.take() {
        streamer.stop();
    }
    if let Some(worker) = app.worker.take() {
        app.ctx.cancel.store(true, Ordering::SeqCst);
        drop(app.confirmation.take());
        worker
            .join()
            .map_err(|_| Error::new("error.io", "worker panicked"))?;
    }
    drop(app);
    drop(docker_monitor);
    drop(monitor);
    drop(terminal);
    drop(_guard);
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
