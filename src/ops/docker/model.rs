//! Domain data model and deterministic value parsers for the dedicated Docker TUI.

use std::{collections::BTreeMap, time::Instant};

/// Maximum bytes retained per single log line to prevent pathological single-line JSON floods.
pub const MAX_LOG_LINE_BYTES: usize = 4096;
/// Maximum history points kept per container for CPU / memory sparklines.
pub const STATS_HISTORY_LIMIT: usize = 30;
/// Maximum log lines retained in memory per active stream.
pub const MAX_DOCKER_LOG_LINES: usize = 5000;

/// Top-level resource tabs inside the dedicated Docker workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerTab {
    Containers,
    Compose,
    Images,
    Volumes,
    Networks,
    Daemon,
}

impl DockerTab {
    pub const ALL: [Self; 6] = [
        Self::Containers,
        Self::Compose,
        Self::Images,
        Self::Volumes,
        Self::Networks,
        Self::Daemon,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Containers => 0,
            Self::Compose => 1,
            Self::Images => 2,
            Self::Volumes => 3,
            Self::Networks => 4,
            Self::Daemon => 5,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        Self::ALL.get(idx).copied().unwrap_or(Self::Containers)
    }

    pub fn from_cli(value: &str) -> Option<Self> {
        match value {
            "containers" => Some(Self::Containers),
            "compose" => Some(Self::Compose),
            "images" => Some(Self::Images),
            "volumes" => Some(Self::Volumes),
            "networks" => Some(Self::Networks),
            "daemon" => Some(Self::Daemon),
            _ => None,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Containers => "docker.tab.containers",
            Self::Compose => "docker.tab.compose",
            Self::Images => "docker.tab.images",
            Self::Volumes => "docker.tab.volumes",
            Self::Networks => "docker.tab.networks",
            Self::Daemon => "docker.tab.daemon",
        }
    }
}

/// Reachability state of the local Docker Engine, explicitly distinguishing missing binary,
/// stopped daemon, permission errors, and unsupported platforms.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DockerAvailability {
    #[default]
    Loading,
    NotApplicable(String),
    NotInstalled,
    DaemonStopped(String),
    PermissionDenied(String),
    Unavailable(String),
    Ready,
}

