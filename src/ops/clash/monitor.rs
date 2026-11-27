//! Lazy background Mihomo monitor (`ClashMonitor`) built on `std::thread` + bounded `mpsc`.
//!
//! Zero network or disk probes run while idle outside the Mihomo workspace; automatically sleeps
//! 10 seconds after leaving the Mihomo view. Uses HTTP/1.1 Keep-Alive for REST queries and
//! pure-Rust RFC 6455 WebSocket streams for real-time `/traffic`, `/memory`, and on-demand `/logs`.

use super::{
    controller::{self, CloseOnSwitchPolicy, KeepAliveClient},
    model::{
        ClashAvailability, ClashSnapshot, ClashTab, ConnectionItem, IpQuality, IpQualityOutcome,
        MAX_CLASH_LOG_LINES, MAX_CLOSED_CONNECTIONS, MAX_TRAFFIC_HISTORY, OutboundIpInfo,
        ServiceCheckItem, ServiceProbeStatus, ServiceTarget, TrafficSample,
    },
    outbound_ip, service_check, ws,
};
use crate::{error::Result, runtime::Context, storage};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FAST_POLL_INTERVAL: Duration = Duration::from_secs(1);
const FULL_POLL_INTERVAL: Duration = Duration::from_secs(3);
const SLOW_POLL_INTERVAL: Duration = Duration::from_secs(5);
const LOG_ROTATE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_SLEEP_AFTER: Duration = Duration::from_secs(10);
const ROUTE_SETTLE_DELAY: Duration = Duration::from_millis(600);
const QUALITY_CACHE_TTL_SECS: u64 = 72 * 3600;
const QUALITY_CACHE_CAP: usize = 128;
const MAX_QUALITY_CACHE_FILE_BYTES: u64 = 256 * 1024;
const EVENT_QUEUE_CAP: usize = 64;
const WS_QUEUE_CAP: usize = 64;
const DELAY_POOL_CONCURRENCY: usize = 16;
const FAILED_SERVICE_BACKOFF_SECS: [u64; 3] = [60, 120, 300];

fn push_bounded<T>(tx: &SyncSender<T>, item: T) -> bool {
    !matches!(tx.try_send(item), Err(TrySendError::Disconnected(_)))
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClashActionTag {
    SelectProxy,
    UnfixGroup,
    SetMode,
    CloseConn,
    CloseAllConns,
    SwitchProfile,
    UpdateProfile(String),
    UpdateAllProfiles,
    RemoveProfile,
    UpdateProvider,
    HealthcheckProvider,
    EnableLocalController,
}

#[derive(Debug)]
pub enum ClashMonitorCmd {
    Wake(ClashTab),
    SetTab(ClashTab),
    ForceRefresh,
    RefreshOnce,
    LeaveClash,
    SelectProxy {
        group: String,
        node: String,
    },
    UnfixGroup {
        group: String,
    },
    SetMode {
        mode: String,
    },
    TestGroupDelay {
        group: String,
        url: String,
        timeout_ms: u64,
    },
    TestNodeDelay {
        group: String,
        node: String,
        url: String,
        timeout_ms: u64,
    },
    CloseConnection {
        id: String,
    },
    CloseAllConnections,
    SwitchProfile {
        name: String,
    },
    UpdateProfile {
        name: String,
    },
    UpdateAllProfiles,
    RemoveProfile {
        name: String,
    },
    UpdateProxyProvider {
        name: String,
    },
    HealthcheckProxyProvider {
        name: String,
    },
    UpdateRuleProvider {
        name: String,
    },
    EnableLocalController,
    ProbeExit,
    CheckServices {
        targets: Vec<ServiceTarget>,
        manual: bool,
    },
    SetPinnedServices(Vec<ServiceTarget>),
    SetCloseOnSwitch(CloseOnSwitchPolicy),
    SetLogStream(bool),
    RefreshProfileUserinfo {
        name: String,
    },
    RouteChanged,
    FetchLogs,
    Shutdown,
}

#[derive(Debug)]
pub enum ClashEvent {
    Snapshot(Box<ClashSnapshot>),
    NodeDelayTick {
        group: String,
        node: String,
        delay_ms: Option<u64>,
        completed: usize,
        total: usize,
    },
    GroupDelayDone {
        group: String,
        result: Result<BTreeMap<String, Option<u64>>>,
    },
    NodeDelayDone {
        group: String,
        node: String,
        result: Result<Option<u64>>,
    },
    ActionDone {
        tag: ClashActionTag,
        result: Result<String>,
    },
    OutboundProbeStarted {
        generation: u64,
        manual: bool,
    },
    OutboundIpResolved {
        generation: u64,
        result: Result<OutboundIpInfo>,
        manual: bool,
    },
    OutboundQualityResolved {
        generation: u64,
        ip: String,
        outcome: IpQualityOutcome,
    },
    OutboundLatencyResolved {
        generation: u64,
        google_ms: Option<u64>,
        github_ms: Option<u64>,
    },
    ServiceCheckStarted {
        generation: u64,
        targets: Vec<ServiceTarget>,
    },
    ServiceCheckItemDone {
        generation: u64,
        item: ServiceCheckItem,
    },
    ServiceCheckFinished {
        generation: u64,
        manual: bool,
    },
    ProfileUserinfoState {
        name: String,
        probing: bool,
        found: Option<bool>,
    },
    LogsLoaded(Result<Vec<String>>),
    LogStreamLine(String),
}

enum WsUpdate {
    Traffic { up: u64, down: u64 },
    Memory { inuse: u64 },
    LogLine(String),
}

/// Pure connection speed diffing + bounded closed-connection history tracker.
#[derive(Default)]
struct ConnectionTracker {
    prev_totals: Option<(u64, u64, Instant)>,
    prev_conn_bytes: HashMap<String, (u64, u64)>,
    prev_active: HashMap<String, ConnectionItem>,
    closed_ring: VecDeque<ConnectionItem>,
    traffic_history: VecDeque<TrafficSample>,
    peak_up: u64,
    peak_down: u64,
}

impl ConnectionTracker {
    fn reset(&mut self) {
        self.prev_totals = None;
        self.prev_conn_bytes.clear();
        self.prev_active.clear();
        self.closed_ring.clear();
        self.traffic_history.clear();
        self.peak_up = 0;
        self.peak_down = 0;
    }

    fn record_ws_traffic(&mut self, up: u64, down: u64, snap: &mut ClashSnapshot) {
        self.peak_up = self.peak_up.max(up);
        self.peak_down = self.peak_down.max(down);
        if self.traffic_history.len() >= MAX_TRAFFIC_HISTORY {
            self.traffic_history.pop_front();
        }
        let active_conns = snap.connections.len().min(u32::MAX as usize) as u32;
        self.traffic_history.push_back(TrafficSample {
            up_bps: up,
            down_bps: down,
            active_conns,
        });
        snap.up_speed = up;
        snap.down_speed = down;
        snap.peak_up_speed = self.peak_up;
        snap.peak_down_speed = self.peak_down;
        snap.traffic_history = self.traffic_history.clone();
    }

    #[cfg(test)]
    fn ingest(&mut self, raw: controller::RawConnections, now: Instant, snap: &mut ClashSnapshot) {
        self.ingest_with_mode(raw, now, false, snap);
    }

    fn ingest_with_mode(
        &mut self,
        raw: controller::RawConnections,
        now: Instant,
        ws_traffic_active: bool,
        snap: &mut ClashSnapshot,
    ) {
        let elapsed_secs = self
            .prev_totals
            .map(|(_, _, at)| now.saturating_duration_since(at).as_secs_f64().max(0.25));
        let active_conns = raw.connections.len().min(u32::MAX as usize) as u32;

        let (up_speed, down_speed, record_sample) = match (self.prev_totals, elapsed_secs) {
            (Some((prev_up, prev_down, _)), Some(dt)) => {
                if raw.upload_total < prev_up || raw.download_total < prev_down {
                    // Counter reset (e.g. Mihomo core restarted): re-anchor baseline cleanly
                    // without polluting the real-time traffic graph with a false cliff.
                    (0, 0, false)
                } else {
                    let du = raw.upload_total - prev_up;
                    let dd = raw.download_total - prev_down;
                    ((du as f64 / dt) as u64, (dd as f64 / dt) as u64, true)
                }
            }
            _ => (0, 0, false),
        };
        self.prev_totals = Some((raw.upload_total, raw.download_total, now));
        if record_sample && !ws_traffic_active {
            self.peak_up = self.peak_up.max(up_speed);
            self.peak_down = self.peak_down.max(down_speed);
            if self.traffic_history.len() >= MAX_TRAFFIC_HISTORY {
                self.traffic_history.pop_front();
            }
            self.traffic_history.push_back(TrafficSample {
                up_bps: up_speed,
                down_bps: down_speed,
                active_conns,
            });
        }

        let mut next_active = HashMap::with_capacity(raw.connections.len());
        let mut next_bytes = HashMap::with_capacity(raw.connections.len());
        let mut live_list = Vec::with_capacity(raw.connections.len());
        let mut current_ids = HashSet::with_capacity(raw.connections.len());

        for mut conn in raw.connections {
            current_ids.insert(conn.id.clone());
            if let (Some(&(pu, pd)), Some(dt)) = (self.prev_conn_bytes.get(&conn.id), elapsed_secs)
            {
                let du = conn.upload.saturating_sub(pu);
                let dd = conn.download.saturating_sub(pd);
                conn.up_speed = (du as f64 / dt) as u64;
                conn.down_speed = (dd as f64 / dt) as u64;
            }
            next_bytes.insert(conn.id.clone(), (conn.upload, conn.download));
            next_active.insert(conn.id.clone(), conn.clone());
            live_list.push(conn);
        }

        // Move disappeared connections into the bounded closed-connection ring buffer.
        for (old_id, mut old_conn) in self.prev_active.drain() {
            if !current_ids.contains(&old_id) {
                old_conn.closed = true;
                old_conn.up_speed = 0;
                old_conn.down_speed = 0;
                self.closed_ring.push_front(old_conn);
                while self.closed_ring.len() > MAX_CLOSED_CONNECTIONS {
                    self.closed_ring.pop_back();
                }
            }
        }
        self.prev_active = next_active;
        self.prev_conn_bytes = next_bytes;

        snap.upload_total = raw.upload_total;
        snap.download_total = raw.download_total;
        if !ws_traffic_active {
            snap.up_speed = up_speed;
            snap.down_speed = down_speed;
            snap.peak_up_speed = self.peak_up;
            snap.peak_down_speed = self.peak_down;
            snap.traffic_history = self.traffic_history.clone();
        }
        if raw.memory.is_some() {
            snap.memory_bytes = raw.memory;
        }
        snap.connections = live_list;
        snap.closed_connections = self.closed_ring.clone();
    }
}

type RouteSignature = (String, BTreeMap<String, String>, BTreeMap<String, String>);

fn route_signature(snap: &ClashSnapshot) -> RouteSignature {
    let groups = snap
        .groups
        .iter()
        .map(|g| (g.name.clone(), g.now.clone()))
        .collect();
    let providers = snap
        .proxy_providers
        .iter()
        .map(|p| (p.name.clone(), p.updated_at.clone()))
        .collect();
    (snap.summary.mode.clone(), groups, providers)
}

fn spawn_ws_stream(
    settings: &controller::Settings,
    path: &'static str,
    ws_tx: SyncSender<WsUpdate>,
    stop: Arc<AtomicBool>,
    global_cancel: Arc<AtomicBool>,
    ctx_for_logs: Option<Context>,
) -> JoinHandle<()> {
    let socket = settings.socket();
    let host = settings.address.clone();
    let secret = settings.secret_str().to_owned();
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) && !global_cancel.load(Ordering::Relaxed) {
            if let Ok(mut client) =
                ws::WsClient::connect(socket, &host, path, &secret, Duration::from_secs(3))
            {
                while let Ok(Some(text)) = client.read_text(&stop, &global_cancel) {
                    if path == "/traffic" {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let up = json.get("up").and_then(|v| v.as_u64()).unwrap_or(0);
                            let down = json.get("down").and_then(|v| v.as_u64()).unwrap_or(0);
                            if !push_bounded(&ws_tx, WsUpdate::Traffic { up, down }) {
                                return;
                            }
                        }
                    } else if path == "/memory" {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                            && let Some(inuse) = json.get("inuse").and_then(|v| v.as_u64())
                            && !push_bounded(&ws_tx, WsUpdate::Memory { inuse })
                        {
                            return;
                        }
                    } else if let Some(ref c) = ctx_for_logs {
                        let raw_line =
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                let level = json
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("info")
                                    .to_ascii_uppercase();
                                let payload =
                                    json.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                                format!("[{level}] {payload}")
                            } else {
                                text
                            };
                        if let Ok(safe) = super::logs::redact(c, &raw_line)
                            && !push_bounded(&ws_tx, WsUpdate::LogLine(safe))
                        {
                            return;
                        }
                    }
                }
            }
            // Back off cleanly before reconnecting, waking early if stopped.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if stop.load(Ordering::Relaxed) || global_cancel.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(80));
            }
        }
    })
}

