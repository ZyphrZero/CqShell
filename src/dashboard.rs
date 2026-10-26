//! Read-only, independently refreshed dashboard data. No system-management action runs here.
use crate::{
    config::Paths,
    error::{Error, Result},
    runtime::Context,
};
use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use sysinfo::{Disks, System};

#[derive(Debug, Clone, PartialEq)]
pub enum Reading<T> {
    Loading,
    NotQueried,
    Value(T),
    Unavailable(String),
    NotApplicable,
}
impl<T> Reading<T> {
    pub fn from_result(result: std::result::Result<T, impl ToString>) -> Self {
        match result {
            Ok(value) => Self::Value(value),
            Err(error) => Self::Unavailable(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}
impl Usage {
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalSnapshot {
    pub host: Reading<String>,
    pub os: Reading<String>,
    pub kernel: Reading<String>,
    pub arch: String,
    pub cpu: Reading<String>,
    pub cpu_usage: Reading<f32>,
    pub memory: Reading<Usage>,
    pub swap: Reading<Usage>,
    pub disk: Reading<Usage>,
    pub disk_name: String,
    pub uptime: Reading<u64>,
    pub timezone: Reading<String>,
    pub congestion: Reading<String>,
    pub qdisc: Reading<String>,
    pub bbr_available: Reading<bool>,
    pub bbr_version: Reading<String>,
    pub tcp_configured: Reading<bool>,
    pub updated: Option<Instant>,
}
impl Default for LocalSnapshot {
    fn default() -> Self {
        Self {
            host: Reading::Loading,
            os: Reading::Loading,
            kernel: Reading::Loading,
            arch: std::env::consts::ARCH.into(),
            cpu: Reading::Loading,
            cpu_usage: Reading::Loading,
            memory: Reading::Loading,
            swap: Reading::Loading,
            disk: Reading::Loading,
            disk_name: String::new(),
            uptime: Reading::Loading,
            timezone: Reading::Loading,
            congestion: Reading::Loading,
            qdisc: Reading::Loading,
            bbr_available: Reading::Loading,
            bbr_version: Reading::Loading,
            tcp_configured: Reading::Loading,
            updated: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublicSnapshot {
    pub ipv4: Reading<String>,
    pub ipv6: Reading<String>,
    pub isp: Reading<String>,
    pub updated: Option<Instant>,
}
impl Default for PublicSnapshot {
    fn default() -> Self {
        Self {
            ipv4: Reading::NotQueried,
            ipv6: Reading::NotQueried,
            isp: Reading::NotQueried,
            updated: None,
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub local: LocalSnapshot,
    pub public: PublicSnapshot,
}

pub struct Sampler {
    system: System,
    disks: Disks,
    first_sample: Option<Instant>,
}
impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}
impl Sampler {
    pub fn new() -> Self {
        Self {
            system: System::new(),
            disks: Disks::new_with_refreshed_list(),
            first_sample: None,
        }
    }
    pub fn sample(&mut self, paths: &Paths) -> LocalSnapshot {
        self.system.refresh_cpu_all();
        self.system.refresh_memory();
        self.disks.refresh(false);
        let now = Instant::now();
        let ready = self
            .first_sample
            .is_some_and(|at| at.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        self.first_sample.get_or_insert(now);
        let cpus = self.system.cpus();
        let cpu = if cpus.is_empty() {
            Reading::Unavailable("CPU information unavailable".into())
        } else {
            Reading::Value(format!("{} × {}", cpus[0].brand().trim(), cpus.len()))
        };
        let cpu_usage = if !ready {
            Reading::Loading
        } else if cpus.is_empty() {
            Reading::Unavailable("CPU sample unavailable".into())
        } else {
            Reading::Value(self.system.global_cpu_usage())
        };
        let root = if cfg!(windows) {
            std::env::var_os("SystemDrive").map(|mut drive| {
                drive.push("\\");
                std::path::PathBuf::from(drive)
            })
        } else {
            Some(std::path::PathBuf::from("/"))
        };
        let disk = root.as_ref().and_then(|root| {
            self.disks.iter().find(|d| {
                d.mount_point()
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&root.as_os_str().to_string_lossy())
            })
        });
        let disk_name = disk
            .map(|d| d.mount_point().display().to_string())
            .unwrap_or_default();
        let disk = match disk {
            Some(d) if d.total_space() > 0 && d.available_space() <= d.total_space() => {
                Reading::Value(Usage {
                    used: d.total_space() - d.available_space(),
                    total: d.total_space(),
                })
            }
            _ => Reading::Unavailable("system filesystem statistics unavailable".into()),
        };
        let mut snapshot = LocalSnapshot {
            host: optional(System::host_name(), "hostname"),
            os: optional(System::long_os_version().or_else(System::name), "OS"),
            kernel: optional(System::kernel_version(), "kernel"),
            arch: std::env::consts::ARCH.into(),
            cpu,
            cpu_usage,
            memory: if self.system.total_memory() == 0 {
                Reading::Unavailable("memory statistics unavailable".into())
            } else {
                Reading::Value(Usage {
                    used: self.system.used_memory(),
                    total: self.system.total_memory(),
                })
            },
            swap: Reading::Value(Usage {
                used: self.system.used_swap(),
                total: self.system.total_swap(),
            }),
            disk,
            disk_name,
            uptime: Reading::Value(System::uptime()),
            timezone: read_timezone(),
            congestion: Reading::NotApplicable,
            qdisc: Reading::NotApplicable,
            bbr_available: Reading::NotApplicable,
            bbr_version: Reading::NotApplicable,
            tcp_configured: Reading::NotApplicable,
            updated: Some(now),
        };
        if cfg!(target_os = "linux") {
            fill_linux_network(
                &mut snapshot,
                Path::new("/proc/sys"),
                Path::new("/sys/module/tcp_bbr/version"),
                &paths.tcp,
            );
        }
        snapshot
    }
}

fn optional(value: Option<String>, name: &str) -> Reading<String> {
    match value.filter(|s| !s.trim().is_empty()) {
        Some(value) => Reading::Value(value),
        None => Reading::Unavailable(format!("{name} unavailable")),
    }
}
fn read_text(path: &Path) -> Reading<String> {
    Reading::from_result(fs::read_to_string(path).and_then(|s| {
        if s.trim().is_empty() {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: empty value", path.display()),
            ))
        } else {
            Ok(s.trim().into())
        }
    }))
}
fn read_timezone() -> Reading<String> {
    #[cfg(target_os = "linux")]
    {
        match fs::read_to_string("/etc/timezone") {
            Ok(zone) if !zone.trim().is_empty() => return Reading::Value(zone.trim().into()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Reading::Unavailable(e.to_string());
            }
            _ => (),
        }
        match fs::read_link("/etc/localtime") {
            Ok(path) => path
                .to_string_lossy()
                .split_once("zoneinfo/")
                .map(|(_, value)| Reading::Value(value.into()))
                .unwrap_or_else(|| Reading::Unavailable("timezone name unavailable".into())),
            Err(e) => Reading::Unavailable(e.to_string()),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match std::process::Command::new("tzutil.exe")
            .arg("/g")
            .creation_flags(0x08000000)
            .output()
        {
            Ok(output) if output.status.success() => optional(
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().into()),
                "timezone",
            ),
            Ok(output) => Reading::Unavailable(format!("tzutil: {}", output.status)),
            Err(e) => Reading::Unavailable(e.to_string()),
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    Reading::NotApplicable
}

pub fn fill_linux_network(snapshot: &mut LocalSnapshot, sysctl: &Path, version: &Path, tcp: &Path) {
    snapshot.congestion = read_text(&sysctl.join("net/ipv4/tcp_congestion_control"));
    snapshot.qdisc = read_text(&sysctl.join("net/core/default_qdisc"));
    snapshot.bbr_available =
        match read_text(&sysctl.join("net/ipv4/tcp_available_congestion_control")) {
            Reading::Value(value) => {
                Reading::Value(value.split_whitespace().any(|name| name == "bbr"))
            }
            Reading::Unavailable(error) => Reading::Unavailable(error),
            _ => unreachable!(),
        };
    snapshot.bbr_version = match read_text(version) {
        Reading::Value(version) if ["1", "2", "3"].contains(&version.as_str()) => {
            Reading::Value(format!("v{version}"))
        }
        _ => Reading::Unavailable("running kernel does not expose a recognized BBR version".into()),
    };
    snapshot.tcp_configured = match fs::symlink_metadata(tcp) {
        Ok(meta) if meta.is_file() => Reading::Value(true),
        Ok(_) => Reading::Unavailable(format!("{}: not a regular file", tcp.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Reading::Value(false),
        Err(e) => Reading::Unavailable(e.to_string()),
    };
}

pub fn parse_ip(text: &str, v4: bool) -> Result<String> {
    let ip: std::net::IpAddr = text
        .trim()
        .parse()
        .map_err(|e| Error::new("error.data", e))?;
    if ip.is_ipv4() != v4 {
        return Err(Error::new("error.data", "unexpected IP family"));
    }
    Ok(ip.to_string())
}
fn query_public(ctx: &Context) -> PublicSnapshot {
    let query = |url: &str| {
        ctx.output(
            "curl",
            &[
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "-fSsL",
                "--connect-timeout",
                "3",
                "--max-time",
                "5",
                "--max-filesize",
                "65536",
                url,
            ],
        )
    };
    let ipv4 = Reading::from_result(query("https://ipv4.ip.sb").and_then(|s| parse_ip(&s, true)));
    let ipv6 = Reading::from_result(query("https://ipv6.ip.sb").and_then(|s| parse_ip(&s, false)));
    let isp = Reading::from_result(query("https://ipinfo.io/json").and_then(|s| {
        let data: serde_json::Value = serde_json::from_str(&s)?;
        data["org"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(crate::runtime::sanitize)
            .ok_or_else(|| Error::new("error.data", "ipinfo.org"))
    }));
    PublicSnapshot {
        ipv4,
        ipv6,
        isp,
        updated: Some(Instant::now()),
    }
}

/// Local sampling never waits for public IP requests. Public queries require an explicit UI action.
pub struct Monitor {
    state: Arc<Mutex<Snapshot>>,
    refresh: Sender<()>,
    public: SyncSender<()>,
    stop: Arc<AtomicBool>,
    querying: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}
impl Monitor {
    pub fn start(ctx: &Context) -> Self {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let querying = Arc::new(AtomicBool::new(false));
        let (refresh, receive) = mpsc::channel();
        let (public, public_receive) = mpsc::sync_channel(1);
        let local_state = state.clone();
        let local_stop = stop.clone();
        let paths = ctx.paths.clone();
        let local = thread::spawn(move || {
            let mut sampler = Sampler::new();
            let mut warmup = true;
            while !local_stop.load(Ordering::SeqCst) {
                let value = sampler.sample(&paths);
                let Ok(mut state) = local_state.lock() else {
                    return;
                };
                state.local = value;
                drop(state);
                let wait = if warmup {
                    sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250))
                } else {
                    Duration::from_secs(3)
                };
                warmup = false;
                if wait_for_refresh(&receive, &local_stop, wait).is_err() {
                    break;
                }
            }
        });
        let public_state = state.clone();
        let public_stop = stop.clone();
        let pending = querying.clone();
        let mut probe = ctx.clone();
        probe.cancel = stop.clone();
        probe.events = None;
        let network = thread::spawn(move || {
            while !public_stop.load(Ordering::SeqCst) {
                match public_receive.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) => {
                        let snapshot = query_public(&probe);
                        if let Ok(mut state) = public_state.lock() {
                            state.public = snapshot;
                        } else {
                            return;
                        }
                        pending.store(false, Ordering::SeqCst);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(_) => break,
                }
            }
        });
        Self {
            state,
            refresh,
            public,
            stop,
            querying,
            threads: vec![local, network],
        }
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.state
            .lock()
            .map(|s| s.clone())
            .map_err(|e| Error::new("error.io", e))
    }
    pub fn refresh(&self) {
        let _ = self.refresh.send(());
    }
    pub fn query_public(&self) -> Result<()> {
        if self.querying.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        {
            let mut state = self.state.lock().map_err(|e| Error::new("error.io", e))?;
            state.public = PublicSnapshot {
                ipv4: Reading::Loading,
                ipv6: Reading::Loading,
                isp: Reading::Loading,
                updated: None,
            };
        }
        self.public.try_send(()).map_err(|e| {
            self.querying.store(false, Ordering::SeqCst);
            Error::new("error.io", e)
        })
    }
}
fn wait_for_refresh(
    receiver: &Receiver<()>,
    stop: &AtomicBool,
    wait: Duration,
) -> std::result::Result<(), ()> {
    let until = Instant::now() + wait;
    while Instant::now() < until && !stop.load(Ordering::SeqCst) {
        match receiver.recv_timeout(
            until
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        ) {
            Ok(()) => {
                while receiver.try_recv().is_ok() {}
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(()),
        }
    }
    Ok(())
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.refresh.send(());
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                eprintln!("dashboard worker panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn local_refresh_continues_during_slow_public_query_and_startup_is_offline() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let curl = temp.path().join("curl");
        let marker = temp.path().join("called");
        fs::write(
            &curl,
            "#!/bin/sh\nprintf called > \"$CQS_PROBE_MARKER\"\nsleep 3\nexit 23\n",
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        let mut ctx = Context::new(
            Paths::from_env().unwrap(),
            crate::i18n::I18n::load(Some("en-US"), None).unwrap(),
            false,
        );
        ctx.environment.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", temp.path().display()),
        );
        ctx.environment
            .insert("CQS_PROBE_MARKER".into(), marker.display().to_string());
        let monitor = Monitor::start(&ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = loop {
            let snapshot = monitor.snapshot().unwrap();
            if let Some(at) = snapshot.local.updated {
                break at;
            }
            assert!(Instant::now() < deadline, "local sampler did not respond");
            thread::sleep(Duration::from_millis(20));
        };
        assert!(!marker.exists());
        assert_eq!(monitor.snapshot().unwrap().public.ipv4, Reading::NotQueried);
        monitor.query_public().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = monitor.snapshot().unwrap();
            if snapshot.local.updated.is_some_and(|at| at > first) && marker.exists() {
                assert_eq!(snapshot.public.ipv4, Reading::Loading);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "public query blocked local sampling"
            );
            thread::sleep(Duration::from_millis(20));
        }
        drop(monitor); // Cancels and reaps the test process; no public service was contacted.
    }
    #[test]
    fn initial_snapshot_does_not_invent_measurements_or_query_public_services() {
        let snapshot = Snapshot::default();
        assert_eq!(snapshot.local.cpu_usage, Reading::Loading);
        assert_eq!(snapshot.public.ipv4, Reading::NotQueried);
        assert_eq!(snapshot.public.isp, Reading::NotQueried);
    }
    #[test]
    fn ip_responses_are_validated_and_error_pages_rejected() {
        assert_eq!(parse_ip("203.0.113.7\n", true).unwrap(), "203.0.113.7");
        assert_eq!(parse_ip("2001:db8::1", false).unwrap(), "2001:db8::1");
        assert!(parse_ip("<html>error</html>", true).is_err());
        assert!(parse_ip("203.0.113.7", false).is_err());
    }
    #[test]
    fn network_fields_fail_independently_and_bbr_uses_exact_algorithm_name() {
        let temp = tempfile::tempdir().unwrap();
        let sysctl = temp.path().join("sysctl");
        fs::create_dir_all(sysctl.join("net/ipv4")).unwrap();
        fs::write(sysctl.join("net/ipv4/tcp_congestion_control"), "cubic").unwrap();
        fs::write(
            sysctl.join("net/ipv4/tcp_available_congestion_control"),
            "reno bbr2 cubic",
        )
        .unwrap();
        let mut snapshot = LocalSnapshot::default();
        fill_linux_network(
            &mut snapshot,
            &sysctl,
            &temp.path().join("version"),
            &temp.path().join("tcp.conf"),
        );
        assert_eq!(snapshot.congestion, Reading::Value("cubic".into()));
        assert_eq!(snapshot.bbr_available, Reading::Value(false));
        assert_eq!(snapshot.tcp_configured, Reading::Value(false));
        assert!(matches!(snapshot.qdisc, Reading::Unavailable(_)));
    }
}