impl DockerAvailability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Summary metadata of the local Docker Engine and `/etc/docker/daemon.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineInfo {
    pub server_version: String,
    pub api_version: String,
    pub compose_version: Option<String>,
    pub context: String,
    pub os_type: String,
    pub arch: String,
    pub kernel: String,
    pub storage_driver: String,
    pub cgroup_driver: String,
    pub cgroup_version: String,
    pub root_dir: String,
    pub containers_running: usize,
    pub containers_paused: usize,
    pub containers_stopped: usize,
    pub images_total: usize,
    pub service_active: Option<bool>,
    pub daemon_mirrors: Vec<String>,
    pub daemon_ipv6: Option<bool>,
    pub daemon_ipv6_cidr: Option<String>,
    pub daemon_json_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContainerState {
    Running,
    Restarting,
    Paused,
    Created,
    Removing,
    Exited,
    Dead,
    Unknown,
}

impl ContainerState {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "running" => Self::Running,
            "restarting" => Self::Restarting,
            "paused" => Self::Paused,
            "created" => Self::Created,
            "removing" => Self::Removing,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            _ => Self::Unknown,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Restarting)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Running => "docker.state.running",
            Self::Restarting => "docker.state.restarting",
            Self::Paused => "docker.state.paused",
            Self::Created => "docker.state.created",
            Self::Removing => "docker.state.removing",
            Self::Exited => "docker.state.exited",
            Self::Dead => "docker.state.dead",
            Self::Unknown => "docker.state.unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerHealth {
    None,
    Healthy,
    Unhealthy,
    Starting,
}

impl ContainerHealth {
    pub fn from_status(status: &str) -> Self {
        let lower = status.to_ascii_lowercase();
        if lower.contains("(healthy)") {
            Self::Healthy
        } else if lower.contains("(unhealthy)") {
            Self::Unhealthy
        } else if lower.contains("(health: starting)") {
            Self::Starting
        } else {
            Self::None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerItem {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    pub created_at: String,
    pub running_for: String,
    pub status: String,
    pub state: ContainerState,
    pub health: ContainerHealth,
    pub ports: Vec<String>,
    pub networks: Vec<String>,
    pub mounts: Vec<String>,
    pub mounts_count: usize,
    pub size: String,
    pub size_bytes: Option<u64>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub compose_working_dir: Option<String>,
    pub compose_config_files: Option<String>,
    pub labels: BTreeMap<String, String>,
}

impl ContainerItem {
    pub fn compact_ports(&self) -> String {
        if self.ports.is_empty() {
            return "-".into();
        }
        let mut short = Vec::new();
        for p in &self.ports {
            if let Some((host, _)) = p.split_once("->") {
                let port = host.rsplit(':').next().unwrap_or(host);
                if !short.iter().any(|existing| existing == port) {
                    short.push(port.to_owned());
                }
            } else if !short.iter().any(|existing| existing == p) {
                short.push(p.clone());
            }
        }
        if short.is_empty() {
            "-".into()
        } else {
            short.join(",")
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerStats {
    pub container_id: String,
    pub cpu_percent: f64,
    pub mem_used_bytes: u64,
    pub mem_limit_bytes: u64,
    pub mem_percent: f64,
    pub mem_usage_text: String,
    pub net_io_text: String,
    pub block_io_text: String,
    pub pids: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerTopRow {
    pub uid: String,
    pub pid: String,
    pub cpu: String,
    pub mem: String,
    pub time: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub mode: String,
    pub rw: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkEndpoint {
    pub name: String,
    pub ipv4: String,
    pub ipv6: String,
    pub mac: String,
    pub gateway: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthLogEntry {
    pub exit_code: i64,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInspect {
    pub id: String,
    pub name: String,
    pub image: String,
    pub platform: String,
    pub restart_policy: String,
    pub restart_count: u64,
    pub created: String,
    pub started_at: String,
    pub finished_at: String,
    pub exit_code: i64,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: String,
    pub user: String,
    pub ports: Vec<String>,
    pub networks: Vec<NetworkEndpoint>,
    pub mounts: Vec<MountInfo>,
    pub env: Vec<EnvVar>,
    pub labels: BTreeMap<String, String>,
    pub health_status: Option<String>,
    pub health_logs: Vec<HealthLogEntry>,
    pub top: Vec<ContainerTopRow>,
    pub raw_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeProject {
    pub name: String,
    pub status: String,
    pub config_files: Vec<String>,
    pub working_dir: Option<String>,
    pub running_count: usize,
    pub total_count: usize,
    pub container_ids: Vec<String>,
    pub synthesized_from_labels: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageItem {
    pub id: String,
    pub short_id: String,
    pub repository: String,
    pub tag: String,
    pub created_since: String,
    pub created_at: String,
    pub size_text: String,
    pub size_bytes: u64,
    pub dangling: bool,
    pub used_by: Vec<String>,
}

impl ImageItem {
    pub fn display_ref(&self) -> String {
        if self.dangling || (self.repository == "<none>" && self.tag == "<none>") {
            format!("<none> ({})", self.short_id)
        } else if self.tag.is_empty() || self.tag == "<none>" {
            self.repository.clone()
        } else {
            format!("{}:{}", self.repository, self.tag)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageHistoryRow {
    pub id: String,
    pub created_since: String,
    pub created_by: String,
    pub size_text: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInspect {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub os: String,
    pub arch: String,
    pub size_bytes: u64,
    pub exposed_ports: Vec<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: String,
    pub env: Vec<EnvVar>,
    pub labels: BTreeMap<String, String>,
    pub history: Vec<ImageHistoryRow>,
    pub raw_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeItem {
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub mountpoint: String,
    pub size_text: String,
    pub size_bytes: Option<u64>,
    pub anonymous: bool,
    pub used_by: Vec<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkItem {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub ipv6: bool,
    pub internal: bool,
    pub builtin: bool,
    pub subnet: String,
    pub gateway: String,
    pub ipv6_subnet: String,
    pub containers: Vec<(String, String)>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsageRow {
    pub kind: String,
    pub total_count: u64,
    pub active_count: u64,
    pub size_text: String,
    pub size_bytes: u64,
    pub reclaimable_text: String,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSeverity {
    Error,
    Warn,
    Info,
    Debug,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerLogEntry {
    pub timestamp: Option<String>,
    pub message: String,
    pub severity: LogSeverity,
}

/// Full point-in-time read-only snapshot held by the Docker TUI workspace.
#[derive(Debug, Clone, Default)]
pub struct DockerSnapshot {
    pub availability: DockerAvailability,
    pub engine: Option<EngineInfo>,
    pub containers: Vec<ContainerItem>,
    pub compose: Vec<ComposeProject>,
    pub images: Vec<ImageItem>,
    pub volumes: Vec<VolumeItem>,
    pub networks: Vec<NetworkItem>,
    pub disk_usage: Vec<DiskUsageRow>,
    pub stats: BTreeMap<String, ContainerStats>,
    pub cpu_history: BTreeMap<String, Vec<f32>>,
    pub mem_history: BTreeMap<String, Vec<f32>>,
    pub updated: Option<Instant>,
    pub last_error: Option<String>,
    /// Bitmask of tabs already queried at least once (avoids re-polling genuinely empty lists).
    pub fetched_tabs: u8,
    /// Last time `docker info` was queried.
    pub engine_checked: Option<Instant>,
    /// Last time `docker system df` was queried.
    pub df_updated: Option<Instant>,
    /// Last time `docker system df -v` (volume sizes) was queried.
    pub vol_df_updated: Option<Instant>,
}

impl DockerSnapshot {
    pub fn tab_bit(tab: DockerTab) -> u8 {
        match tab {
            DockerTab::Containers => 1 << 0,
            DockerTab::Compose => 1 << 1,
            DockerTab::Images => 1 << 2,
            DockerTab::Volumes => 1 << 3,
            DockerTab::Networks => 1 << 4,
            DockerTab::Daemon => 1 << 5,
        }
    }

    pub fn has_fetched(&self, tab: DockerTab) -> bool {
        self.fetched_tabs & Self::tab_bit(tab) != 0
    }

    pub fn mark_fetched(&mut self, tab: DockerTab) {
        self.fetched_tabs |= Self::tab_bit(tab);
    }
    pub fn running_containers(&self) -> usize {
        self.containers
            .iter()
            .filter(|c| c.state.is_running())
            .count()
    }

    pub fn total_disk_bytes(&self) -> u64 {
        self.disk_usage.iter().map(|row| row.size_bytes).sum()
    }

    pub fn total_reclaimable_bytes(&self) -> u64 {
        self.disk_usage
            .iter()
            .map(|row| row.reclaimable_bytes)
            .sum()
    }

    pub fn record_stats(&mut self, incoming: BTreeMap<String, ContainerStats>) {
        for (id, st) in &incoming {
            let cpu = self.cpu_history.entry(id.clone()).or_default();
            if cpu.len() >= STATS_HISTORY_LIMIT {
                cpu.remove(0);
            }
            cpu.push(st.cpu_percent.clamp(0.0, 100.0) as f32);

            let mem = self.mem_history.entry(id.clone()).or_default();
            if mem.len() >= STATS_HISTORY_LIMIT {
                mem.remove(0);
            }
            mem.push(st.mem_percent.clamp(0.0, 100.0) as f32);
        }
        self.stats = incoming;
    }
}

/// Returns true if an environment variable key likely carries secrets/credentials and should
/// be masked by default in the TUI inspector.
pub fn is_sensitive_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    [
        "PASS",
        "SECRET",
        "TOKEN",
        "KEY",
        "AUTH",
        "CREDENTIAL",
        "PRIVATE",
        "DSN",
        "COOKIE",
        "CERT",
        "SIGNING",
        "JWT",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

/// Parses human-readable Docker sizes such as `"1.24GB"`, `"318MiB"`, `"42kB"`, `"0B"`, or
/// `"128MB (virtual 412MB)"` into bytes.
pub fn parse_size_bytes(raw: &str) -> Option<u64> {
    let first = raw.split('(').next()?.trim();
    let first = first.split('/').next()?.trim();
    if first.is_empty() || first == "--" || first == "N/A" {
        return None;
    }
    let split_at = first
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(first.len());
    let num: f64 = first[..split_at].trim().parse().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    let unit = first[split_at..].trim();
    let mult: f64 = match unit {
        "" | "B" | "b" => 1.0,
        "kB" | "KB" | "K" | "k" => 1_000.0,
        "KiB" | "Ki" => 1_024.0,
        "MB" | "M" => 1_000_000.0,
        "MiB" | "Mi" => 1_048_576.0,
        "GB" | "G" => 1_000_000_000.0,
        "GiB" | "Gi" => 1_073_741_824.0,
        "TB" | "T" => 1_000_000_000_000.0,
        "TiB" | "Ti" => 1_099_511_627_776.0,
        _ => return None,
    };
    Some((num * mult).round() as u64)
}

/// Parses `"8.42%"` or `"0.00%"` into a percentage float.
pub fn parse_percent(raw: &str) -> Option<f64> {
    let trimmed = raw.trim().trim_end_matches('%').trim();
    if trimmed.is_empty() || trimmed == "--" || trimmed == "N/A" {
        return None;
    }
    let val: f64 = trimmed.parse().ok()?;
    val.is_finite().then_some(val)
}

/// Formats bytes into compact human-readable KiB/MiB/GiB/TiB text.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Compact cell-friendly memory string (`42M`, `1.2G`, `512K`).
pub fn format_bytes_compact(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1}G", b / GIB)
    } else if b >= MIB {
        format!("{:.0}M", b / MIB)
    } else if b >= KIB {
        format!("{:.0}K", b / KIB)
    } else {
        format!("{bytes}B")
    }
}

/// Heuristic log severity classifier for Docker container output lines.
pub fn classify_log_severity(message: &str) -> LogSeverity {
    let upper = message.to_ascii_uppercase();
    if upper.contains("FATAL")
        || upper.contains("PANIC")
        || upper.contains("ERROR")
        || upper.contains("ERR ")
        || upper.contains("[ERR]")
        || upper.contains("CRITICAL")
        || upper.contains("EXCEPTION")
    {
        LogSeverity::Error
    } else if upper.contains("WARN") || upper.contains("[WRN]") {
        LogSeverity::Warn
    } else if upper.contains("DEBUG") || upper.contains("TRACE") || upper.contains("[DBG]") {
        LogSeverity::Debug
    } else if upper.contains("INFO") || upper.contains("[INF]") {
        LogSeverity::Info
    } else {
        LogSeverity::Plain
    }
}

/// Parses one raw line from `docker logs --timestamps`, sanitizing terminal escape sequences
/// and capping excessive length.
pub fn parse_log_line(raw: &str) -> DockerLogEntry {
    let clean = crate::runtime::sanitize(raw);
    let capped = if clean.len() > MAX_LOG_LINE_BYTES {
        let mut end = MAX_LOG_LINE_BYTES;
        while end > 0 && !clean.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &clean[..end])
    } else {
        clean
    };

    // Standard RFC3339 prefix emitted by `docker logs --timestamps`:
    // e.g. `2026-11-23T09:18:01.123456789Z message...`
    let (timestamp, message) = match capped.split_once(' ') {
        Some((head, rest)) if looks_like_rfc3339(head) => {
            let short_ts = compact_timestamp(head);
            (Some(short_ts), rest.to_owned())
        }
        _ => (None, capped),
    };
    let severity = classify_log_severity(&message);
    DockerLogEntry {
        timestamp,
        message,
        severity,
    }
}

fn looks_like_rfc3339(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b't')
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..4].iter().all(u8::is_ascii_digit)
}

fn compact_timestamp(ts: &str) -> String {
    // Turn `2026-11-23T09:18:01.123456789Z` into `11-23 09:18:01` for compact terminal display.
    if ts.len() >= 19 {
        format!("{} {}", &ts[5..10], &ts[11..19])
    } else {
        ts.to_owned()
    }
}

pub fn short_id(id: &str) -> String {
    let trimmed = id.strip_prefix("sha256:").unwrap_or(id);
    trimmed.chars().take(12).collect()
}

pub fn is_anonymous_volume_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
}