struct WsBundle {
    target_addr: String,
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl WsBundle {
    fn start_metrics(
        settings: &controller::Settings,
        ws_tx: &SyncSender<WsUpdate>,
        global_cancel: &Arc<AtomicBool>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let h_traffic = spawn_ws_stream(
            settings,
            "/traffic",
            ws_tx.clone(),
            stop.clone(),
            global_cancel.clone(),
            None,
        );
        let h_memory = spawn_ws_stream(
            settings,
            "/memory",
            ws_tx.clone(),
            stop.clone(),
            global_cancel.clone(),
            None,
        );
        Self {
            target_addr: settings.address.clone(),
            stop,
            handles: vec![h_traffic, h_memory],
        }
    }

    fn start_logs(
        ctx: &Context,
        settings: &controller::Settings,
        ws_tx: &SyncSender<WsUpdate>,
        global_cancel: &Arc<AtomicBool>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let h_logs = spawn_ws_stream(
            settings,
            "/logs?level=info",
            ws_tx.clone(),
            stop.clone(),
            global_cancel.clone(),
            Some(ctx.clone()),
        );
        Self {
            target_addr: settings.address.clone(),
            stop,
            handles: vec![h_logs],
        }
    }

    fn stop_nonblocking(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn stop_and_join(self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

pub struct ClashMonitor {
    tx: Sender<ClashMonitorCmd>,
    rx: Receiver<ClashEvent>,
    handle: Option<JoinHandle<()>>,
}

impl ClashMonitor {
    pub fn start(ctx: &Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<ClashMonitorCmd>();
        let (evt_tx, evt_rx) = mpsc::sync_channel::<ClashEvent>(EVENT_QUEUE_CAP);
        let (ws_tx, ws_rx) = mpsc::sync_channel::<WsUpdate>(WS_QUEUE_CAP);
        let ctx = ctx.silent();
        let self_tx = cmd_tx.clone();
        let initial_close_policy = CloseOnSwitchPolicy::from_preferences(&ctx);

        let handle = thread::spawn(move || {
            let mut active = false;
            let mut left_at: Option<Instant> = None;
            let mut tab = ClashTab::Overview;
            let mut snap = ClashSnapshot::default();
            let mut tracker = ConnectionTracker::default();
            let mut last_fast = Instant::now() - FAST_POLL_INTERVAL;
            let mut last_full = Instant::now() - FULL_POLL_INTERVAL;
            let mut last_slow = Instant::now() - SLOW_POLL_INTERVAL;
            let mut last_log_rotate = Instant::now();
            let mut version_cached = String::new();
            let mut cached_settings: Option<controller::Settings> = None;
            let mut ka_client: Option<KeepAliveClient> = None;
            let mut metrics_ws: Option<WsBundle> = None;
            let mut logs_ws: Option<WsBundle> = None;
            let mut log_stream_requested = false;
            let mut last_ws_traffic: Option<Instant> = None;
            let mut uptime_anchor: Option<(u64, Instant)> = None;
            let mut probe_generation: u64 = 0;
            let mut active_probe_cancel: Option<Arc<AtomicBool>> = None;
            let mut service_generation: u64 = 0;
            let mut active_service_cancel: Option<Arc<AtomicBool>> = None;
            let mut close_on_switch = initial_close_policy;
            let mut pinned_services: Vec<ServiceTarget> = vec![
                ServiceTarget::Google,
                ServiceTarget::GitHub,
                ServiceTarget::ChatGpt,
                ServiceTarget::Claude,
            ];
            let mut userinfo_probed_profiles: HashSet<String> = HashSet::new();
            let quality_cache_path = quality_cache_file(&ctx);
            let quality_cache: QualityCache =
                Arc::new(Mutex::new(load_quality_cache_disk(&quality_cache_path)));
            let mut auto_probed_ready = false;
            let mut pending_route_probe_at: Option<Instant> = None;
            let mut prev_route_sig: Option<RouteSignature> = None;
            let failed_pinned: Arc<Mutex<Vec<ServiceTarget>>> = Arc::new(Mutex::new(Vec::new()));
            let mut failed_recheck_step: u8 = 0;
            let mut failed_recheck_at: Option<Instant> = None;

            let stop_all_ws = |m: &mut Option<WsBundle>, l: &mut Option<WsBundle>| {
                if let Some(bundle) = m.take() {
                    bundle.stop_nonblocking();
                }
                if let Some(bundle) = l.take() {
                    bundle.stop_nonblocking();
                }
            };

            loop {
                if ctx.cancel.load(Ordering::SeqCst) {
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(cancel) = active_service_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(bundle) = metrics_ws.take() {
                        bundle.stop_and_join();
                    }
                    if let Some(bundle) = logs_ws.take() {
                        bundle.stop_and_join();
                    }
                    break;
                }
                let wait = if active && snap.availability.is_ready() {
                    let until_fast = FAST_POLL_INTERVAL
                        .saturating_sub(Instant::now().saturating_duration_since(last_fast));
                    until_fast.clamp(Duration::from_millis(25), Duration::from_millis(120))
                } else if active {
                    Duration::from_millis(120)
                } else {
                    Duration::from_millis(300)
                };
                let first = cmd_rx.recv_timeout(wait);
                if matches!(first, Err(mpsc::RecvTimeoutError::Disconnected)) {
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(cancel) = active_service_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(bundle) = metrics_ws.take() {
                        bundle.stop_and_join();
                    }
                    if let Some(bundle) = logs_ws.take() {
                        bundle.stop_and_join();
                    }
                    break;
                }
                let mut batch = Vec::new();
                if let Ok(cmd) = first {
                    batch.push(cmd);
                    while let Ok(more) = cmd_rx.try_recv() {
                        batch.push(more);
                    }
                }

                // Drain real-time WebSocket updates without blocking command handling.
                let mut ws_snap_dirty = false;
                while let Ok(msg) = ws_rx.try_recv() {
                    match msg {
                        WsUpdate::Traffic { up, down } => {
                            last_ws_traffic = Some(Instant::now());
                            tracker.record_ws_traffic(up, down, &mut snap);
                            if let Some((base_secs, anchor)) = uptime_anchor {
                                snap.summary.uptime_secs =
                                    Some(base_secs.saturating_add(anchor.elapsed().as_secs()));
                            }
                            snap.updated_at = Some(Instant::now());
                            ws_snap_dirty = true;
                        }
                        WsUpdate::Memory { inuse } => {
                            snap.memory_bytes = Some(inuse);
                            ws_snap_dirty = true;
                        }
                        WsUpdate::LogLine(line) => {
                            let _ = push_bounded(&evt_tx, ClashEvent::LogStreamLine(line));
                        }
                    }
                }

                let mut force_full = false;
                let mut force_rules = false;
                let mut force_providers = false;
                let mut refresh_once = false;
                let mut shutdown = false;

                for cmd in batch {
                    match cmd {
                        ClashMonitorCmd::Wake(t) => {
                            let switched = !active || tab != t;
                            active = true;
                            left_at = None;
                            tab = t;
                            if switched {
                                force_full = true;
                                if t == ClashTab::Rules || !snap.fetched_rules {
                                    force_rules = true;
                                }
                                if matches!(
                                    t,
                                    ClashTab::Overview
                                        | ClashTab::Proxies
                                        | ClashTab::Profiles
                                        | ClashTab::Rules
                                ) {
                                    force_providers = true;
                                }
                            }
                        }
                        ClashMonitorCmd::SetTab(t) => {
                            if tab != t {
                                tab = t;
                                force_full = true;
                                if t == ClashTab::Rules && !snap.fetched_rules {
                                    force_rules = true;
                                }
                                if matches!(
                                    t,
                                    ClashTab::Overview
                                        | ClashTab::Proxies
                                        | ClashTab::Profiles
                                        | ClashTab::Rules
                                ) && !snap.fetched_providers
                                {
                                    force_providers = true;
                                }
                            }
                        }
                        ClashMonitorCmd::ForceRefresh => {
                            active = true;
                            left_at = None;
                            cached_settings = None;
                            ka_client = None;
                            version_cached.clear();
                            uptime_anchor = None;
                            force_full = true;
                            force_rules = tab == ClashTab::Rules || !snap.fetched_rules;
                            force_providers = matches!(
                                tab,
                                ClashTab::Overview
                                    | ClashTab::Proxies
                                    | ClashTab::Profiles
                                    | ClashTab::Rules
                            );
                        }
                        ClashMonitorCmd::RefreshOnce => {
                            refresh_once = true;
                        }
                        ClashMonitorCmd::LeaveClash => {
                            if left_at.is_none() {
                                left_at = Some(Instant::now());
                            }
                        }
                        ClashMonitorCmd::Shutdown => {
                            shutdown = true;
                            break;
                        }
                        ClashMonitorCmd::SelectProxy { group, node } => {
                            let policy = close_on_switch;
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::SelectProxy,
                                true,
                                move |c| {
                                    let _ = controller::select_proxy_live_with_policy(
                                        &c, &group, &node, policy,
                                    )?;
                                    Ok(format!("{group} -> {node}"))
                                },
                            );
                        }
                        ClashMonitorCmd::UnfixGroup { group } => {
                            let policy = close_on_switch;
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UnfixGroup,
                                true,
                                move |c| {
                                    let _ = controller::unfix_group_live_with_policy(
                                        &c, &group, policy,
                                    )?;
                                    Ok(group)
                                },
                            );
                        }
                        ClashMonitorCmd::SetMode { mode } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::SetMode,
                                true,
                                move |c| {
                                    controller::mode(&c, &mode, true)?;
                                    Ok(mode)
                                },
                            );
                        }
                        ClashMonitorCmd::TestGroupDelay {
                            group,
                            url,
                            timeout_ms,
                        } => {
                            let members: Vec<String> = snap
                                .groups
                                .iter()
                                .find(|g| g.name == group)
                                .map(|g| g.members.iter().map(|m| m.name.clone()).collect())
                                .unwrap_or_default();
                            spawn_streaming_group_delay(
                                &ctx, &evt_tx, &self_tx, group, members, url, timeout_ms,
                            );
                        }
                        ClashMonitorCmd::TestNodeDelay {
                            group,
                            node,
                            url,
                            timeout_ms,
                        } => {
                            let c = ctx.clone();
                            let tx = evt_tx.clone();
                            let refresh = self_tx.clone();
                            thread::spawn(move || {
                                let result = controller::delay_node_ms(&c, &node, &url, timeout_ms);
                                let _ = push_bounded(
                                    &tx,
                                    ClashEvent::NodeDelayDone {
                                        group,
                                        node,
                                        result,
                                    },
                                );
                                let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
                            });
                        }
                        ClashMonitorCmd::CloseConnection { id } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::CloseConn,
                                false,
                                move |c| {
                                    controller::close_connection(&c, &id)?;
                                    Ok(id)
                                },
                            );
                        }
                        ClashMonitorCmd::CloseAllConnections => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::CloseAllConns,
                                false,
                                move |c| {
                                    controller::close_all_connections(&c)?;
                                    Ok(String::new())
                                },
                            );
                        }
                        ClashMonitorCmd::SwitchProfile { name } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::SwitchProfile,
                                true,
                                move |c| {
                                    let reloaded = super::subscription_use_and_reload(&c, &name)?;
                                    Ok(if reloaded {
                                        format!("reloaded:{name}")
                                    } else {
                                        format!("saved:{name}")
                                    })
                                },
                            );
                        }
                        ClashMonitorCmd::UpdateProfile { name } => {
                            let tag_name = name.clone();
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UpdateProfile(tag_name),
                                false,
                                move |c| {
                                    let reloaded =
                                        super::subscription_update_and_reload(&c, &name)?;
                                    Ok(if reloaded {
                                        format!("reloaded:{name}")
                                    } else {
                                        format!("updated:{name}")
                                    })
                                },
                            );
                        }
                        ClashMonitorCmd::UpdateAllProfiles => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UpdateAllProfiles,
                                false,
                                move |c| {
                                    let res = super::subscription_update_all(&c)?;
                                    Ok(format!(
                                        "{}|{}|{}",
                                        res.updated,
                                        res.failures.len(),
                                        u8::from(res.reloaded)
                                    ))
                                },
                            );
                        }
                        ClashMonitorCmd::RemoveProfile { name } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::RemoveProfile,
                                false,
                                move |c| {
                                    super::subscription_remove(&c, &name)?;
                                    Ok(name)
                                },
                            );
                        }
                        ClashMonitorCmd::UpdateProxyProvider { name } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UpdateProvider,
                                false,
                                move |c| {
                                    controller::update_proxy_provider(&c, &name)?;
                                    Ok(name)
                                },
                            );
                        }
                        ClashMonitorCmd::HealthcheckProxyProvider { name } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::HealthcheckProvider,
                                false,
                                move |c| {
                                    controller::healthcheck_proxy_provider(&c, &name)?;
                                    Ok(name)
                                },
                            );
                        }
                        ClashMonitorCmd::UpdateRuleProvider { name } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UpdateProvider,
                                false,
                                move |c| {
                                    controller::update_rule_provider(&c, &name)?;
                                    Ok(name)
                                },
                            );
                        }
                        ClashMonitorCmd::EnableLocalController => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::EnableLocalController,
                                false,
                                |c| controller::enable_loopback_and_apply(&c),
                            );
                        }
                        ClashMonitorCmd::ProbeExit => {
                            pending_route_probe_at = None;
                            auto_probed_ready = true;
                            launch_outbound_probe(
                                &ctx,
                                &evt_tx,
                                &quality_cache,
                                &quality_cache_path,
                                &mut probe_generation,
                                &mut active_probe_cancel,
                                true,
                                true,
                            );
                        }
                        ClashMonitorCmd::CheckServices { targets, manual } => {
                            failed_recheck_at = None;
                            failed_recheck_step = 0;
                            let route = route_context_from(&snap);
                            launch_service_check(
                                &ctx,
                                &evt_tx,
                                route,
                                targets,
                                &mut service_generation,
                                &mut active_service_cancel,
                                manual,
                                None,
                            );
                        }
                        ClashMonitorCmd::SetPinnedServices(targets) => {
                            if !targets.is_empty() {
                                pinned_services = targets;
                            }
                        }
                        ClashMonitorCmd::SetCloseOnSwitch(policy) => {
                            close_on_switch = policy;
                        }
                        ClashMonitorCmd::SetLogStream(enabled) => {
                            log_stream_requested = enabled;
                            if !enabled && let Some(bundle) = logs_ws.take() {
                                bundle.stop_nonblocking();
                            }
                        }
                        ClashMonitorCmd::RefreshProfileUserinfo { name } => {
                            userinfo_probed_profiles.insert(name.clone());
                            spawn_userinfo_backfill(&ctx, &evt_tx, &self_tx, name);
                        }
                        ClashMonitorCmd::RouteChanged => {
                            failed_recheck_at = None;
                            failed_recheck_step = 0;
                            if active && snap.availability.is_ready() {
                                pending_route_probe_at = Some(Instant::now() + ROUTE_SETTLE_DELAY);
                            }
                        }
                        ClashMonitorCmd::FetchLogs => {
                            let c = ctx.clone();
                            let tx = evt_tx.clone();
                            thread::spawn(move || {
                                let result = super::read_redacted_lines(&c, MAX_CLASH_LOG_LINES);
                                let _ = push_bounded(&tx, ClashEvent::LogsLoaded(result));
                            });
                        }
                    }
                }

                if shutdown {
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(cancel) = active_service_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(bundle) = metrics_ws.take() {
                        bundle.stop_and_join();
                    }
                    if let Some(bundle) = logs_ws.take() {
                        bundle.stop_and_join();
                    }
                    break;
                }

                if let Some(left) = left_at
                    && left.elapsed() >= IDLE_SLEEP_AFTER
                {
                    active = false;
                    left_at = None;
                    failed_recheck_at = None;
                    stop_all_ws(&mut metrics_ws, &mut logs_ws);
                    last_ws_traffic = None;
                }

                // Manage real-time WebSocket streams cleanly based on active + ready state.
                if active && snap.availability.is_ready() {
                    if let Some(ref s) = cached_settings {
                        if metrics_ws
                            .as_ref()
                            .is_none_or(|b| b.target_addr != s.address)
                        {
                            if let Some(old) = metrics_ws.take() {
                                old.stop_nonblocking();
                            }
                            metrics_ws = Some(WsBundle::start_metrics(s, &ws_tx, &ctx.cancel));
                        }
                        if log_stream_requested {
                            if logs_ws.as_ref().is_none_or(|b| b.target_addr != s.address) {
                                if let Some(old) = logs_ws.take() {
                                    old.stop_nonblocking();
                                }
                                logs_ws = Some(WsBundle::start_logs(&ctx, s, &ws_tx, &ctx.cancel));
                            }
                        } else if let Some(old) = logs_ws.take() {
                            old.stop_nonblocking();
                        }
                    }
                } else {
                    stop_all_ws(&mut metrics_ws, &mut logs_ws);
                    last_ws_traffic = None;
                }

                // Trigger debounced route-change outbound IP & pinned services re-probe once Mihomo settles.
                if active
                    && snap.availability.is_ready()
                    && pending_route_probe_at.is_some_and(|at| Instant::now() >= at)
                {
                    pending_route_probe_at = None;
                    auto_probed_ready = true;
                    failed_recheck_step = 0;
                    launch_outbound_probe(
                        &ctx,
                        &evt_tx,
                        &quality_cache,
                        &quality_cache_path,
                        &mut probe_generation,
                        &mut active_probe_cancel,
                        false,
                        false,
                    );
                    let route = route_context_from(&snap);
                    if let Ok(mut guard) = failed_pinned.lock() {
                        guard.clear();
                    }
                    launch_service_check(
                        &ctx,
                        &evt_tx,
                        route,
                        pinned_services.clone(),
                        &mut service_generation,
                        &mut active_service_cancel,
                        false,
                        Some(failed_pinned.clone()),
                    );
                    failed_recheck_at =
                        Some(Instant::now() + Duration::from_secs(FAILED_SERVICE_BACKOFF_SECS[0]));
                }

                // Bounded exponential-backoff recovery for failed Overview pinned services (60s -> 120s -> 300s).
                if active
                    && snap.availability.is_ready()
                    && pending_route_probe_at.is_none()
                    && failed_recheck_at.is_some_and(|at| Instant::now() >= at)
                {
                    let to_retry: Vec<ServiceTarget> = failed_pinned
                        .lock()
                        .map(|mut g| std::mem::take(&mut *g))
                        .unwrap_or_default();
                    if to_retry.is_empty() {
                        failed_recheck_at = None;
                        failed_recheck_step = 0;
                    } else {
                        let next_step = (failed_recheck_step as usize + 1)
                            .min(FAILED_SERVICE_BACKOFF_SECS.len() - 1);
                        failed_recheck_step = next_step as u8;
                        failed_recheck_at = Some(
                            Instant::now()
                                + Duration::from_secs(FAILED_SERVICE_BACKOFF_SECS[next_step]),
                        );
                        let route = route_context_from(&snap);
                        launch_service_check(
                            &ctx,
                            &evt_tx,
                            route,
                            to_retry,
                            &mut service_generation,
                            &mut active_service_cancel,
                            false,
                            Some(failed_pinned.clone()),
                        );
                    }
                }

                let now = Instant::now();
                let ws_traffic_active =
                    last_ws_traffic.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                // When `/traffic` WS is streaming live speeds, only poll `/connections` at 1 Hz on
                // tabs that actually render connection/flow details (`Overview` and `Connections`).
                let tab_needs_conns = matches!(tab, ClashTab::Overview | ClashTab::Connections);
                let needs_fast = active
                    && snap.availability.is_ready()
                    && (tab_needs_conns || !ws_traffic_active)
                    && now.saturating_duration_since(last_fast) >= FAST_POLL_INTERVAL;
                let needs_full = active
                    && (force_full
                        || (matches!(
                            tab,
                            ClashTab::Overview | ClashTab::Proxies | ClashTab::Connections
                        ) && now.saturating_duration_since(last_full) >= FULL_POLL_INTERVAL)
                        || (matches!(tab, ClashTab::Profiles | ClashTab::Tools)
                            && now.saturating_duration_since(last_slow) >= SLOW_POLL_INTERVAL));

                if !needs_fast && !needs_full && !refresh_once && !force_rules && !force_providers {
                    if ws_snap_dirty
                        && !push_bounded(&evt_tx, ClashEvent::Snapshot(Box::new(snap.clone())))
                    {
                        break;
                    }
                    continue;
                }

                let did_full = needs_full || refresh_once;
                let prev_ready = snap.availability.is_ready();

                poll_step(
                    &ctx,
                    tab,
                    did_full,
                    (needs_fast || (did_full && tab_needs_conns))
                        || (!ws_traffic_active && did_full),
                    ws_traffic_active,
                    force_rules || (did_full && !snap.fetched_rules),
                    force_providers,
                    &mut version_cached,
                    &mut cached_settings,
                    &mut ka_client,
                    &mut uptime_anchor,
                    &mut tracker,
                    &mut snap,
                );

                if did_full
                    && now.saturating_duration_since(last_log_rotate) >= LOG_ROTATE_CHECK_INTERVAL
                {
                    last_log_rotate = now;
                    let _ = super::logs::rotate_if_oversized(&ctx);
                }

                if prev_ready && !snap.availability.is_ready() {
                    tracker.reset();
                    version_cached.clear();
                    cached_settings = None;
                    ka_client = None;
                    uptime_anchor = None;
                    auto_probed_ready = false;
                    pending_route_probe_at = None;
                    prev_route_sig = None;
                    failed_recheck_at = None;
                    failed_recheck_step = 0;
                    last_ws_traffic = None;
                    stop_all_ws(&mut metrics_ws, &mut logs_ws);
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    if let Some(cancel) = active_service_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }

                // Automatic route drift detection: notice URLTest/Fallback auto-switches or
                // background proxy-provider updates and re-probe exit IP + pinned services.
                if did_full && snap.availability.is_ready() && !snap.groups.is_empty() {
                    let next_sig = route_signature(&snap);
                    if let Some(ref prev) = prev_route_sig
                        && *prev != next_sig
                        && auto_probed_ready
                        && pending_route_probe_at.is_none()
                    {
                        pending_route_probe_at = Some(Instant::now() + ROUTE_SETTLE_DELAY);
                    }
                    prev_route_sig = Some(next_sig);
                }

                let done_at = Instant::now();
                if needs_fast || did_full {
                    last_fast = done_at;
                }
                if did_full {
                    last_full = done_at;
                    last_slow = done_at;
                }

                if !push_bounded(&evt_tx, ClashEvent::Snapshot(Box::new(snap.clone()))) {
                    break;
                }

                // Automatically backfill missing `subscription-userinfo` once per session for a
                // remote active profile without rewriting its YAML config or restarting Mihomo.
                if active
                    && matches!(tab, ClashTab::Overview | ClashTab::Profiles)
                    && let Some(prof) = snap.active_profile()
                    && !prof.is_local
                    && prof.userinfo.is_none()
                    && userinfo_probed_profiles.insert(prof.name.clone())
                {
                    spawn_userinfo_backfill(&ctx, &evt_tx, &self_tx, prof.name.clone());
                }

                // Automatically probe outbound IP, quality, and pinned services once Mihomo becomes ready.
                if active && snap.availability.is_ready() && !auto_probed_ready {
                    auto_probed_ready = true;
                    failed_recheck_step = 0;
                    launch_outbound_probe(
                        &ctx,
                        &evt_tx,
                        &quality_cache,
                        &quality_cache_path,
                        &mut probe_generation,
                        &mut active_probe_cancel,
                        false,
                        false,
                    );
                    let route = route_context_from(&snap);
                    if let Ok(mut guard) = failed_pinned.lock() {
                        guard.clear();
                    }
                    launch_service_check(
                        &ctx,
                        &evt_tx,
                        route,
                        pinned_services.clone(),
                        &mut service_generation,
                        &mut active_service_cancel,
                        false,
                        Some(failed_pinned.clone()),
                    );
                    failed_recheck_at =
                        Some(Instant::now() + Duration::from_secs(FAILED_SERVICE_BACKOFF_SECS[0]));
                }
            }
        });

        Self {
            tx: cmd_tx,
            rx: evt_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: ClashMonitorCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn drain(&self) -> Vec<ClashEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            out.push(evt);
        }
        out
    }
}

