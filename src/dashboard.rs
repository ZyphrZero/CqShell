//! Read-only, independently refreshed dashboard data. No system-management action runs here.
use crate::{
    error::{Error, Result},
    ops::{
        clash::{self, State},
        memory,
    },
    runtime::Context,
};
use std::{
    collections::VecDeque,
    ffi::OsString,
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

const PROXY_ENV_VARS: &[&str] = &[
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];

fn proxy_environment_enabled(values: impl IntoIterator<Item = OsString>) -> bool {
    values.into_iter().any(|value| !value.is_empty())
}

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
    /// Logical CPUs.
    pub cpu_count: Reading<usize>,
    pub terminal_proxy: Reading<bool>,
    pub mihomo_state: Reading<State>,
    pub mihomo_version: Reading<String>,
    pub memory: Reading<Usage>,
    /// The memory limit tcp.tune sizes buffers against: physical memory capped by cgroup
    /// limits, in KiB. Linux only.
    pub effective_memory_kb: Reading<u64>,
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
    /// The max field of /proc/sys/net/ipv4/tcp_rmem, in bytes. Linux only.
    pub tcp_rmem_max: Reading<u64>,
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
            cpu_count: Reading::Loading,
            terminal_proxy: Reading::Loading,
            mihomo_state: Reading::Loading,
            mihomo_version: Reading::Loading,
            memory: Reading::Loading,
            effective_memory_kb: Reading::Loading,
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
            tcp_rmem_max: Reading::Loading,
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
/// The most CPU usage samples `Snapshot::cpu_history` keeps.
pub const CPU_HISTORY: usize = 120;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub local: LocalSnapshot,
    pub public: PublicSnapshot,
    /// CPU usage of the newest local samples that measured it, oldest first, at most
    /// `CPU_HISTORY` of them. Kept only in memory.
    pub cpu_history: Vec<f32>,
}

/// Appends one local sample's CPU usage to `history` if it is a Value, dropping the oldest
/// sample once `CPU_HISTORY` are held. Other readings leave `history` unchanged.
fn push_cpu_sample(history: &mut VecDeque<f32>, usage: &Reading<f32>) {
    let Reading::Value(usage) = usage else {
        return;
    };
    if history.len() >= CPU_HISTORY {
        history.pop_front();
    }
    history.push_back(*usage);
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
    pub fn sample(&mut self, ctx: &Context) -> LocalSnapshot {
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
        let cpu_count = if cpus.is_empty() {
            Reading::Unavailable("CPU count unavailable".into())
        } else {
            Reading::Value(cpus.len())
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
            cpu_count,
            terminal_proxy: Reading::Value(proxy_environment_enabled(
                PROXY_ENV_VARS.iter().filter_map(std::env::var_os),
            )),
            mihomo_state: Reading::NotApplicable,
            mihomo_version: Reading::NotApplicable,
            memory: if self.system.total_memory() == 0 {
                Reading::Unavailable("memory statistics unavailable".into())
            } else {
                Reading::Value(Usage {
                    used: self.system.used_memory(),
                    total: self.system.total_memory(),
                })
            },
            effective_memory_kb: Reading::NotApplicable,
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
            tcp_rmem_max: Reading::NotApplicable,
            updated: Some(now),
        };
        if cfg!(target_os = "linux") {
            fill_linux_network(
                &mut snapshot,
                Path::new("/proc/sys"),
                Path::new("/sys/module/tcp_bbr/version"),
                &ctx.paths.tcp,
            );
            // Re-read every sample: cgroup limits can change while the TUI runs.
            snapshot.effective_memory_kb =
                Reading::from_result(memory::effective_kb(Path::new("/proc")));
            fill_mihomo(&mut snapshot, ctx);
        }
        snapshot
    }
}

