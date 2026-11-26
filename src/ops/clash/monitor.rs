//! Lazy background Mihomo monitor (`ClashMonitor`) built on `std::thread` + bounded `mpsc`.
//!
//! Zero network or disk probes run while idle outside the Mihomo workspace; automatically sleeps
//! 10 seconds after leaving the Mihomo view.

use super::{
    controller,
    model::{
        ClashAvailability, ClashSnapshot, ClashTab, ConnectionItem, IpQuality, IpQualityOutcome,
        MAX_CLASH_LOG_LINES, MAX_CLOSED_CONNECTIONS, MAX_TRAFFIC_HISTORY, OutboundIpInfo,
        TrafficSample,
    },
    outbound_ip,
};
use crate::{error::Result, runtime::Context};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const FAST_POLL_INTERVAL: Duration = Duration::from_secs(1);
const FULL_POLL_INTERVAL: Duration = Duration::from_secs(3);
const SLOW_POLL_INTERVAL: Duration = Duration::from_secs(5);
const IDLE_SLEEP_AFTER: Duration = Duration::from_secs(10);
const ROUTE_SETTLE_DELAY: Duration = Duration::from_millis(600);
const QUALITY_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const QUALITY_CACHE_CAP: usize = 64;
const EVENT_QUEUE_CAP: usize = 48;

fn push_bounded<T>(tx: &SyncSender<T>, item: T) -> bool {
    !matches!(tx.try_send(item), Err(TrySendError::Disconnected(_)))
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
    ProbeExit,
    RouteChanged,
    FetchLogs,
    Shutdown,
}

#[derive(Debug)]
pub enum ClashEvent {
    Snapshot(Box<ClashSnapshot>),
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
    LogsLoaded(Result<Vec<String>>),
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