impl Drop for ClashMonitor {
    fn drop(&mut self) {
        let _ = self.tx.send(ClashMonitorCmd::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Persistent IP quality cache entry (72-hour TTL, successes only, private `0600` JSON file).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedQualityEntry {
    cached_at_secs: u64,
    quality: IpQuality,
}

type QualityCache = Arc<Mutex<HashMap<String, PersistedQualityEntry>>>;

fn quality_cache_file(ctx: &Context) -> PathBuf {
    ctx.paths.clash_data.join("ip-quality-cache.json")
}

fn load_quality_cache_disk(path: &Path) -> HashMap<String, PersistedQualityEntry> {
    let mut map = HashMap::new();
    if storage::check_path(path, true).is_err() {
        return map;
    }
    let Ok(meta) = fs::metadata(path) else {
        return map;
    };
    if !meta.is_file() || meta.len() > MAX_QUALITY_CACHE_FILE_BYTES {
        return map;
    }
    let Ok(bytes) = fs::read(path) else {
        return map;
    };
    let Ok(entries) = serde_json::from_slice::<Vec<PersistedQualityEntry>>(&bytes) else {
        return map;
    };
    let now = now_epoch_secs();
    for entry in entries {
        if now.saturating_sub(entry.cached_at_secs) < QUALITY_CACHE_TTL_SECS {
            map.insert(entry.quality.ip.clone(), entry);
            if map.len() >= QUALITY_CACHE_CAP {
                break;
            }
        }
    }
    map
}

fn save_quality_cache_disk(path: &Path, map: &HashMap<String, PersistedQualityEntry>) {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        return;
    }
    let entries: Vec<&PersistedQualityEntry> = map.values().collect();
    if let Ok(bytes) = serde_json::to_vec(&entries) {
        let _ = storage::atomic_write(path, &bytes, 0o600);
    }
}

fn cache_get_quality(cache: &QualityCache, ip: &str) -> Option<IpQuality> {
    let guard = cache.lock().ok()?;
    let entry = guard.get(ip)?;
    if now_epoch_secs().saturating_sub(entry.cached_at_secs) < QUALITY_CACHE_TTL_SECS {
        Some(entry.quality.clone())
    } else {
        None
    }
}

fn cache_put_quality(cache: &QualityCache, path: &Path, quality: IpQuality) {
    let Ok(mut guard) = cache.lock() else {
        return;
    };
    let now = now_epoch_secs();
    guard.retain(|_, entry| now.saturating_sub(entry.cached_at_secs) < QUALITY_CACHE_TTL_SECS);
    while guard.len() >= QUALITY_CACHE_CAP {
        let Some(oldest_key) = guard
            .iter()
            .min_by_key(|(_, entry)| entry.cached_at_secs)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        guard.remove(&oldest_key);
    }
    guard.insert(
        quality.ip.clone(),
        PersistedQualityEntry {
            cached_at_secs: now,
            quality,
        },
    );
    save_quality_cache_disk(path, &guard);
}

#[allow(clippy::too_many_arguments)]
fn launch_outbound_probe(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    quality_cache: &QualityCache,
    quality_cache_path: &Path,
    probe_generation: &mut u64,
    active_probe_cancel: &mut Option<Arc<AtomicBool>>,
    force_quality: bool,
    manual: bool,
) {
    if let Some(prev) = active_probe_cancel.take() {
        prev.store(true, Ordering::SeqCst);
    }
    *probe_generation = probe_generation.wrapping_add(1).max(1);
    let generation = *probe_generation;
    let cancel = Arc::new(AtomicBool::new(false));
    *active_probe_cancel = Some(cancel.clone());

    let _ = push_bounded(
        evt_tx,
        ClashEvent::OutboundProbeStarted { generation, manual },
    );

    let c = ctx.clone();
    let tx = evt_tx.clone();
    let cache = quality_cache.clone();
    let cache_path = quality_cache_path.to_path_buf();
    thread::spawn(move || {
        let transport = match outbound_ip::resolve_transport(&c) {
            Ok(t) => t,
            Err(err) => {
                if !cancel.load(Ordering::SeqCst) {
                    let _ = push_bounded(
                        &tx,
                        ClashEvent::OutboundIpResolved {
                            generation,
                            result: Err(err),
                            manual,
                        },
                    );
                }
                return;
            }
        };

        let info = match outbound_ip::lookup_outbound_ip(&c, &transport, &cancel) {
            Ok(info) => info,
            Err(err) => {
                if !cancel.load(Ordering::SeqCst) {
                    let _ = push_bounded(
                        &tx,
                        ClashEvent::OutboundIpResolved {
                            generation,
                            result: Err(err),
                            manual,
                        },
                    );
                }
                return;
            }
        };

        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let ip = info.ip.clone();
        let _ = push_bounded(
            &tx,
            ClashEvent::OutboundIpResolved {
                generation,
                result: Ok(info),
                manual,
            },
        );

        // Stage 2: 4-wave hedged IP quality lookup (reusing 72h persistent cache unless manually refreshed).
        if !force_quality && let Some(cached) = cache_get_quality(&cache, &ip) {
            let _ = push_bounded(
                &tx,
                ClashEvent::OutboundQualityResolved {
                    generation,
                    ip,
                    outcome: IpQualityOutcome::Ready(cached),
                },
            );
            return;
        }

        let outcome = match outbound_ip::lookup_ip_quality(&c, &transport, &ip, &cancel) {
            Ok(quality) => {
                cache_put_quality(&cache, &cache_path, quality.clone());
                IpQualityOutcome::Ready(quality)
            }
            Err(failures) => IpQualityOutcome::Failed(failures),
        };
        if !cancel.load(Ordering::SeqCst) {
            let _ = push_bounded(
                &tx,
                ClashEvent::OutboundQualityResolved {
                    generation,
                    ip,
                    outcome,
                },
            );
        }
    });
}

fn route_context_from(snap: &ClashSnapshot) -> service_check::RouteContext {
    service_check::RouteContext {
        mode: snap.summary.mode.clone(),
        rules: snap.rules.clone(),
        groups: snap.groups.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn launch_service_check(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    route: service_check::RouteContext,
    targets: Vec<ServiceTarget>,
    service_generation: &mut u64,
    active_service_cancel: &mut Option<Arc<AtomicBool>>,
    manual: bool,
    record_failures: Option<Arc<Mutex<Vec<ServiceTarget>>>>,
) {
    if targets.is_empty() {
        return;
    }
    if let Some(prev) = active_service_cancel.take() {
        prev.store(true, Ordering::SeqCst);
    }
    *service_generation = service_generation.wrapping_add(1).max(1);
    let generation = *service_generation;
    let cancel = Arc::new(AtomicBool::new(false));
    *active_service_cancel = Some(cancel.clone());

    let _ = push_bounded(
        evt_tx,
        ClashEvent::ServiceCheckStarted {
            generation,
            targets: targets.clone(),
        },
    );

    let c = ctx.clone();
    let tx = evt_tx.clone();
    thread::spawn(move || {
        let transport = match outbound_ip::resolve_transport(&c) {
            Ok(t) => t,
            Err(_) => {
                if !cancel.load(Ordering::SeqCst) {
                    let now = Instant::now();
                    for target in targets {
                        if let Some(ref failures) = record_failures
                            && let Ok(mut guard) = failures.lock()
                            && !guard.contains(&target)
                        {
                            guard.push(target);
                        }
                        let _ = push_bounded(
                            &tx,
                            ClashEvent::ServiceCheckItemDone {
                                generation,
                                item: ServiceCheckItem {
                                    target,
                                    status: ServiceProbeStatus::Failed,
                                    delay_ms: None,
                                    region: None,
                                    ip: None,
                                    exit_node: None,
                                    checked_at: now,
                                },
                            },
                        );
                    }
                    let _ =
                        push_bounded(&tx, ClashEvent::ServiceCheckFinished { generation, manual });
                }
                return;
            }
        };

        let tx_item = tx.clone();
        let failures_sink = record_failures.clone();
        service_check::run_service_sweep(&c, &transport, &route, &targets, &cancel, move |item| {
            if let Some(ref failures) = failures_sink
                && matches!(
                    item.status,
                    ServiceProbeStatus::Failed | ServiceProbeStatus::Timeout
                )
                && let Ok(mut guard) = failures.lock()
                && !guard.contains(&item.target)
            {
                guard.push(item.target);
            }
            let _ = push_bounded(
                &tx_item,
                ClashEvent::ServiceCheckItemDone { generation, item },
            );
        });

        if !cancel.load(Ordering::SeqCst) {
            let _ = push_bounded(&tx, ClashEvent::ServiceCheckFinished { generation, manual });
        }
    });
}

fn spawn_streaming_group_delay(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    self_tx: &Sender<ClashMonitorCmd>,
    group: String,
    members: Vec<String>,
    url: String,
    timeout_ms: u64,
) {
    let c = ctx.clone();
    let tx = evt_tx.clone();
    let refresh = self_tx.clone();
    thread::spawn(move || {
        if members.is_empty() {
            let result = controller::delay_group_map(&c, &group, &url, timeout_ms);
            let _ = push_bounded(&tx, ClashEvent::GroupDelayDone { group, result });
            let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
            return;
        }
        let total = members.len();
        let queue = Mutex::new(VecDeque::from(members));
        let results: Mutex<BTreeMap<String, Option<u64>>> = Mutex::new(BTreeMap::new());
        let completed = AtomicUsize::new(0);
        let workers = DELAY_POOL_CONCURRENCY.min(total).max(1);

        thread::scope(|s| {
            for _ in 0..workers {
                let c_ref = &c;
                let tx_ref = &tx;
                let group_ref = &group;
                let url_ref = &url;
                let q_ref = &queue;
                let res_ref = &results;
                let done_ref = &completed;
                s.spawn(move || {
                    loop {
                        if c_ref.cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        let next = q_ref.lock().ok().and_then(|mut q| q.pop_front());
                        let Some(node) = next else {
                            break;
                        };
                        let delay_ms = controller::delay_node_ms(c_ref, &node, url_ref, timeout_ms)
                            .ok()
                            .flatten();
                        if let Ok(mut map) = res_ref.lock() {
                            map.insert(node.clone(), delay_ms);
                        }
                        let count = done_ref.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = push_bounded(
                            tx_ref,
                            ClashEvent::NodeDelayTick {
                                group: group_ref.clone(),
                                node,
                                delay_ms,
                                completed: count,
                                total,
                            },
                        );
                    }
                });
            }
        });

        let final_map = results.into_inner().unwrap_or_default();
        let _ = push_bounded(
            &tx,
            ClashEvent::GroupDelayDone {
                group,
                result: Ok(final_map),
            },
        );
        let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
    });
}

fn spawn_userinfo_backfill(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    self_tx: &Sender<ClashMonitorCmd>,
    name: String,
) {
    let _ = push_bounded(
        evt_tx,
        ClashEvent::ProfileUserinfoState {
            name: name.clone(),
            probing: true,
            found: None,
        },
    );
    let c = ctx.clone();
    let tx = evt_tx.clone();
    let refresh = self_tx.clone();
    thread::spawn(move || {
        let res = super::refresh_profile_userinfo(&c, &name);
        let found = matches!(res, Ok(Some(_)));
        let _ = push_bounded(
            &tx,
            ClashEvent::ProfileUserinfoState {
                name,
                probing: false,
                found: Some(found),
            },
        );
        if found {
            let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
        }
    });
}

fn spawn_action<F>(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    self_tx: &Sender<ClashMonitorCmd>,
    tag: ClashActionTag,
    triggers_route_probe: bool,
    work: F,
) where
    F: FnOnce(Context) -> Result<String> + Send + 'static,
{
    let c = ctx.clone();
    let tx = evt_tx.clone();
    let refresh = self_tx.clone();
    thread::spawn(move || {
        let result = work(c);
        let ok = result.is_ok();
        let _ = push_bounded(&tx, ClashEvent::ActionDone { tag, result });
        if ok && triggers_route_probe {
            let _ = refresh.send(ClashMonitorCmd::RouteChanged);
        }
        let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
    });
}

fn preserve_panel_summary(
    prev: &super::model::ClashRuntimeSummary,
    next: &mut super::model::ClashRuntimeSummary,
) {
    next.panel_installed = prev.panel_installed;
    if next.panel_name.is_none() {
        next.panel_name = prev.panel_name.clone();
    }
    if next.panel_version.is_none() {
        next.panel_version = prev.panel_version.clone();
    }
    if next.panel_url.is_none() {
        next.panel_url = prev.panel_url.clone();
    }
    if next.panel_bind.is_none() {
        next.panel_bind = prev.panel_bind.clone();
    }
    next.panel_loopback = prev.panel_loopback;
}

fn preserve_controller_summary(
    prev: &super::model::ClashRuntimeSummary,
    next: &mut super::model::ClashRuntimeSummary,
    version_cached: &str,
    preserve_panel: bool,
) {
    if !version_cached.is_empty() {
        next.version = version_cached.to_owned();
    } else if next.version.is_empty() {
        next.version = prev.version.clone();
    }
    if next.mode.is_empty() {
        next.mode = prev.mode.clone();
    }
    next.mixed_port = prev.mixed_port.or(next.mixed_port);
    next.http_port = prev.http_port.or(next.http_port);
    next.socks_port = prev.socks_port.or(next.socks_port);
    next.allow_lan = prev.allow_lan;
    next.tun_enabled = prev.tun_enabled;
    if next.tun_stack.is_empty() {
        next.tun_stack = prev.tun_stack.clone();
    }
    if preserve_panel {
        preserve_panel_summary(prev, next);
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_step(
    ctx: &Context,
    tab: ClashTab,
    full: bool,
    fetch_conns: bool,
    ws_traffic_active: bool,
    force_rules: bool,
    force_providers: bool,
    version_cached: &mut String,
    cached_settings: &mut Option<controller::Settings>,
    ka_client: &mut Option<KeepAliveClient>,
    uptime_anchor: &mut Option<(u64, Instant)>,
    tracker: &mut ConnectionTracker,
    snap: &mut ClashSnapshot,
) {
    let now = Instant::now();

    // Lightweight 1 Hz fast path: reuse Keep-Alive TCP connection and cached controller settings.
    if !full
        && !force_rules
        && !force_providers
        && fetch_conns
        && snap.availability.is_ready()
        && let Some(settings) = cached_settings.as_ref()
    {
        if ka_client
            .as_ref()
            .is_none_or(|c| !c.matches_settings(settings))
        {
            *ka_client = Some(KeepAliveClient::new(settings));
        }
        if let Some(client) = ka_client.as_mut() {
            match client.fetch_connections_raw(ctx) {
                Ok(raw) => {
                    tracker.ingest_with_mode(raw, now, ws_traffic_active, snap);
                    if let Some((base_secs, anchor)) = *uptime_anchor {
                        snap.summary.uptime_secs = Some(
                            base_secs
                                .saturating_add(now.saturating_duration_since(anchor).as_secs()),
                        );
                    }
                    snap.availability = ClashAvailability::Ready;
                    snap.last_error = None;
                    snap.updated_at = Some(now);
                    return;
                }
                Err(_) => {
                    *cached_settings = None;
                    *ka_client = None;
                }
            }
        }
    }

    let include_panel = full || !snap.availability.is_ready();
    let (local_avail, mut local_summary) = super::read_local_runtime_summary(ctx, include_panel);
    if !include_panel {
        preserve_panel_summary(&snap.summary, &mut local_summary);
    }
    if full || snap.profiles.is_empty() {
        snap.profiles = super::profile_summaries(ctx);
    }

    if !local_avail.is_ready() {
        *cached_settings = None;
        *ka_client = None;
        *uptime_anchor = None;
        snap.availability = local_avail;
        snap.summary = local_summary;
        snap.groups.clear();
        snap.connections.clear();
        snap.up_speed = 0;
        snap.down_speed = 0;
        snap.updated_at = Some(now);
        return;
    }

    if let Some(secs) = local_summary.uptime_secs {
        *uptime_anchor = Some((secs, now));
    }

    let settings = match controller::effective_settings(ctx) {
        Ok(Some(s)) => s,
        Ok(None) => {
            *cached_settings = None;
            *ka_client = None;
            snap.availability = ClashAvailability::ControllerUnconfigured;
            snap.summary = local_summary;
            snap.updated_at = Some(now);
            return;
        }
        Err(err) => {
            *cached_settings = None;
            *ka_client = None;
            snap.availability = ClashAvailability::ControllerUnreachable(err.detail);
            snap.summary = local_summary;
            snap.updated_at = Some(now);
            return;
        }
    };

    if ka_client
        .as_ref()
        .is_none_or(|c| !c.matches_settings(&settings))
    {
        *ka_client = Some(KeepAliveClient::new(&settings));
    }
    let client = ka_client.as_mut().expect("keep-alive client initialized");

    let mut summary = local_summary;
    summary.controller_url = Some(settings.url.clone());
    summary.controller_has_secret = settings.has_secret();
    preserve_controller_summary(&snap.summary, &mut summary, version_cached, !include_panel);

    if fetch_conns {
        match client.fetch_connections_raw(ctx) {
            Ok(raw) => {
                tracker.ingest_with_mode(raw, now, ws_traffic_active, snap);
                snap.availability = ClashAvailability::Ready;
                snap.last_error = None;
            }
            Err(err) => {
                *cached_settings = None;
                *ka_client = None;
                snap.availability = ClashAvailability::ControllerUnreachable(err.detail.clone());
                snap.last_error = Some(err.detail);
                snap.summary = summary;
                snap.updated_at = Some(now);
                return;
            }
        }
    }

    if full {
        if version_cached.is_empty()
            && let Ok(ver) = client.fetch_version(ctx)
        {
            *version_cached = ver;
        }
        if !version_cached.is_empty() {
            summary.version = version_cached.clone();
        }
        match client.fetch_configs(ctx) {
            Ok(cfg) => {
                summary.mode = cfg.mode;
                summary.mixed_port = cfg.mixed_port.or(summary.mixed_port);
                summary.http_port = cfg.http_port.or(summary.http_port);
                summary.socks_port = cfg.socks_port.or(summary.socks_port);
                summary.allow_lan = cfg.allow_lan;
                summary.tun_enabled = cfg.tun_enabled;
                if !cfg.tun_stack.is_empty() {
                    summary.tun_stack = cfg.tun_stack;
                }
                snap.availability = ClashAvailability::Ready;
                snap.last_error = None;
            }
            Err(err) => {
                *ka_client = None;
                snap.availability = ClashAvailability::ControllerUnreachable(err.detail.clone());
                snap.last_error = Some(err.detail);
                snap.summary = summary;
                snap.updated_at = Some(now);
                return;
            }
        }
        if matches!(tab, ClashTab::Overview | ClashTab::Proxies) || snap.groups.is_empty() {
            match client.fetch_groups(ctx) {
                Ok(groups) => snap.groups = groups,
                Err(err) => snap.last_error = Some(err.detail),
            }
        }
    }

    if force_providers {
        if let Ok(pp) = client.fetch_proxy_providers(ctx) {
            snap.proxy_providers = pp;
        }
        if let Ok(rp) = client.fetch_rule_providers(ctx) {
            snap.rule_providers = rp;
        }
        snap.fetched_providers = true;
    }

    if force_rules {
        match client.fetch_rules(ctx) {
            Ok(rules) => {
                snap.rules = rules;
                snap.fetched_rules = true;
            }
            Err(err) => snap.last_error = Some(err.detail),
        }
    }

    if snap.availability.is_ready() {
        *cached_settings = Some(settings);
    }
    snap.summary = summary;
    snap.updated_at = Some(now);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_tracker_diffs_speeds_and_archives_closed_connections() {
        let mut tracker = ConnectionTracker::default();
        let mut snap = ClashSnapshot::default();
        let t0 = Instant::now();
        let conn_a = ConnectionItem {
            id: "a".into(),
            network: "TCP".into(),
            conn_type: "HTTP".into(),
            source: "127.0.0.1:50000".into(),
            destination: "1.1.1.1:443".into(),
            host: "example.com:443".into(),
            process: "curl".into(),
            rule: "DomainSuffix".into(),
            rule_payload: "example.com".into(),
            chains: vec!["Proxy".into(), "JP-01".into()],
            exit_node: "JP-01".into(),
            upload: 1000,
            download: 2000,
            up_speed: 0,
            down_speed: 0,
            start: String::new(),
            closed: false,
        };
        tracker.ingest(
            controller::RawConnections {
                upload_total: 1000,
                download_total: 2000,
                memory: Some(64 * 1024 * 1024),
                connections: vec![conn_a.clone()],
            },
            t0,
            &mut snap,
        );
        assert_eq!(snap.connections.len(), 1);
        assert!(snap.closed_connections.is_empty());

        let mut conn_a2 = conn_a;
        conn_a2.upload = 3000;
        conn_a2.download = 10000;
        tracker.ingest(
            controller::RawConnections {
                upload_total: 3000,
                download_total: 10000,
                memory: Some(64 * 1024 * 1024),
                connections: vec![conn_a2],
            },
            t0 + Duration::from_secs(1),
            &mut snap,
        );
        assert_eq!(snap.up_speed, 2000);
        assert_eq!(snap.down_speed, 8000);
        assert_eq!(snap.connections[0].down_speed, 8000);

        // Connection closes on tick 2
        tracker.ingest(
            controller::RawConnections {
                upload_total: 3000,
                download_total: 10000,
                memory: Some(64 * 1024 * 1024),
                connections: vec![],
            },
            t0 + Duration::from_secs(2),
            &mut snap,
        );
        assert!(snap.connections.is_empty());
        assert_eq!(snap.closed_connections.len(), 1);
        assert!(snap.closed_connections[0].closed);
        assert_eq!(snap.closed_connections[0].id, "a");
    }

    #[test]
    fn connection_tracker_enforces_closed_ring_and_sparkline_bounds() {
        let mut tracker = ConnectionTracker::default();
        let mut snap = ClashSnapshot::default();
        let t0 = Instant::now();

        let make_conn = |id: usize| ConnectionItem {
            id: format!("c-{id}"),
            network: "TCP".into(),
            conn_type: "HTTP".into(),
            source: "127.0.0.1:40000".into(),
            destination: "1.1.1.1:443".into(),
            host: format!("h{id}.example.com:443"),
            process: "curl".into(),
            rule: "Match".into(),
            rule_payload: String::new(),
            chains: vec!["Proxy".into(), "JP-01".into()],
            exit_node: "JP-01".into(),
            upload: 100,
            download: 200,
            up_speed: 0,
            down_speed: 0,
            start: String::new(),
            closed: false,
        };

        let mut batch = Vec::new();
        for i in 0..250 {
            batch.push(make_conn(i));
        }
        tracker.ingest(
            controller::RawConnections {
                upload_total: 1000,
                download_total: 2000,
                memory: Some(32 * 1024 * 1024),
                connections: batch,
            },
            t0,
            &mut snap,
        );
        tracker.ingest(
            controller::RawConnections {
                upload_total: 2000,
                download_total: 4000,
                memory: Some(32 * 1024 * 1024),
                connections: Vec::new(),
            },
            t0 + Duration::from_secs(1),
            &mut snap,
        );
        assert_eq!(snap.closed_connections.len(), MAX_CLOSED_CONNECTIONS);
        assert!(snap.closed_connections.iter().all(|c| c.closed));

        for tick in 2..=(MAX_TRAFFIC_HISTORY as u64 + 25) {
            tracker.ingest(
                controller::RawConnections {
                    upload_total: 2000 + tick * 500,
                    download_total: 4000 + tick * 1000,
                    memory: Some(32 * 1024 * 1024),
                    connections: Vec::new(),
                },
                t0 + Duration::from_secs(tick),
                &mut snap,
            );
        }
        assert_eq!(snap.traffic_history.len(), MAX_TRAFFIC_HISTORY);
        assert_eq!(snap.peak_down_speed, 2000);
        assert_eq!(snap.peak_up_speed, 1000);

        let prev_len = snap.traffic_history.len();
        tracker.ingest(
            controller::RawConnections {
                upload_total: 100,
                download_total: 200,
                memory: Some(32 * 1024 * 1024),
                connections: Vec::new(),
            },
            t0 + Duration::from_secs(MAX_TRAFFIC_HISTORY as u64 + 26),
            &mut snap,
        );
        assert_eq!(snap.traffic_history.len(), prev_len);
        assert_eq!(snap.up_speed, 0);
        assert_eq!(snap.down_speed, 0);
    }

    #[test]
    fn fast_tick_preserves_controller_summary_fields_without_flicker() {
        let prev = super::super::model::ClashRuntimeSummary {
            version: "v1.19.34".into(),
            mode: "rule".into(),
            mixed_port: Some(7890),
            http_port: Some(7890),
            socks_port: Some(7891),
            allow_lan: true,
            tun_enabled: true,
            tun_stack: "mixed".into(),
            panel_installed: true,
            panel_name: Some("Zashboard".into()),
            panel_version: Some("v3.30.0".into()),
            panel_url: Some("http://127.0.0.1:9090/ui/".into()),
            panel_bind: Some("127.0.0.1:9090".into()),
            panel_loopback: true,
            uptime_secs: Some(100),
            pid: Some(1234),
            ..Default::default()
        };
        let mut next = super::super::model::ClashRuntimeSummary {
            http_port: Some(7890),
            uptime_secs: Some(101),
            pid: Some(1234),
            ..Default::default()
        };
        preserve_controller_summary(&prev, &mut next, "v1.19.34", true);
        assert_eq!(next.version, "v1.19.34");
        assert_eq!(next.mode, "rule");
        assert_eq!(next.mixed_port, Some(7890));
        assert!(next.allow_lan);
        assert!(next.tun_enabled);
        assert_eq!(next.tun_stack, "mixed");
        assert!(next.panel_installed);
        assert_eq!(next.panel_name.as_deref(), Some("Zashboard"));
        assert_eq!(next.panel_version.as_deref(), Some("v3.30.0"));
        assert_eq!(next.panel_url.as_deref(), Some("http://127.0.0.1:9090/ui/"));
        assert!(next.panel_loopback);
        assert_eq!(next.uptime_secs, Some(101));

        let mut full_next = super::super::model::ClashRuntimeSummary {
            http_port: Some(7890),
            panel_installed: false,
            ..Default::default()
        };
        preserve_controller_summary(&prev, &mut full_next, "v1.19.34", false);
        assert_eq!(full_next.version, "v1.19.34");
        assert_eq!(full_next.mode, "rule");
        assert!(!full_next.panel_installed);
    }

    #[test]
    fn persistent_quality_cache_round_trips_and_drops_expired_entries() {
        use super::super::model::{IpQualitySource, IpType};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ip-quality-cache.json");
        let cache: QualityCache = Arc::new(Mutex::new(HashMap::new()));
        let fresh = IpQuality {
            ip: "198.51.100.10".into(),
            source: IpQualitySource::IpLocate,
            ip_type: IpType::Residential,
            organization: Some("Example ISP".into()),
            asn: Some(64500),
            is_proxy: false,
            is_vpn: false,
            is_tor: false,
            is_abuser: false,
        };
        cache_put_quality(&cache, &path, fresh.clone());
        assert!(path.is_file());
        #[cfg(unix)]
        assert_eq!(storage::mode(&path).unwrap(), 0o600);

        // Inject an expired entry directly on disk and verify load drops it while preserving fresh.
        let expired = PersistedQualityEntry {
            cached_at_secs: now_epoch_secs().saturating_sub(QUALITY_CACHE_TTL_SECS + 60),
            quality: IpQuality {
                ip: "203.0.113.99".into(),
                ..fresh.clone()
            },
        };
        let mut disk_entries = vec![
            PersistedQualityEntry {
                cached_at_secs: now_epoch_secs(),
                quality: fresh.clone(),
            },
            expired,
        ];
        fs::write(&path, serde_json::to_vec(&disk_entries).unwrap()).unwrap();
        let reloaded = load_quality_cache_disk(&path);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded["198.51.100.10"].quality, fresh);
        disk_entries.clear();
    }

    #[test]
    fn route_signature_detects_urltest_and_provider_drift() {
        use super::super::model::{ProxyGroupItem, ProxyGroupKind, ProxyProviderItem};
        let mut snap = ClashSnapshot::default();
        snap.summary.mode = "rule".into();
        snap.groups.push(ProxyGroupItem {
            name: "Auto".into(),
            kind: ProxyGroupKind::UrlTest,
            now: "JP-01".into(),
            resolved_chain: vec!["JP-01".into()],
            fixed: None,
            hidden: false,
            members: vec![],
        });
        snap.proxy_providers.push(ProxyProviderItem {
            name: "SubA".into(),
            vehicle_type: "HTTP".into(),
            node_count: 5,
            updated_at: "2026-11-26T00:00:00Z".into(),
            userinfo: None,
        });
        let sig1 = route_signature(&snap);
        snap.groups[0].now = "SG-01".into();
        let sig2 = route_signature(&snap);
        assert_ne!(sig1, sig2);
        snap.proxy_providers[0].updated_at = "2026-11-27T00:00:00Z".into();
        let sig3 = route_signature(&snap);
        assert_ne!(sig2, sig3);
    }
}
