//! Lazy background Docker monitor (`DockerMonitor`) and leak-free streaming log reader
//! (`LogStreamer`) built on `std::thread` + `std::sync::mpsc`.

use super::{
    client,
    model::{
        ContainerInspect, ContainerStats, DockerLogEntry, DockerSnapshot, DockerTab, ImageInspect,
        parse_log_line,
    },
};
use crate::{error::Result, runtime::Context};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL_INTERVAL: Duration = Duration::from_millis(2500);
const STATS_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_SLEEP_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum MonitorCmd {
    Wake(DockerTab),
    SetTab(DockerTab),
    ForceRefresh,
    RefreshOnce,
    EnableStats(bool),
    FetchContainerInspect { id: String, running: bool },
    FetchImageInspect { id: String },
    FetchComposeYaml { path: String },
    LeaveDocker,
    Shutdown,
}

#[derive(Debug)]
pub enum DockerEvent {
    Snapshot(Box<DockerSnapshot>),
    Stats(BTreeMap<String, ContainerStats>),
    ContainerInspect {
        id: String,
        result: Result<Box<ContainerInspect>>,
    },
    ImageInspect {
        id: String,
        result: Result<Box<ImageInspect>>,
    },
    ComposeYaml {
        path: String,
        result: Result<String>,
    },
}

/// Background polling thread for Docker data. Zero subprocesses run while idle outside the Docker
/// workspace; auto-sleeps 10 seconds after leaving the Docker page.
pub struct DockerMonitor {
    tx: Sender<MonitorCmd>,
    rx: Receiver<DockerEvent>,
    handle: Option<JoinHandle<()>>,
}

impl DockerMonitor {
    pub fn start(ctx: &Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<MonitorCmd>();
        let (evt_tx, evt_rx) = mpsc::channel::<DockerEvent>();
        let ctx = ctx.clone();

        let handle = thread::spawn(move || {
            let mut active = false;
            let mut left_at: Option<Instant> = None;
            let mut tab = DockerTab::Containers;
            let mut stats_enabled = false;
            let mut last_snap = DockerSnapshot::default();
            let mut last_poll = Instant::now() - POLL_INTERVAL;
            let mut last_stats = Instant::now() - STATS_INTERVAL;

            loop {
                let wait = if active {
                    Duration::from_millis(120)
                } else {
                    Duration::from_millis(300)
                };

                let mut force = false;
                let mut shutdown = false;

                let first = cmd_rx.recv_timeout(wait);
                let mut batch = Vec::new();
                if let Ok(c) = first {
                    batch.push(c);
                    while let Ok(more) = cmd_rx.try_recv() {
                        batch.push(more);
                    }
                }

                for cmd in batch {
                    match cmd {
                        MonitorCmd::Wake(t) => {
                            let switched = tab != t || !active;
                            active = true;
                            left_at = None;
                            tab = t;
                            if switched {
                                force = true;
                            }
                        }
                        MonitorCmd::SetTab(t) => {
                            if tab != t {
                                tab = t;
                                force = true;
                            }
                        }
                        MonitorCmd::ForceRefresh => {
                            active = true;
                            left_at = None;
                            force = true;
                        }
                        MonitorCmd::RefreshOnce => {
                            let next = client::fetch_snapshot(&ctx, tab, &last_snap);
                            last_snap = next.clone();
                            last_poll = Instant::now();
                            let _ = evt_tx.send(DockerEvent::Snapshot(Box::new(next)));
                        }
                        MonitorCmd::EnableStats(on) => {
                            if on && !stats_enabled {
                                last_stats = Instant::now() - STATS_INTERVAL;
                            }
                            stats_enabled = on;
                        }
                        MonitorCmd::FetchContainerInspect { id, running } => {
                            let result =
                                client::fetch_container_inspect(&ctx, &id, running).map(Box::new);
                            let _ = evt_tx.send(DockerEvent::ContainerInspect { id, result });
                        }
                        MonitorCmd::FetchImageInspect { id } => {
                            let result = client::fetch_image_inspect(&ctx, &id).map(Box::new);
                            let _ = evt_tx.send(DockerEvent::ImageInspect { id, result });
                        }
                        MonitorCmd::FetchComposeYaml { path } => {
                            let result = client::read_compose_file(&path);
                            let _ = evt_tx.send(DockerEvent::ComposeYaml { path, result });
                        }
                        MonitorCmd::LeaveDocker => {
                            if left_at.is_none() {
                                left_at = Some(Instant::now());
                            }
                        }
                        MonitorCmd::Shutdown => {
                            shutdown = true;
                        }
                    }
                }
                if shutdown {
                    break;
                }

                if let Some(at) = left_at
                    && at.elapsed() >= IDLE_SLEEP_AFTER
                {
                    active = false;
                    left_at = None;
                }

                if !active {
                    continue;
                }

                let now = Instant::now();
                if force || now.duration_since(last_poll) >= POLL_INTERVAL {
                    let next = client::fetch_snapshot(&ctx, tab, &last_snap);
                    last_snap = next.clone();
                    last_poll = Instant::now();
                    let _ = evt_tx.send(DockerEvent::Snapshot(Box::new(next)));
                }

                if stats_enabled
                    && last_snap.availability.is_ready()
                    && last_snap.running_containers() > 0
                    && Instant::now().duration_since(last_stats) >= STATS_INTERVAL
                {
                    last_stats = Instant::now();
                    if let Ok(stats) = client::fetch_stats(&ctx) {
                        let _ = evt_tx.send(DockerEvent::Stats(stats));
                    }
                }
            }
        });

        Self {
            tx: cmd_tx,
            rx: evt_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: MonitorCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn drain(&self) -> Vec<DockerEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            out.push(evt);
        }
        out
    }
}