    fn ingest(&mut self, raw: controller::RawConnections, now: Instant, snap: &mut ClashSnapshot) {
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
        if record_sample {
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
        snap.up_speed = up_speed;
        snap.down_speed = down_speed;
        snap.peak_up_speed = self.peak_up;
        snap.peak_down_speed = self.peak_down;
        if raw.memory.is_some() {
            snap.memory_bytes = raw.memory;
        }
        snap.connections = live_list;
        snap.closed_connections = self.closed_ring.clone();
        snap.traffic_history = self.traffic_history.clone();
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
        let ctx = ctx.silent();
        let self_tx = cmd_tx.clone();

        let handle = thread::spawn(move || {
            let mut active = false;
            let mut left_at: Option<Instant> = None;
            let mut tab = ClashTab::Overview;
            let mut snap = ClashSnapshot::default();
            let mut tracker = ConnectionTracker::default();
            let mut last_fast = Instant::now() - FAST_POLL_INTERVAL;
            let mut last_full = Instant::now() - FULL_POLL_INTERVAL;
            let mut last_slow = Instant::now() - SLOW_POLL_INTERVAL;
            let mut version_cached = String::new();
            let mut cached_settings: Option<controller::Settings> = None;
            let mut uptime_anchor: Option<(u64, Instant)> = None;
            let mut probe_generation: u64 = 0;
            let mut active_probe_cancel: Option<Arc<AtomicBool>> = None;
            let quality_cache: QualityCache = Arc::new(Mutex::new(HashMap::new()));
            let mut auto_probed_ready = false;
            let mut pending_route_probe_at: Option<Instant> = None;

            loop {
                if ctx.cancel.load(Ordering::SeqCst) {
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
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
                    break;
                }
                let mut batch = Vec::new();
                if let Ok(cmd) = first {
                    batch.push(cmd);
                    while let Ok(more) = cmd_rx.try_recv() {
                        batch.push(more);
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
                                if t == ClashTab::Rules {
                                    force_rules = true;
                                }
                                if matches!(
                                    t,
                                    ClashTab::Proxies | ClashTab::Profiles | ClashTab::Rules
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
                                    ClashTab::Proxies | ClashTab::Profiles | ClashTab::Rules
                                ) && !snap.fetched_providers
                                {
                                    force_providers = true;
                                }
                            }
                        }
                        ClashMonitorCmd::ForceRefresh => {
                            active = true;
                            left_at = None;
                            force_full = true;
                            force_rules = tab == ClashTab::Rules;
                            force_providers = matches!(
                                tab,
                                ClashTab::Proxies | ClashTab::Profiles | ClashTab::Rules
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
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::SelectProxy,
                                true,
                                move |c| {
                                    controller::select_proxy_live(&c, &group, &node)?;
                                    Ok(format!("{group} -> {node}"))
                                },
                            );
                        }
                        ClashMonitorCmd::UnfixGroup { group } => {
                            spawn_action(
                                &ctx,
                                &evt_tx,
                                &self_tx,
                                ClashActionTag::UnfixGroup,
                                true,
                                move |c| {
                                    controller::unfix_group_live(&c, &group)?;
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
                            let c = ctx.clone();
                            let tx = evt_tx.clone();
                            let refresh = self_tx.clone();
                            thread::spawn(move || {
                                let result =
                                    controller::delay_group_map(&c, &group, &url, timeout_ms);
                                let _ =
                                    push_bounded(&tx, ClashEvent::GroupDelayDone { group, result });
                                let _ = refresh.send(ClashMonitorCmd::ForceRefresh);
                            });
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
                        ClashMonitorCmd::ProbeExit => {
                            pending_route_probe_at = None;
                            auto_probed_ready = true;
                            launch_outbound_probe(
                                &ctx,
                                &evt_tx,
                                &quality_cache,
                                &mut probe_generation,
                                &mut active_probe_cancel,
                                true,
                                true,
                            );
                        }
                        ClashMonitorCmd::RouteChanged => {
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
                    break;
                }

                if let Some(left) = left_at
                    && left.elapsed() >= IDLE_SLEEP_AFTER
                {
                    active = false;
                    left_at = None;
                }

                // Trigger debounced route-change outbound IP re-probe once Mihomo settles.
                if active
                    && snap.availability.is_ready()
                    && pending_route_probe_at.is_some_and(|at| Instant::now() >= at)
                {
                    pending_route_probe_at = None;
                    auto_probed_ready = true;
                    launch_outbound_probe(
                        &ctx,
                        &evt_tx,
                        &quality_cache,
                        &mut probe_generation,
                        &mut active_probe_cancel,
                        false,
                        false,
                    );
                }

                let now = Instant::now();
                // Keep 1 Hz real-time traffic sampling unbroken across all tabs while the Mihomo
                // workspace is active and ready; automatically stops once LeaveClash sleeps.
                let needs_fast = active
                    && snap.availability.is_ready()
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
                    continue;
                }

                let did_full = needs_full || refresh_once;
                let prev_ready = snap.availability.is_ready();

                poll_step(
                    &ctx,
                    tab,
                    did_full,
                    needs_fast || did_full,
                    force_rules,
                    force_providers,
                    &mut version_cached,
                    &mut cached_settings,
                    &mut uptime_anchor,
                    &mut tracker,
                    &mut snap,
                );

                if prev_ready && !snap.availability.is_ready() {
                    tracker.reset();
                    version_cached.clear();
                    cached_settings = None;
                    uptime_anchor = None;
                    auto_probed_ready = false;
                    pending_route_probe_at = None;
                    if let Some(cancel) = active_probe_cancel.take() {
                        cancel.store(true, Ordering::SeqCst);
                    }
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

                // Automatically probe outbound IP & quality once Mihomo becomes ready in the active workspace.
                if active && snap.availability.is_ready() && !auto_probed_ready {
                    auto_probed_ready = true;
                    launch_outbound_probe(
                        &ctx,
                        &evt_tx,
                        &quality_cache,
                        &mut probe_generation,
                        &mut active_probe_cancel,
                        false,
                        false,
                    );
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

type QualityCache = Arc<Mutex<HashMap<String, (IpQuality, Instant)>>>;

fn cache_get_quality(cache: &QualityCache, ip: &str) -> Option<IpQuality> {
    let guard = cache.lock().ok()?;
    let (quality, cached_at) = guard.get(ip)?;
    if cached_at.elapsed() < QUALITY_CACHE_TTL {
        Some(quality.clone())
    } else {
        None
    }
}

fn cache_put_quality(cache: &QualityCache, quality: IpQuality) {
    let Ok(mut guard) = cache.lock() else {
        return;
    };
    let now = Instant::now();
    guard.retain(|_, (_, at)| now.saturating_duration_since(*at) < QUALITY_CACHE_TTL);
    if guard.len() >= QUALITY_CACHE_CAP
        && let Some(oldest_key) = guard
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone())
    {
        guard.remove(&oldest_key);
    }
    guard.insert(quality.ip.clone(), (quality, now));
}

fn launch_outbound_probe(
    ctx: &Context,
    evt_tx: &SyncSender<ClashEvent>,
    quality_cache: &QualityCache,
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

        // Stage 3: Connectivity latency probe runs concurrently so it never blocks IP quality.
        {
            let c_lat = c.clone();
            let tr_lat = transport.clone();
            let cancel_lat = cancel.clone();
            let tx_lat = tx.clone();
            thread::spawn(move || {
                let (google_ms, github_ms) =
                    outbound_ip::probe_connectivity(&c_lat, &tr_lat, &cancel_lat);
                if !cancel_lat.load(Ordering::SeqCst) {
                    let _ = push_bounded(
                        &tx_lat,
                        ClashEvent::OutboundLatencyResolved {
                            generation,
                            google_ms,
                            github_ms,
                        },
                    );
                }
            });
        }

        // Stage 2: 4-wave hedged IP quality lookup (reusing TTL cache unless manually refreshed).
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
                cache_put_quality(&cache, quality.clone());
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
    force_rules: bool,
    force_providers: bool,
    version_cached: &mut String,
    cached_settings: &mut Option<controller::Settings>,
    uptime_anchor: &mut Option<(u64, Instant)>,
    tracker: &mut ConnectionTracker,
    snap: &mut ClashSnapshot,
) {
    let now = Instant::now();

    // Lightweight 1 Hz fast path: reuse cached controller settings instead of re-reading and
    // YAML-parsing potentially multi-megabyte `runtime.yaml` every second. If the controller
    // request fails, invalidate the cache and fall through to full local + controller diagnostics.
    if !full
        && !force_rules
        && !force_providers
        && fetch_conns
        && snap.availability.is_ready()
        && let Some(settings) = cached_settings.as_ref()
    {
        match controller::fetch_connections_raw(ctx, settings) {
            Ok(raw) => {
                tracker.ingest(raw, now, snap);
                if let Some((base_secs, anchor)) = *uptime_anchor {
                    snap.summary.uptime_secs = Some(
                        base_secs.saturating_add(now.saturating_duration_since(anchor).as_secs()),
                    );
                }
                snap.availability = ClashAvailability::Ready;
                snap.last_error = None;
                snap.updated_at = Some(now);
                return;
            }
            Err(_) => {
                *cached_settings = None;
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
            snap.availability = ClashAvailability::ControllerUnconfigured;
            snap.summary = local_summary;
            snap.updated_at = Some(now);
            return;
        }
        Err(err) => {
            *cached_settings = None;
            snap.availability = ClashAvailability::ControllerUnreachable(err.detail);
            snap.summary = local_summary;
            snap.updated_at = Some(now);
            return;
        }
    };

    let mut summary = local_summary;
    summary.controller_url = Some(settings.url.clone());
    summary.controller_has_secret = settings.has_secret();
    // Always seed controller-derived fields from the previous snapshot before HTTP calls so
    // neither 1-second fast ticks nor transient controller timeouts wipe version/mode/ports.
    preserve_controller_summary(&snap.summary, &mut summary, version_cached, !include_panel);

    // Fast path: live connections + traffic speed diffing across active Mihomo tabs.
    if fetch_conns {
        match controller::fetch_connections_raw(ctx, &settings) {
            Ok(raw) => {
                tracker.ingest(raw, now, snap);
                snap.availability = ClashAvailability::Ready;
                snap.last_error = None;
            }
            Err(err) => {
                *cached_settings = None;
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
            && let Ok(ver) = controller::fetch_version(ctx, &settings)
        {
            *version_cached = ver;
        }
        if !version_cached.is_empty() {
            summary.version = version_cached.clone();
        }
        match controller::fetch_configs(ctx, &settings) {
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
                snap.availability = ClashAvailability::ControllerUnreachable(err.detail.clone());
                snap.last_error = Some(err.detail);
                snap.summary = summary;
                snap.updated_at = Some(now);
                return;
            }
        }
        if matches!(tab, ClashTab::Overview | ClashTab::Proxies) || snap.groups.is_empty() {
            match controller::fetch_groups(ctx, &settings) {
                Ok(groups) => snap.groups = groups,
                Err(err) => snap.last_error = Some(err.detail),
            }
        }
    }

    if force_providers {
        if let Ok(pp) = controller::fetch_proxy_providers(ctx, &settings) {
            snap.proxy_providers = pp;
        }
        if let Ok(rp) = controller::fetch_rule_providers(ctx, &settings) {
            snap.rule_providers = rp;
        }
        snap.fetched_providers = true;
    }

    if force_rules {
        match controller::fetch_rules(ctx, &settings) {
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

        // Feed 250 short-lived connections across ticks to exercise both closed-connection ring
        // eviction (cap 200) and sparkline sample history eviction (cap 40).
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
        // Close all 250 connections on tick 1
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

        // Feed additional traffic ticks past MAX_TRAFFIC_HISTORY -> history stays strictly bounded.
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

        // Counter reset (e.g. Mihomo restart) re-anchors without recording a bogus dip/spike.
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

        // On a full tick where WebUI was uninstalled (preserve_panel=false), controller fields
        // remain seeded against transient HTTP failure while fresh panel absence is honored.
        let mut full_next = super::super::model::ClashRuntimeSummary {
            http_port: Some(7890),
            panel_installed: false,
            ..Default::default()
        };
        preserve_controller_summary(&prev, &mut full_next, "v1.19.34", false);
        assert_eq!(full_next.version, "v1.19.34");
        assert_eq!(full_next.mode, "rule");
        assert_eq!(full_next.mixed_port, Some(7890));
        assert!(!full_next.panel_installed);
        assert!(full_next.panel_name.is_none());
    }
}