fn fill_mihomo(snapshot: &mut LocalSnapshot, ctx: &Context) {
    snapshot.mihomo_state = Reading::from_result(clash::state(ctx));
    snapshot.mihomo_version = match clash::installed_version(ctx) {
        Ok(Some(version)) => Reading::Value(version),
        Ok(None) => Reading::NotApplicable,
        Err(error) => Reading::Unavailable(error.to_string()),
    };
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
    let rmem = sysctl.join("net/ipv4/tcp_rmem");
    snapshot.tcp_rmem_max = Reading::from_result(
        fs::read_to_string(&rmem)
            .map_err(|e| Error::new("error.io", format!("{}: {e}", rmem.display())))
            .and_then(|text| parse_tcp_rmem(&text)),
    );
}

/// Returns the max field of a tcp_rmem value ("min default max", in bytes). The text must be
/// exactly three whitespace-separated decimal integers; a trailing newline is whitespace.
pub fn parse_tcp_rmem(text: &str) -> Result<u64> {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    let &[min, default, max] = words.as_slice() else {
        return Err(Error::new(
            "error.data",
            format!("tcp_rmem: expected 3 fields, found {}", words.len()),
        ));
    };
    let field = |word: &str| -> Result<u64> {
        // `u64::from_str` alone would accept a leading `+`.
        if !word.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::new(
                "error.data",
                format!("tcp_rmem: {word:?} is not a decimal integer"),
            ));
        }
        word.parse()
            .map_err(|e| Error::new("error.data", format!("tcp_rmem: {word:?}: {e}")))
    };
    field(min)?;
    field(default)?;
    field(max)
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
        let mut local_probe = ctx.clone();
        local_probe.cancel = stop.clone();
        local_probe.events = None;
        let local = thread::spawn(move || {
            let mut sampler = Sampler::new();
            // Owned by the only loop that takes local samples: each sample is pushed once, right
            // after it is taken, and public updates (which only replace `state.public`) cannot
            // reach it.
            let mut cpu_history = VecDeque::with_capacity(CPU_HISTORY);
            let mut warmup = true;
            while !local_stop.load(Ordering::SeqCst) {
                let value = sampler.sample(&local_probe);
                push_cpu_sample(&mut cpu_history, &value.cpu_usage);
                let Ok(mut state) = local_state.lock() else {
                    return;
                };
                state.local = value;
                state.cpu_history.clear();
                state.cpu_history.extend(&cpu_history);
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
    #[test]
    fn missing_mihomo_is_read_only_and_independent_of_terminal_proxy() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let mut snapshot = LocalSnapshot {
            terminal_proxy: Reading::Value(true),
            ..LocalSnapshot::default()
        };
        fill_mihomo(&mut snapshot, &ctx);
        assert_eq!(snapshot.mihomo_state, Reading::Value(State::NotInstalled));
        assert_eq!(snapshot.mihomo_version, Reading::NotApplicable);
        assert_eq!(snapshot.terminal_proxy, Reading::Value(true));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn invalid_binary_path_is_not_reported_as_missing() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(&ctx.paths.clash_binary).unwrap();
        let mut snapshot = LocalSnapshot::default();
        fill_mihomo(&mut snapshot, &ctx);
        assert!(matches!(snapshot.mihomo_state, Reading::Unavailable(_)));
        assert!(matches!(snapshot.mihomo_version, Reading::Unavailable(_)));
    }

    #[test]
    fn invalid_and_unverified_pid_records_are_not_reported_as_stopped() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        let mut snapshot = LocalSnapshot::default();
        fs::write(&ctx.paths.clash_pid, "invalid").unwrap();
        fill_mihomo(&mut snapshot, &ctx);
        assert!(matches!(snapshot.mihomo_state, Reading::Unavailable(_)));
        assert_eq!(snapshot.mihomo_version, Reading::NotApplicable);
        assert_eq!(fs::read_to_string(&ctx.paths.clash_pid).unwrap(), "invalid");

        fs::write(&ctx.paths.clash_pid, "1").unwrap();
        fill_mihomo(&mut snapshot, &ctx);
        assert_eq!(snapshot.mihomo_state, Reading::Value(State::Unknown));
        assert_eq!(fs::read_to_string(&ctx.paths.clash_pid).unwrap(), "1");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_sampler_marks_mihomo_not_applicable() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let snapshot = Sampler::new().sample(&ctx);
        assert_eq!(snapshot.mihomo_state, Reading::NotApplicable);
        assert_eq!(snapshot.mihomo_version, Reading::NotApplicable);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_sampler_marks_tcp_rmem_and_effective_memory_not_applicable() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let snapshot = Sampler::new().sample(&ctx);
        assert_eq!(snapshot.tcp_rmem_max, Reading::NotApplicable);
        assert_eq!(snapshot.effective_memory_kb, Reading::NotApplicable);
        assert!(
            matches!(snapshot.cpu_count, Reading::Value(count) if count > 0),
            "{:?}",
            snapshot.cpu_count
        );
    }

    #[test]
    fn tcp_rmem_read_and_parse_failures_are_unavailable_with_a_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let sysctl = temp.path().join("sysctl");
        fs::create_dir_all(sysctl.join("net/ipv4")).unwrap();
        let rmem = sysctl.join("net/ipv4/tcp_rmem");
        let sample = || {
            let mut snapshot = LocalSnapshot::default();
            fill_linux_network(
                &mut snapshot,
                &sysctl,
                &temp.path().join("version"),
                &temp.path().join("tcp.conf"),
            );
            snapshot.tcp_rmem_max
        };
        let unavailable = |reading: Reading<u64>| match reading {
            Reading::Unavailable(diagnostic) => diagnostic,
            other => panic!("expected Unavailable, got {other:?}"),
        };
        let missing = unavailable(sample());
        assert!(
            missing.starts_with("error.io: ") && missing.contains(&rmem.display().to_string()),
            "{missing}"
        );
        for (text, detail) in [
            ("4096 131072\n", "expected 3 fields, found 2"),
            (
                "4096 +131072 6291456\n",
                "\"+131072\" is not a decimal integer",
            ),
            (
                "4096 131072 18446744073709551616\n",
                "\"18446744073709551616\": number too large",
            ),
        ] {
            fs::write(&rmem, text).unwrap();
            let diagnostic = unavailable(sample());
            assert!(
                diagnostic.starts_with("error.data: tcp_rmem: ") && diagnostic.contains(detail),
                "{text:?}: {diagnostic}"
            );
        }
        fs::write(&rmem, "4096\t131072\t6291456\n").unwrap();
        assert_eq!(sample(), Reading::Value(6_291_456));
    }

    mod tcp_rmem_property {
        use super::super::parse_tcp_rmem;
        use proptest::prelude::*;

        /// Any u64, drawing the boundaries more often than uniform sampling would.
        fn number() -> impl Strategy<Value = u64> {
            prop_oneof![6 => any::<u64>(), 1 => Just(0u64), 1 => Just(u64::MAX)]
        }

        /// A non-empty run of spaces and tabs.
        fn separator() -> impl Strategy<Value = String> {
            proptest::collection::vec(prop_oneof![Just(' '), Just('\t')], 1..=4)
                .prop_map(|chars| chars.into_iter().collect())
        }

        /// `words` joined by separators, optionally followed by a newline.
        fn joined(words: Vec<String>) -> impl Strategy<Value = String> {
            let gaps = words.len().saturating_sub(1);
            (proptest::collection::vec(separator(), gaps), any::<bool>()).prop_map(
                move |(separators, newline)| {
                    let mut text = String::new();
                    for (index, word) in words.iter().enumerate() {
                        if index > 0 {
                            text.push_str(&separators[index - 1]);
                        }
                        text.push_str(word);
                    }
                    if newline {
                        text.push('\n');
                    }
                    text
                },
            )
        }

        fn decimal(numbers: &[u64]) -> Vec<String> {
            numbers.iter().map(u64::to_string).collect()
        }

        /// Three numbers in tcp_rmem's format, with the max the parser must return.
        fn valid() -> impl Strategy<Value = (String, u64)> {
            (number(), number(), number()).prop_flat_map(|(min, default, max)| {
                joined(decimal(&[min, default, max])).prop_map(move |text| (text, max))
            })
        }

        /// Decimal words, but not three of them (including none).
        fn wrong_count() -> impl Strategy<Value = String> {
            prop_oneof![
                proptest::collection::vec(number(), 0..3),
                proptest::collection::vec(number(), 4..8),
            ]
            .prop_flat_map(|numbers| joined(decimal(&numbers)))
        }

        /// A word that is not a decimal u64: signed, holding another character, or too large.
        fn non_decimal_word() -> impl Strategy<Value = String> {
            // Whitespace would split the word, so it is excluded.
            let foreign = prop_oneof![
                Just('+'),
                Just('-'),
                Just('.'),
                Just('０'),
                any::<char>().prop_filter("a non-digit word character", |c| {
                    !c.is_ascii_digit() && !c.is_whitespace()
                }),
            ];
            prop_oneof![
                (prop_oneof![Just('+'), Just('-')], number())
                    .prop_map(|(sign, number)| format!("{sign}{number}")),
                ("[0-9]{0,5}", foreign, "[0-9]{0,5}")
                    .prop_map(|(head, foreign, tail)| format!("{head}{foreign}{tail}")),
                ((u64::MAX as u128 + 1)..=u128::MAX).prop_map(|number| number.to_string()),
            ]
        }

        /// Three words, one of which is not a decimal u64.
        fn non_decimal() -> impl Strategy<Value = String> {
            (number(), number(), number(), 0..3usize, non_decimal_word()).prop_flat_map(
                |(min, default, max, at, word)| {
                    let mut words = decimal(&[min, default, max]);
                    words[at] = word;
                    joined(words)
                },
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]

            // Feature: tui-redesign, Property 3: tcp_rmem 往返
            // **Validates: Requirements 16.4, 16.5, 16.10**
            #[test]
            fn tcp_rmem_round_trips_and_rejects_other_text(
                (text, max) in valid(),
                wrong_count in wrong_count(),
                non_decimal in non_decimal(),
            ) {
                prop_assert_eq!(parse_tcp_rmem(&text).map_err(|e| e.to_string()), Ok(max));
                prop_assert!(parse_tcp_rmem(&wrong_count).is_err(), "{:?}", wrong_count);
                prop_assert!(parse_tcp_rmem(&non_decimal).is_err(), "{:?}", non_decimal);
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn version_errors_preserve_process_state_and_time_out() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
        let mut snapshot = LocalSnapshot::default();
        for script in [
            "#!/bin/sh\nexit 7\n",
            "#!/bin/sh\nprintf 'unexpected banner'\n",
            "#!/bin/sh\nexec sleep 30\n",
        ] {
            fs::write(&ctx.paths.clash_binary, script).unwrap();
            fs::set_permissions(&ctx.paths.clash_binary, fs::Permissions::from_mode(0o700))
                .unwrap();
            let started = Instant::now();
            fill_mihomo(&mut snapshot, &ctx);
            assert!(started.elapsed() < Duration::from_secs(5));
            assert_eq!(snapshot.mihomo_state, Reading::Value(State::Stopped));
            assert!(matches!(snapshot.mihomo_version, Reading::Unavailable(_)));
            assert!(!ctx.paths.clash_pid.exists());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a verified Mihomo binary in CQS_CLASH_SMOKE_BINARY"]
    fn real_mihomo_dashboard_tracks_version_start_and_stop() {
        use std::os::unix::fs::PermissionsExt;
        let binary = std::env::var_os("CQS_CLASH_SMOKE_BINARY").expect("CQS_CLASH_SMOKE_BINARY");
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        fs::create_dir_all(ctx.paths.clash_binary.parent().unwrap()).unwrap();
        fs::create_dir_all(&ctx.paths.clash_data).unwrap();
        fs::copy(binary, &ctx.paths.clash_binary).unwrap();
        fs::set_permissions(&ctx.paths.clash_binary, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&ctx.paths.clash_config, "mode: direct\nmixed-port: 0\n").unwrap();
        let mut sampler = Sampler::new();
        let stopped = sampler.sample(&ctx);
        assert_eq!(stopped.mihomo_state, Reading::Value(State::Stopped));
        let Reading::Value(version) = &stopped.mihomo_version else {
            panic!("expected a version, got {:?}", stopped.mihomo_version);
        };
        assert!(version.starts_with('v'));
        struct StopOnDrop(Context);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                let _ = clash::stop(&self.0);
            }
        }
        let _cleanup = StopOnDrop(ctx.clone());
        clash::start(&ctx).unwrap();
        let running = sampler.sample(&ctx);
        assert!(matches!(
            running.mihomo_state,
            Reading::Value(State::Running(_))
        ));
        assert_eq!(running.mihomo_version, stopped.mihomo_version);
        clash::stop(&ctx).unwrap();
        let stopped_again = sampler.sample(&ctx);
        assert_eq!(stopped_again.mihomo_state, Reading::Value(State::Stopped));
        assert_eq!(stopped_again.mihomo_version, stopped.mihomo_version);
    }

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
        let mut ctx = crate::tests::context(temp.path());
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
        assert_eq!(snapshot.local.terminal_proxy, Reading::Loading);
        assert_eq!(snapshot.local.mihomo_state, Reading::Loading);
        assert_eq!(snapshot.local.mihomo_version, Reading::Loading);
        assert_eq!(snapshot.local.cpu_count, Reading::Loading);
        assert_eq!(snapshot.local.effective_memory_kb, Reading::Loading);
        assert_eq!(snapshot.local.tcp_rmem_max, Reading::Loading);
        assert_eq!(snapshot.public.ipv4, Reading::NotQueried);
        assert_eq!(snapshot.public.isp, Reading::NotQueried);
        assert!(snapshot.cpu_history.is_empty());
    }

    #[test]
    fn cpu_history_drops_the_oldest_sample_when_full_and_skips_other_readings() {
        let mut history = VecDeque::new();
        for usage in 0..CPU_HISTORY {
            push_cpu_sample(&mut history, &Reading::Value(usage as f32));
        }
        for usage in [
            Reading::Loading,
            Reading::NotQueried,
            Reading::Unavailable("CPU sample unavailable".into()),
            Reading::NotApplicable,
        ] {
            push_cpu_sample(&mut history, &usage);
        }
        assert_eq!(history.len(), CPU_HISTORY);
        assert_eq!(history.front(), Some(&0.0));
        push_cpu_sample(&mut history, &Reading::Value(500.0));
        assert_eq!(history.len(), CPU_HISTORY);
        assert_eq!(history.front(), Some(&1.0));
        assert_eq!(history.back(), Some(&500.0));
    }

    mod cpu_history_property {
        use super::super::{CPU_HISTORY, Reading, push_cpu_sample};
        use proptest::prelude::*;
        use std::collections::VecDeque;

        /// Any reading `cpu_usage` can hold. Values come up 3 times in 5, so sequences near the
        /// 300-sample limit usually hold more than `CPU_HISTORY` of them.
        fn usage() -> impl Strategy<Value = Reading<f32>> {
            prop_oneof![
                3 => (0.0f32..=100.0).prop_map(Reading::Value),
                2 => prop_oneof![
                    Just(Reading::Loading),
                    Just(Reading::NotQueried),
                    Just(Reading::NotApplicable),
                    "[a-z ]{0,16}".prop_map(Reading::Unavailable),
                ],
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]

            // Feature: tui-redesign, Property 16: CPU 历史
            // **Validates: Requirements 16.1, 16.2**
            #[test]
            fn cpu_history_keeps_the_newest_values_oldest_first(
                samples in proptest::collection::vec(usage(), 0..=300),
            ) {
                let mut history = VecDeque::new();
                let mut values = Vec::new();
                // Checked after every sample, so the property holds for each prefix too.
                for sample in &samples {
                    push_cpu_sample(&mut history, sample);
                    if let Reading::Value(value) = sample {
                        values.push(*value);
                    }
                    let newest = &values[values.len().saturating_sub(CPU_HISTORY)..];
                    prop_assert_eq!(history.len(), values.len().min(CPU_HISTORY));
                    prop_assert_eq!(history.iter().copied().collect::<Vec<_>>(), newest);
                }
            }
        }
    }

    #[test]
    fn terminal_proxy_status_only_requires_a_proxy_variable_with_a_value() {
        assert!(!proxy_environment_enabled([OsString::from("")]));
        assert!(proxy_environment_enabled([OsString::from(
            "http://127.0.0.1:7890"
        )]));
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