impl Drop for DockerMonitor {
    fn drop(&mut self) {
        let _ = self.tx.send(MonitorCmd::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Target for an active `docker logs -f` or `docker compose logs -f` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    Container {
        id: String,
    },
    Compose {
        project: String,
        config_files: Vec<String>,
    },
}

#[derive(Debug)]
pub enum LogStreamEvent {
    Batch(Vec<DockerLogEntry>),
    Ended(Option<String>),
}

/// Manages a single streaming `docker logs -f` child process with guaranteed process-group cleanup.
pub struct LogStreamer {
    pub target: LogTarget,
    stop: Arc<AtomicBool>,
    rx: Receiver<LogStreamEvent>,
    handle: Option<JoinHandle<()>>,
}

impl LogStreamer {
    pub fn start(ctx: &Context, target: LogTarget, tail: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let (tx, rx) = mpsc::channel::<LogStreamEvent>();
        let ctx = ctx.clone();
        let target_clone = target.clone();

        let handle = thread::spawn(move || {
            let valid_token =
                |s: &str| !s.is_empty() && !s.starts_with('-') && !s.chars().any(char::is_control);
            let args: Vec<String> = match &target_clone {
                LogTarget::Container { id } => {
                    if !valid_token(id) {
                        let _ = tx.send(LogStreamEvent::Ended(Some("invalid container id".into())));
                        return;
                    }
                    vec![
                        "logs".into(),
                        "--tail".into(),
                        tail.to_string(),
                        "--timestamps".into(),
                        "--follow".into(),
                        id.clone(),
                    ]
                }
                LogTarget::Compose {
                    project,
                    config_files,
                } => {
                    if !valid_token(project) || config_files.iter().any(|f| !valid_token(f)) {
                        let _ = tx.send(LogStreamEvent::Ended(Some(
                            "invalid compose log target".into(),
                        )));
                        return;
                    }
                    let mut a = vec!["compose".into(), "-p".into(), project.clone()];
                    for f in config_files {
                        a.push("-f".into());
                        a.push(f.clone());
                    }
                    a.extend([
                        "logs".into(),
                        "--tail".into(),
                        tail.to_string(),
                        "--timestamps".into(),
                        "--follow".into(),
                        "--no-color".into(),
                    ]);
                    a
                }
            };

            let mut cmd = Command::new("docker");
            cmd.args(&args)
                .envs(&ctx.environment)
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                unsafe {
                    cmd.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }

            let mut child = match crate::runtime::retry_executable_busy(&ctx, || cmd.spawn()) {
                Ok(c) => c,
                Err(err) => {
                    let _ = tx.send(LogStreamEvent::Ended(Some(err.to_string())));
                    return;
                }
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let (line_tx, line_rx) = mpsc::channel::<DockerLogEntry>();

            let spawn_reader = |pipe: Option<Box<dyn std::io::Read + Send>>,
                                ltx: Sender<DockerLogEntry>,
                                flag: Arc<AtomicBool>| {
                thread::spawn(move || {
                    let Some(pipe) = pipe else { return };
                    let mut reader = BufReader::new(pipe);
                    let mut line = String::new();
                    loop {
                        if flag.load(Ordering::SeqCst) {
                            break;
                        }
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) => break,
                            Ok(_) => {
                                let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
                                if ltx.send(parse_log_line(trimmed)).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                })
            };

            let r1 = spawn_reader(
                stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
                line_tx.clone(),
                stop_flag.clone(),
            );
            let r2 = spawn_reader(
                stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
                line_tx,
                stop_flag.clone(),
            );

            let mut exited = None;
            loop {
                if stop_flag.load(Ordering::SeqCst) {
                    terminate_stream_child(&mut child);
                    let _ = child.wait();
                    break;
                }
                let mut batch = Vec::new();
                if let Ok(first) = line_rx.recv_timeout(Duration::from_millis(60)) {
                    batch.push(first);
                    while batch.len() < 128 {
                        match line_rx.try_recv() {
                            Ok(entry) => batch.push(entry),
                            Err(_) => break,
                        }
                    }
                    if tx.send(LogStreamEvent::Batch(batch)).is_err() {
                        terminate_stream_child(&mut child);
                        let _ = child.wait();
                        break;
                    }
                }
                if let Ok(Some(status)) = child.try_wait() {
                    exited = Some(status);
                    break;
                }
            }
            let _ = r1.join();
            let _ = r2.join();
            if let Some(status) = exited {
                let mut tail_batch = Vec::new();
                while let Ok(entry) = line_rx.try_recv() {
                    tail_batch.push(entry);
                }
                if !tail_batch.is_empty() {
                    let _ = tx.send(LogStreamEvent::Batch(tail_batch));
                }
                let diag = (!status.success()).then(|| format!("docker logs exited with {status}"));
                let _ = tx.send(LogStreamEvent::Ended(diag));
            }
        });

        Self {
            target,
            stop,
            rx,
            handle: Some(handle),
        }
    }

    pub fn drain(&self) -> Vec<LogStreamEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            out.push(evt);
        }
        out
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LogStreamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn terminate_stream_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            thread::sleep(Duration::from_millis(40));
            if child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
    let _ = child.kill();
}
