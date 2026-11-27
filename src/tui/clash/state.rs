//! State machine for the dedicated Mihomo visual management workspace.

use crate::{
    ops::clash::{
        controller::DEFAULT_DELAY_URL,
        model::{
            ClashAvailability, ClashSnapshot, ClashTab, ConnectionItem, ExitProbeResult,
            ProfileSummary, ProxyGroupItem, ProxyNodeItem, RuleItem, ServiceEntryState,
            ServiceTarget,
        },
        monitor::{ClashActionTag, ClashEvent},
    },
    tui::{input::InputBuffer, state::Tone},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

/// Visualization style for the real-time traffic waveform graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum TrafficGraphMode {
    /// Mirrored dual-wave spectrum: download rises upward, upload drops downward from center horizon.
    #[default]
    Mirror,
    /// Shared Y-axis dual-curve overlay for direct download vs upload magnitude comparison.
    Overlay,
    /// Combined throughput stacked area (download base + upload cap).
    Stacked,
}

impl TrafficGraphMode {
    pub fn next(self) -> Self {
        match self {
            Self::Mirror => Self::Overlay,
            Self::Overlay => Self::Stacked,
            Self::Stacked => Self::Mirror,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Mirror => "clash.traffic.mode_mirror",
            Self::Overlay => "clash.traffic.mode_overlay",
            Self::Stacked => "clash.traffic.mode_stacked",
        }
    }
}

/// Rolling time window displayed along the real-time traffic X-axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum TrafficWindow {
    #[default]
    Sec60,
    Min3,
    Min5,
}

impl TrafficWindow {
    pub fn next(self) -> Self {
        match self {
            Self::Sec60 => Self::Min3,
            Self::Min3 => Self::Min5,
            Self::Min5 => Self::Sec60,
        }
    }

    pub fn secs(self) -> usize {
        match self {
            Self::Sec60 => 60,
            Self::Min3 => 180,
            Self::Min5 => 300,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sec60 => "60s",
            Self::Min3 => "3m",
            Self::Min5 => "5m",
        }
    }

    pub fn mid_label(self) -> &'static str {
        match self {
            Self::Sec60 => "-30s",
            Self::Min3 => "-90s",
            Self::Min5 => "-2.5m",
        }
    }
}

/// Vertical Y-axis scaling mode for the real-time traffic waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum TrafficScale {
    #[default]
    Linear,
    Log,
}

impl TrafficScale {
    pub fn toggle(self) -> Self {
        match self {
            Self::Linear => Self::Log,
            Self::Log => Self::Linear,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Linear => "clash.traffic.scale_linear",
            Self::Log => "clash.traffic.scale_log",
        }
    }
}

/// Secondary distribution view cycled with `b` in the fullscreen Traffic Atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum TrafficBreakdownKind {
    #[default]
    Exit,
    Rule,
    Process,
    Flows,
}

impl TrafficBreakdownKind {
    pub fn next(self) -> Self {
        match self {
            Self::Exit => Self::Rule,
            Self::Rule => Self::Process,
            Self::Process => Self::Flows,
            Self::Flows => Self::Exit,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Exit => "clash.traffic.by_exit",
            Self::Rule => "clash.traffic.by_rule",
            Self::Process => "clash.traffic.by_process",
            Self::Flows => "clash.traffic.top_flows",
        }
    }
}

/// Sort criteria for proxy nodes inside the active policy group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum ProxySort {
    #[default]
    Default,
    Delay,
    Name,
    Protocol,
}

impl ProxySort {
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Delay,
            Self::Delay => Self::Name,
            Self::Name => Self::Protocol,
            Self::Protocol => Self::Default,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Default => "clash.sort.default",
            Self::Delay => "clash.sort.delay",
            Self::Name => "clash.sort.name",
            Self::Protocol => "clash.sort.protocol",
        }
    }
}

/// Scope filter for the Requests/Connections tab (matches FlClash Running/Closed/All switcher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum ConnectionScope {
    #[default]
    Running,
    Closed,
    All,
}

impl ConnectionScope {
    pub fn next(self) -> Self {
        match self {
            Self::Running => Self::Closed,
            Self::Closed => Self::All,
            Self::All => Self::Running,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Running => "clash.conn.scope_running",
            Self::Closed => "clash.conn.scope_closed",
            Self::All => "clash.conn.scope_all",
        }
    }
}

/// Sort criteria for connections in the Requests tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum ConnectionSort {
    #[default]
    Speed,
    Total,
    Host,
    Process,
}

impl ConnectionSort {
    pub fn next(self) -> Self {
        match self {
            Self::Speed => Self::Total,
            Self::Total => Self::Host,
            Self::Host => Self::Process,
            Self::Process => Self::Speed,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Speed => "clash.conn.sort_speed",
            Self::Total => "clash.conn.sort_total",
            Self::Host => "clash.conn.sort_host",
            Self::Process => "clash.conn.sort_process",
        }
    }
}

/// Destructive or high-impact local confirmation action inside the Mihomo workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) enum ClashConfirmAction {
    RemoveProfile(String),
    CloseAllConnections,
}

/// Workspace-local modal overlays (routed strictly via the 4-way overlay priority stack).
#[derive(Debug, Clone)]
pub(in crate::tui) enum ClashModal {
    DelaySettings {
        url: InputBuffer,
        timeout: InputBuffer,
        active_field: usize,
        error: Option<String>,
    },
    GroupPicker {
        cursor: usize,
    },
    Providers {
        show_rules: bool,
        cursor: usize,
    },
    OutboundIp {
        scroll: u16,
    },
    ServiceStatus {
        cursor: usize,
        manage: bool,
    },
    Confirm {
        title: String,
        message: String,
        danger: bool,
        accept: bool,
        action: ClashConfirmAction,
    },
}

/// Deduplicated management entries hosted in the `Tools` tab (pure query actions already
/// visualized in tabs 1-5 are omitted here while remaining available via CLI and Ctrl+K).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ToolEntry {
    ServiceStart,
    ServiceStop,
    ServiceReload,
    OutboundIp,
    ServiceStatus,
    SocksSecretCard,
    ViewLogs,
    CqsAction(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) struct ToolItem {
    pub section: &'static str,
    pub entry: ToolEntry,
}

impl ToolItem {
    pub const ALL: &'static [Self] = &[
        // Core lifecycle
        Self {
            section: "core",
            entry: ToolEntry::ServiceStart,
        },
        Self {
            section: "core",
            entry: ToolEntry::ServiceStop,
        },
        Self {
            section: "core",
            entry: ToolEntry::ServiceReload,
        },
        Self {
            section: "core",
            entry: ToolEntry::OutboundIp,
        },
        Self {
            section: "core",
            entry: ToolEntry::ServiceStatus,
        },
        Self {
            section: "core",
            entry: ToolEntry::CqsAction("clash.install"),
        },
        // SOCKS5 inbound & credential card
        Self {
            section: "socks",
            entry: ToolEntry::SocksSecretCard,
        },
        Self {
            section: "socks",
            entry: ToolEntry::CqsAction("clash.socks.configure"),
        },
        Self {
            section: "socks",
            entry: ToolEntry::CqsAction("clash.socks.auth.generate"),
        },
        Self {
            section: "socks",
            entry: ToolEntry::CqsAction("clash.socks.auth.set"),
        },
        // TUN / DNS / Core Settings / Mixin
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.tun.configure"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.settings.configure"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.dns.configure"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.dns.query"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.cache.flush"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.mixin.status"),
        },
        Self {
            section: "settings",
            entry: ToolEntry::CqsAction("clash.mixin.reset"),
        },
        // REST Controller & Secret
        Self {
            section: "controller",
            entry: ToolEntry::CqsAction("clash.controller.configure"),
        },
        Self {
            section: "controller",
            entry: ToolEntry::CqsAction("clash.secret.generate"),
        },
        Self {
            section: "controller",
            entry: ToolEntry::CqsAction("clash.secret.set"),
        },
        Self {
            section: "controller",
            entry: ToolEntry::CqsAction("clash.secret.clear"),
        },
        // GeoData / Panel / systemd / Redacted Logs
        Self {
            section: "service",
            entry: ToolEntry::ViewLogs,
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.geodata.update"),
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.panel.install"),
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.panel.remove"),
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.service.status"),
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.service.install"),
        },
        Self {
            section: "service",
            entry: ToolEntry::CqsAction("clash.service.remove"),
        },
    ];
}

pub(in crate::tui) struct ClashState {
    pub active_ever: bool,
    pub tab: ClashTab,
    pub fullscreen: bool,
    pub launched_standalone: bool,
    pub snapshot: ClashSnapshot,

    // Overview & Real-Time Traffic Atlas
    pub overview_group_cursor: usize,
    pub last_overview_group_cols: Cell<usize>,
    pub exit_probe: Option<ExitProbeResult>,
    pub exit_probing: bool,
    pub exit_probe_error: Option<String>,
    pub exit_probe_generation: u64,
    pub hide_ip: bool,
    pub max_outbound_modal_scroll: Cell<u16>,
    pub service_states: BTreeMap<ServiceTarget, ServiceEntryState>,
    pub service_order: Vec<String>,
    pub disabled_services: Vec<String>,
    pub service_generation: u64,
    pub userinfo_probing_profile: Option<String>,
    pub userinfo_checked_profiles: BTreeSet<String>,
    pub traffic_mode: TrafficGraphMode,
    pub traffic_window: TrafficWindow,
    pub traffic_scale: TrafficScale,
    pub traffic_fullscreen: bool,
    pub traffic_paused: Option<ClashSnapshot>,
    pub traffic_breakdown: TrafficBreakdownKind,

    // Proxies
    pub group_cursor: usize,
    pub node_cursor: usize,
    pub proxy_sort: ProxySort,
    pub proxy_sort_asc: bool,
    pub hide_timeout: bool,
    pub delay_url: String,
    pub delay_timeout_ms: u64,
    pub testing_groups: BTreeSet<String>,
    pub testing_group_progress: BTreeMap<String, (usize, usize)>,
    pub testing_nodes: BTreeSet<String>,
    pub last_grid_cols: Cell<usize>,

    // Profiles
    pub profile_cursor: usize,
    pub busy_profiles: BTreeSet<String>,
    pub updating_all_profiles: bool,

    // Connections
    pub conn_scope: ConnectionScope,
    pub conn_sort: ConnectionSort,
    pub conn_sort_asc: bool,
    pub conn_facet: Option<String>,
    pub conn_cursor: usize,
    pub conn_inspector_fullscreen: bool,
    pub conn_inspector_scroll: u16,
    pub max_conn_inspector_scroll: Cell<u16>,

    // Rules
    pub rule_cursor: usize,
    pub rule_facet: Option<String>,

    // Tools & embedded redacted logs
    pub tool_cursor: usize,
    pub tools_log_mode: bool,
    pub tools_log_fullscreen: bool,
    pub logs: Vec<String>,
    pub log_scroll: u16,
    pub max_log_scroll: Cell<u16>,
    pub log_loading: bool,
    pub log_error: Option<String>,

    // Shared inline search / filter
    pub filter: InputBuffer,
    pub filtering: bool,

    // Local modal & status feedback
    pub modal: Option<ClashModal>,
    pub busy_hint: Option<String>,
}

impl Default for ClashState {
    fn default() -> Self {
        Self {
            active_ever: false,
            tab: ClashTab::Overview,
            fullscreen: false,
            launched_standalone: false,
            snapshot: ClashSnapshot::default(),
            overview_group_cursor: 0,
            last_overview_group_cols: Cell::new(2),
            exit_probe: None,
            exit_probing: false,
            exit_probe_error: None,
            exit_probe_generation: 0,
            hide_ip: false,
            max_outbound_modal_scroll: Cell::new(0),
            service_states: BTreeMap::new(),
            service_order: Vec::new(),
            disabled_services: Vec::new(),
            service_generation: 0,
            userinfo_probing_profile: None,
            userinfo_checked_profiles: BTreeSet::new(),
            traffic_mode: TrafficGraphMode::Mirror,
            traffic_window: TrafficWindow::Sec60,
            traffic_scale: TrafficScale::Linear,
            traffic_fullscreen: false,
            traffic_paused: None,
            traffic_breakdown: TrafficBreakdownKind::Exit,
            group_cursor: 0,
            node_cursor: 0,
            proxy_sort: ProxySort::Default,
            proxy_sort_asc: true,
            hide_timeout: false,
            delay_url: DEFAULT_DELAY_URL.to_owned(),
            delay_timeout_ms: 5000,
            testing_groups: BTreeSet::new(),
            testing_group_progress: BTreeMap::new(),
            testing_nodes: BTreeSet::new(),
            last_grid_cols: Cell::new(1),
            profile_cursor: 0,
            busy_profiles: BTreeSet::new(),
            updating_all_profiles: false,
            conn_scope: ConnectionScope::Running,
            conn_sort: ConnectionSort::Speed,
            conn_sort_asc: false,
            conn_facet: None,
            conn_cursor: 0,
            conn_inspector_fullscreen: false,
            conn_inspector_scroll: 0,
            max_conn_inspector_scroll: Cell::new(0),
            rule_cursor: 0,
            rule_facet: None,
            tool_cursor: 0,
            tools_log_mode: false,
            tools_log_fullscreen: false,
            logs: Vec::new(),
            log_scroll: 0,
            max_log_scroll: Cell::new(0),
            log_loading: false,
            log_error: None,
            filter: InputBuffer::default(),
            filtering: false,
            modal: None,
            busy_hint: None,
        }
    }
}

impl ClashState {
    pub fn set_tab(&mut self, tab: ClashTab) {
        if self.tab != tab {
            self.tab = tab;
            self.filtering = false;
            self.filter.clear();
            self.traffic_fullscreen = false;
            self.conn_inspector_fullscreen = false;
            self.tools_log_fullscreen = false;
            if tab == ClashTab::Proxies && !self.locate_active_node() {
                self.node_cursor = 0;
            }
            self.clamp_cursors();
        }
    }

    pub fn traffic_snapshot(&self) -> &ClashSnapshot {
        self.traffic_paused.as_ref().unwrap_or(&self.snapshot)
    }

    pub fn is_traffic_paused(&self) -> bool {
        self.traffic_paused.is_some()
    }

    pub fn toggle_traffic_pause(&mut self) {
        if self.traffic_paused.is_some() {
            self.traffic_paused = None;
        } else {
            self.traffic_paused = Some(self.snapshot.clone());
        }
    }

    pub fn displayed_ip<'a>(&self, ip: &'a str) -> &'a str {
        if self.hide_ip { "*****" } else { ip }
    }

    pub fn ordered_services(&self) -> Vec<ServiceTarget> {
        ServiceTarget::ordered(&self.service_order)
    }

    pub fn enabled_services(&self) -> Vec<ServiceTarget> {
        ServiceTarget::enabled(&self.service_order, &self.disabled_services)
    }

    pub fn pinned_service_targets(&self, max_count: usize) -> Vec<ServiceTarget> {
        self.enabled_services()
            .into_iter()
            .take(max_count.max(1))
            .collect()
    }

    pub fn service_entry(&self, target: ServiceTarget) -> Option<&ServiceEntryState> {
        self.service_states.get(&target)
    }

    pub fn any_service_probing(&self) -> bool {
        self.service_states.values().any(|s| s.probing)
    }

    #[cfg(test)]
    pub fn is_service_enabled(&self, target: ServiceTarget) -> bool {
        self.enabled_services().contains(&target)
    }

    /// Toggle a service target in Manage view, refusing to disable the final remaining service.
    pub fn toggle_service_enabled(&mut self, target: ServiceTarget) -> bool {
        let enabled = self.enabled_services();
        let currently_on = enabled.contains(&target);
        if currently_on {
            if enabled.len() <= 1 {
                return false;
            }
            if !self.disabled_services.iter().any(|id| id == target.id()) {
                self.disabled_services.push(target.id().to_owned());
            }
        } else {
            self.disabled_services.retain(|id| id != target.id());
        }
        true
    }

    /// Reorder a service target up (`delta = -1`) or down (`delta = 1`) in `service_order`.
    pub fn move_service_order(&mut self, index: usize, up: bool) -> Option<usize> {
        let mut ordered = self.ordered_services();
        let target_idx = if up {
            index.checked_sub(1)?
        } else {
            let next = index + 1;
            if next >= ordered.len() {
                return None;
            }
            next
        };
        ordered.swap(index, target_idx);
        self.service_order = ordered.into_iter().map(|t| t.id().to_owned()).collect();
        Some(target_idx)
    }

    pub fn reset_service_prefs(&mut self) {
        self.service_order.clear();
        self.disabled_services.clear();
    }

    pub fn is_animating(&self) -> bool {
        matches!(self.snapshot.availability, ClashAvailability::Loading)
            || self.exit_probing
            || self
                .exit_probe
                .as_ref()
                .is_some_and(ExitProbeResult::is_quality_loading)
            || self.any_service_probing()
            || self.userinfo_probing_profile.is_some()
            || !self.testing_groups.is_empty()
            || !self.testing_nodes.is_empty()
            || !self.busy_profiles.is_empty()
            || self.updating_all_profiles
            || self.log_loading
            || self.busy_hint.is_some()
    }

    /// Visible policy groups ordered for the active mode: in `global` mode `GLOBAL` leads the strip;
    /// otherwise non-hidden groups appear in config order with `GLOBAL` at the end.
    pub fn visible_groups(&self) -> Vec<&ProxyGroupItem> {
        let mut groups = self.snapshot.visible_groups();
        if self.snapshot.summary.mode.eq_ignore_ascii_case("global")
            && let Some(pos) = groups.iter().position(|g| g.name == "GLOBAL")
        {
            let global = groups.remove(pos);
            groups.insert(0, global);
        }
        groups
    }

    pub fn current_group(&self) -> Option<&ProxyGroupItem> {
        let groups = self.visible_groups();
        groups
            .get(self.group_cursor)
            .copied()
            .or_else(|| groups.first().copied())
    }

    pub fn select_group_index(&mut self, idx: usize) {
        let len = self.visible_groups().len();
        if len == 0 {
            self.group_cursor = 0;
            self.node_cursor = 0;
            return;
        }
        self.group_cursor = idx.min(len - 1);
        if !self.locate_active_node() {
            self.node_cursor = 0;
        }
    }

    pub fn cycle_group(&mut self, forward: bool) {
        let len = self.visible_groups().len();
        if len == 0 {
            return;
        }
        self.group_cursor = if forward {
            (self.group_cursor + 1) % len
        } else {
            (self.group_cursor + len - 1) % len
        };
        if !self.locate_active_node() {
            self.node_cursor = 0;
        }
    }

    /// Optimistically update `group.now`, `group.fixed`, and derived resolved exit chains so
    /// switching nodes renders immediately before Mihomo's next controller snapshot arrives.
    pub fn apply_optimistic_select(&mut self, group_name: &str, node_name: &str) {
        let mut updated = false;
        for g in &mut self.snapshot.groups {
            if g.name == group_name {
                g.now = node_name.to_owned();
                if g.kind.supports_unfix() {
                    g.fixed = Some(node_name.to_owned());
                }
                updated = true;
            }
            for m in &mut g.members {
                if m.is_group && m.name == group_name {
                    m.now = Some(node_name.to_owned());
                }
            }
        }
        if updated {
            self.recompute_resolved_chains();
        }
    }

    /// Optimistically clear a pinned selection on an automatic (`URLTest`/`Fallback`) group.
    pub fn apply_optimistic_unfix(&mut self, group_name: &str) {
        for g in &mut self.snapshot.groups {
            if g.name == group_name && g.kind.supports_unfix() {
                g.fixed = None;
            }
        }
    }

    fn recompute_resolved_chains(&mut self) {
        let now_map: BTreeMap<String, String> = self
            .snapshot
            .groups
            .iter()
            .map(|g| (g.name.clone(), g.now.clone()))
            .collect();
        let max_hops = now_map.len().saturating_add(2);
        for g in &mut self.snapshot.groups {
            let mut chain = Vec::new();
            let mut cur = g.name.as_str();
            for _ in 0..max_hops {
                let Some(next) = now_map.get(cur).filter(|s| !s.is_empty()) else {
                    break;
                };
                if chain.iter().any(|seen: &String| seen == next) {
                    break;
                }
                chain.push(next.clone());
                cur = next.as_str();
            }
            g.resolved_chain = chain;
        }
    }

    /// Best known latency (ms) for a policy group's currently resolved exit node.
    pub fn group_resolved_delay(&self, group: &ProxyGroupItem) -> Option<u64> {
        let target = group
            .resolved_chain
            .last()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(group.now.as_str());
        if target.is_empty() {
            return None;
        }
        if let Some(ms) = group
            .members
            .iter()
            .find(|m| m.name == target)
            .and_then(|m| m.delay_ms)
        {
            return Some(ms);
        }
        self.snapshot
            .groups
            .iter()
            .flat_map(|g| g.members.iter())
            .find(|m| m.name == target && m.delay_ms.is_some())
            .and_then(|m| m.delay_ms)
    }

    /// Filtered and sorted nodes in the currently selected policy group.
    pub fn filtered_nodes(&self) -> Vec<&ProxyNodeItem> {
        let Some(group) = self.current_group() else {
            return Vec::new();
        };
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&ProxyNodeItem> = group
            .members
            .iter()
            .filter(|n| {
                if self.hide_timeout && n.tested && n.delay_ms.is_none() && n.name != group.now {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                n.name.to_ascii_lowercase().contains(&q) || n.kind.to_ascii_lowercase().contains(&q)
            })
            .collect();

        let asc = self.proxy_sort_asc;
        items.sort_by(|a, b| {
            let ord = match self.proxy_sort {
                ProxySort::Default => a.original_index.cmp(&b.original_index),
                ProxySort::Delay => match (a.delay_ms, b.delay_ms) {
                    (Some(da), Some(db)) => da.cmp(&db).then_with(|| a.name.cmp(&b.name)),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.original_index.cmp(&b.original_index),
                },
                ProxySort::Name => a
                    .name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase()),
                ProxySort::Protocol => a
                    .kind
                    .to_ascii_lowercase()
                    .cmp(&b.kind.to_ascii_lowercase())
                    .then_with(|| {
                        a.name
                            .to_ascii_lowercase()
                            .cmp(&b.name.to_ascii_lowercase())
                    }),
            };
            // Always keep unreachable/untested nodes at the bottom in Delay mode even when flipped.
            if self.proxy_sort == ProxySort::Delay {
                match (a.delay_ms, b.delay_ms) {
                    (Some(_), None) => return std::cmp::Ordering::Less,
                    (None, Some(_)) => return std::cmp::Ordering::Greater,
                    _ => {}
                }
            }
            if asc { ord } else { ord.reverse() }
        });
        items
    }

    pub fn selected_node(&self) -> Option<&ProxyNodeItem> {
        let nodes = self.filtered_nodes();
        nodes.get(self.node_cursor).copied()
    }

    /// Move `node_cursor` directly onto the group's currently active `now` node (FlClash locate).
    pub fn locate_active_node(&mut self) -> bool {
        let Some(now_name) = self.current_group().map(|g| g.now.clone()) else {
            return false;
        };
        if now_name.is_empty() {
            return false;
        }
        let nodes = self.filtered_nodes();
        if let Some(pos) = nodes.iter().position(|n| n.name == now_name) {
            self.node_cursor = pos;
            true
        } else {
            false
        }
    }

    /// Return the fastest reachable node in the current policy group for explicit quick-pick.
    pub fn fastest_node_in_current_group(&self) -> Option<(String, String, u64)> {
        let group = self.current_group()?;
        if !group.kind.supports_select() {
            return None;
        }
        group
            .members
            .iter()
            .filter_map(|m| m.delay_ms.map(|ms| (m.name.clone(), ms)))
            .min_by_key(|(_, ms)| *ms)
            .map(|(node, ms)| (group.name.clone(), node, ms))
    }

    /// Profiles filtered by the inline query.
    pub fn filtered_profiles(&self) -> Vec<&ProfileSummary> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        self.snapshot
            .profiles
            .iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_ascii_lowercase().contains(&q)
                    || p.host.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn selected_profile(&self) -> Option<&ProfileSummary> {
        let list = self.filtered_profiles();
        list.get(self.profile_cursor).copied()
    }

    /// Dynamic facet chips for Connections tab (FlClash style: TCP, UDP, Domain, DomainSuffix, ...).
    pub fn available_conn_facets(&self) -> Vec<String> {
        let mut nets = BTreeSet::new();
        let mut rules = BTreeSet::new();
        let iter: Box<dyn Iterator<Item = &ConnectionItem> + '_> = match self.conn_scope {
            ConnectionScope::Running => Box::new(self.snapshot.connections.iter()),
            ConnectionScope::Closed => Box::new(self.snapshot.closed_connections.iter()),
            ConnectionScope::All => Box::new(
                self.snapshot
                    .connections
                    .iter()
                    .chain(self.snapshot.closed_connections.iter()),
            ),
        };
        for c in iter {
            if !c.network.is_empty() {
                nets.insert(c.network.clone());
            }
            if !c.rule.is_empty() {
                rules.insert(c.rule.clone());
            }
        }
        let mut out: Vec<String> = nets.into_iter().collect();
        out.extend(rules);
        out
    }

    pub fn filtered_connections(&self) -> Vec<&ConnectionItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let facet = self.conn_facet.as_deref();
        let mut items: Vec<&ConnectionItem> = match self.conn_scope {
            ConnectionScope::Running => self.snapshot.connections.iter().collect(),
            ConnectionScope::Closed => self.snapshot.closed_connections.iter().collect(),
            ConnectionScope::All => self
                .snapshot
                .connections
                .iter()
                .chain(self.snapshot.closed_connections.iter())
                .collect(),
        };

        items.retain(|c| {
            if let Some(f) = facet
                && !c.network.eq_ignore_ascii_case(f)
                && !c.rule.eq_ignore_ascii_case(f)
            {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            c.host.to_ascii_lowercase().contains(&q)
                || c.destination.to_ascii_lowercase().contains(&q)
                || c.process.to_ascii_lowercase().contains(&q)
                || c.exit_node.to_ascii_lowercase().contains(&q)
                || c.rule.to_ascii_lowercase().contains(&q)
                || c.rule_payload.to_ascii_lowercase().contains(&q)
        });

        let asc = self.conn_sort_asc;
        items.sort_by(|a, b| {
            let ord = match self.conn_sort {
                ConnectionSort::Speed => a
                    .down_speed
                    .saturating_add(a.up_speed)
                    .cmp(&b.down_speed.saturating_add(b.up_speed))
                    .then_with(|| a.total_bytes().cmp(&b.total_bytes())),
                ConnectionSort::Total => a.total_bytes().cmp(&b.total_bytes()),
                ConnectionSort::Host => a
                    .display_target()
                    .to_ascii_lowercase()
                    .cmp(&b.display_target().to_ascii_lowercase()),
                ConnectionSort::Process => a
                    .display_process()
                    .to_ascii_lowercase()
                    .cmp(&b.display_process().to_ascii_lowercase()),
            };
            if asc { ord } else { ord.reverse() }
        });
        items
    }

    pub fn selected_connection(&self) -> Option<&ConnectionItem> {
        let list = self.filtered_connections();
        list.get(self.conn_cursor).copied()
    }

    /// Distinct rule types present in `/rules` for quick facet filtering.
    pub fn available_rule_facets(&self) -> Vec<String> {
        let mut kinds = BTreeSet::new();
        for r in &self.snapshot.rules {
            if !r.kind.is_empty() {
                kinds.insert(r.kind.clone());
            }
        }
        kinds.into_iter().collect()
    }

    pub fn filtered_rules(&self) -> Vec<&RuleItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let facet = self.rule_facet.as_deref();
        self.snapshot
            .rules
            .iter()
            .filter(|r| {
                if let Some(f) = facet
                    && !r.kind.eq_ignore_ascii_case(f)
                {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                r.kind.to_ascii_lowercase().contains(&q)
                    || r.payload.to_ascii_lowercase().contains(&q)
                    || r.proxy.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn filtered_tools(&self, i18n: &crate::i18n::I18n) -> Vec<ToolItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        ToolItem::ALL
            .iter()
            .copied()
            .filter(|item| {
                if q.is_empty() {
                    return true;
                }
                let label = tool_entry_title(item.entry, i18n).to_ascii_lowercase();
                let id_match = match item.entry {
                    ToolEntry::CqsAction(id) => id.to_ascii_lowercase().contains(&q),
                    _ => false,
                };
                label.contains(&q) || id_match || item.section.contains(&q)
            })
            .collect()
    }

    pub fn filtered_logs(&self) -> Vec<&str> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        self.logs
            .iter()
            .map(String::as_str)
            .filter(|line| q.is_empty() || line.to_ascii_lowercase().contains(&q))
            .collect()
    }

    pub fn clamp_cursors(&mut self) {
        let g_len = self.visible_groups().len();
        self.group_cursor = if g_len == 0 {
            0
        } else {
            self.group_cursor.min(g_len - 1)
        };
        self.overview_group_cursor = if g_len == 0 {
            0
        } else {
            self.overview_group_cursor.min(g_len - 1)
        };
        let n_len = self.filtered_nodes().len();
        self.node_cursor = if n_len == 0 {
            0
        } else {
            self.node_cursor.min(n_len - 1)
        };
        let p_len = self.filtered_profiles().len();
        self.profile_cursor = if p_len == 0 {
            0
        } else {
            self.profile_cursor.min(p_len - 1)
        };
        let c_len = self.filtered_connections().len();
        self.conn_cursor = if c_len == 0 {
            0
        } else {
            self.conn_cursor.min(c_len - 1)
        };
        let r_len = self.filtered_rules().len();
        self.rule_cursor = if r_len == 0 {
            0
        } else {
            self.rule_cursor.min(r_len - 1)
        };
    }

    /// Apply a background `ClashMonitor` event and optionally produce a `(message, Tone)` toast.
    pub fn apply_event(
        &mut self,
        evt: ClashEvent,
        i18n: &crate::i18n::I18n,
    ) -> Option<(String, Tone)> {
        match evt {
            ClashEvent::Snapshot(snap) => {
                // Preserve currently selected group by name across refreshes.
                let selected_group_name = self.current_group().map(|g| g.name.clone());
                let was_ready = self.snapshot.availability.is_ready();
                self.snapshot = *snap;
                if was_ready && !self.snapshot.availability.is_ready() {
                    self.exit_probe = None;
                    self.exit_probing = false;
                    self.exit_probe_error = None;
                    self.service_states.clear();
                }
                if let Some(name) = selected_group_name {
                    let groups = self.visible_groups();
                    if let Some(pos) = groups.iter().position(|g| g.name == name) {
                        self.group_cursor = pos;
                    }
                }
                self.clamp_cursors();
                None
            }
            ClashEvent::NodeDelayTick {
                group,
                node,
                delay_ms,
                completed,
                total,
            } => {
                if total > 0 {
                    self.testing_group_progress
                        .insert(group, (completed, total));
                }
                for g in &mut self.snapshot.groups {
                    for m in &mut g.members {
                        if m.name == node {
                            m.delay_ms = delay_ms;
                            m.tested = true;
                        }
                    }
                }
                None
            }
            ClashEvent::GroupDelayDone { group, result } => {
                self.testing_groups.remove(&group);
                self.testing_group_progress.remove(&group);
                match result {
                    Ok(delays) => {
                        let mut reachable = 0usize;
                        let mut total = 0usize;
                        for g in &mut self.snapshot.groups {
                            if g.name == group {
                                total = g.members.len();
                                for m in &mut g.members {
                                    if let Some(val) = delays.get(&m.name) {
                                        m.delay_ms = *val;
                                        m.tested = true;
                                        if val.is_some() {
                                            reachable += 1;
                                        }
                                    } else {
                                        m.delay_ms = None;
                                        m.tested = true;
                                    }
                                }
                            }
                        }
                        self.clamp_cursors();
                        Some((
                            i18n.format(
                                "clash.toast.delay_done",
                                &[
                                    ("group", &group),
                                    ("ok", &reachable.to_string()),
                                    ("total", &total.to_string()),
                                ],
                            ),
                            if reachable > 0 {
                                Tone::Success
                            } else {
                                Tone::Warning
                            },
                        ))
                    }
                    Err(err) => Some((
                        i18n.format("clash.toast.action_failed", &[("detail", &err.detail)]),
                        Tone::Danger,
                    )),
                }
            }
            ClashEvent::NodeDelayDone {
                group,
                node,
                result,
            } => {
                self.testing_nodes.remove(&node);
                match result {
                    Ok(delay) => {
                        for g in &mut self.snapshot.groups {
                            for m in &mut g.members {
                                if m.name == node {
                                    m.delay_ms = delay;
                                    m.tested = true;
                                }
                            }
                        }
                        let _ = group;
                        let msg = match delay {
                            Some(ms) => i18n.format(
                                "clash.toast.node_delay_ok",
                                &[("node", &node), ("ms", &ms.to_string())],
                            ),
                            None => {
                                i18n.format("clash.toast.node_delay_timeout", &[("node", &node)])
                            }
                        };
                        Some((
                            msg,
                            if delay.is_some() {
                                Tone::Success
                            } else {
                                Tone::Warning
                            },
                        ))
                    }
                    Err(err) => Some((
                        i18n.format("clash.toast.action_failed", &[("detail", &err.detail)]),
                        Tone::Danger,
                    )),
                }
            }
            ClashEvent::ActionDone { tag, result } => {
                self.busy_hint = None;
                match &tag {
                    ClashActionTag::UpdateProfile(name) => {
                        self.busy_profiles.remove(name);
                    }
                    ClashActionTag::UpdateAllProfiles => self.updating_all_profiles = false,
                    _ => {}
                }
                match result {
                    Ok(info) => {
                        let msg = match tag {
                            ClashActionTag::SelectProxy => {
                                i18n.format("clash.toast.selected", &[("detail", &info)])
                            }
                            ClashActionTag::UnfixGroup => {
                                i18n.format("clash.toast.unfixed", &[("group", &info)])
                            }
                            ClashActionTag::SetMode => {
                                i18n.format("clash.toast.mode_changed", &[("mode", &info)])
                            }
                            ClashActionTag::CloseConn => {
                                i18n.text("clash.toast.conn_closed").to_owned()
                            }
                            ClashActionTag::CloseAllConns => {
                                i18n.text("clash.toast.conns_closed").to_owned()
                            }
                            ClashActionTag::SwitchProfile => {
                                if let Some(name) = info.strip_prefix("reloaded:") {
                                    i18n.format("clash.toast.profile_reloaded", &[("name", name)])
                                } else {
                                    let name = info.strip_prefix("saved:").unwrap_or(&info);
                                    i18n.format("clash.toast.profile_saved", &[("name", name)])
                                }
                            }
                            ClashActionTag::UpdateProfile(_) => {
                                if let Some(name) = info.strip_prefix("reloaded:") {
                                    i18n.format(
                                        "clash.toast.profile_updated_reloaded",
                                        &[("name", name)],
                                    )
                                } else {
                                    let name = info.strip_prefix("updated:").unwrap_or(&info);
                                    i18n.format("clash.toast.profile_updated", &[("name", name)])
                                }
                            }
                            ClashActionTag::UpdateAllProfiles => {
                                let parts: Vec<&str> = info.split('|').collect();
                                let ok = parts.first().copied().unwrap_or("0");
                                let fail = parts.get(1).copied().unwrap_or("0");
                                i18n.format(
                                    "clash.toast.profiles_updated_all",
                                    &[("ok", ok), ("fail", fail)],
                                )
                            }
                            ClashActionTag::RemoveProfile => {
                                i18n.format("clash.toast.profile_removed", &[("name", &info)])
                            }
                            ClashActionTag::UpdateProvider => {
                                i18n.format("clash.toast.provider_updated", &[("name", &info)])
                            }
                            ClashActionTag::HealthcheckProvider => {
                                i18n.format("clash.toast.provider_checked", &[("name", &info)])
                            }
                            ClashActionTag::EnableLocalController => {
                                i18n.format("clash.toast.controller_enabled", &[("addr", &info)])
                            }
                        };
                        Some((msg, Tone::Success))
                    }
                    Err(err) => Some((
                        i18n.format("clash.toast.action_failed", &[("detail", &err.detail)]),
                        Tone::Danger,
                    )),
                }
            }
            ClashEvent::OutboundProbeStarted { generation, .. } => {
                if generation >= self.exit_probe_generation {
                    self.exit_probe_generation = generation;
                    self.exit_probing = true;
                    self.exit_probe_error = None;
                }
                None
            }
            ClashEvent::OutboundIpResolved {
                generation,
                result,
                manual,
            } => {
                if generation < self.exit_probe_generation {
                    return None;
                }
                self.exit_probe_generation = generation;
                self.exit_probing = false;
                match result {
                    Ok(info) => {
                        let ip = info.ip.clone();
                        self.exit_probe = Some(ExitProbeResult::from_ip_info(info));
                        self.exit_probe_error = None;
                        if manual {
                            let shown = self.displayed_ip(&ip).to_owned();
                            Some((
                                i18n.format("clash.toast.exit_probe_ok", &[("ip", &shown)]),
                                Tone::Success,
                            ))
                        } else {
                            None
                        }
                    }
                    Err(err) => {
                        self.exit_probe_error = Some(err.detail.clone());
                        if manual {
                            Some((
                                i18n.format(
                                    "clash.toast.action_failed",
                                    &[("detail", &err.detail)],
                                ),
                                Tone::Warning,
                            ))
                        } else {
                            None
                        }
                    }
                }
            }
            ClashEvent::OutboundQualityResolved {
                generation,
                ip,
                outcome,
            } => {
                if generation < self.exit_probe_generation {
                    return None;
                }
                if let Some(probe) = &mut self.exit_probe
                    && probe.ip == ip
                {
                    probe.quality = outcome;
                }
                None
            }
            ClashEvent::OutboundLatencyResolved {
                generation,
                google_ms,
                github_ms,
            } => {
                if generation < self.exit_probe_generation {
                    return None;
                }
                if let Some(probe) = &mut self.exit_probe {
                    probe.google_ms = google_ms;
                    probe.github_ms = github_ms;
                }
                None
            }
            ClashEvent::ServiceCheckStarted {
                generation,
                targets,
            } => {
                if generation < self.service_generation {
                    return None;
                }
                if generation > self.service_generation {
                    for entry in self.service_states.values_mut() {
                        entry.probing = false;
                    }
                    self.service_generation = generation;
                }
                for target in targets {
                    self.service_states.entry(target).or_default().probing = true;
                }
                None
            }
            ClashEvent::ServiceCheckItemDone { generation, item } => {
                if generation < self.service_generation {
                    return None;
                }
                self.service_generation = generation;
                if let Some(probe) = &mut self.exit_probe {
                    match item.target {
                        ServiceTarget::Google => {
                            probe.google_ms = item
                                .status
                                .is_available()
                                .then_some(item.delay_ms)
                                .flatten();
                        }
                        ServiceTarget::GitHub => {
                            probe.github_ms = item
                                .status
                                .is_available()
                                .then_some(item.delay_ms)
                                .flatten();
                        }
                        _ => {}
                    }
                }
                let entry = self.service_states.entry(item.target).or_default();
                entry.probing = false;
                entry.result = Some(item);
                None
            }
            ClashEvent::ServiceCheckFinished { generation, manual } => {
                if generation < self.service_generation {
                    return None;
                }
                for entry in self.service_states.values_mut() {
                    entry.probing = false;
                }
                if manual {
                    let enabled = self.enabled_services();
                    let checked: Vec<_> = enabled
                        .iter()
                        .filter_map(|t| self.service_states.get(t).and_then(|s| s.result.as_ref()))
                        .collect();
                    let ok = checked.iter().filter(|r| r.status.is_available()).count();
                    let total = checked.len();
                    if total > 0 {
                        return Some((
                            i18n.format(
                                "clash.toast.services_checked",
                                &[("ok", &ok.to_string()), ("total", &total.to_string())],
                            ),
                            if ok == total {
                                Tone::Success
                            } else {
                                Tone::Warning
                            },
                        ));
                    }
                }
                None
            }
            ClashEvent::ProfileUserinfoState {
                name,
                probing,
                found,
            } => {
                if probing {
                    self.userinfo_probing_profile = Some(name);
                } else {
                    if self.userinfo_probing_profile.as_deref() == Some(name.as_str()) {
                        self.userinfo_probing_profile = None;
                    }
                    if found.is_some() {
                        self.userinfo_checked_profiles.insert(name);
                    }
                }
                None
            }
            ClashEvent::LogsLoaded(result) => {
                self.log_loading = false;
                match result {
                    Ok(lines) => {
                        self.logs = lines;
                        self.log_error = None;
                        None
                    }
                    Err(err) => {
                        self.log_error = Some(err.detail);
                        None
                    }
                }
            }
            ClashEvent::LogStreamLine(line) => {
                self.log_loading = false;
                self.log_error = None;
                self.logs.push(line);
                if self.logs.len() > crate::ops::clash::model::MAX_CLASH_LOG_LINES {
                    let excess = self.logs.len() - crate::ops::clash::model::MAX_CLASH_LOG_LINES;
                    self.logs.drain(..excess);
                }
                None
            }
        }
    }

    pub fn tab_count(&self, tab: ClashTab) -> String {
        match tab {
            ClashTab::Overview => String::new(),
            ClashTab::Proxies => {
                let n = self.visible_groups().len();
                if n > 0 { n.to_string() } else { String::new() }
            }
            ClashTab::Profiles => {
                let n = self.snapshot.profiles.len();
                if n > 0 { n.to_string() } else { String::new() }
            }
            ClashTab::Connections => {
                let n = self.snapshot.connections.len();
                if n > 0 { n.to_string() } else { String::new() }
            }
            ClashTab::Rules => {
                let n = self.snapshot.rules.len();
                if n > 0 { n.to_string() } else { String::new() }
            }
            ClashTab::Tools => String::new(),
        }
    }
}

pub(in crate::tui) fn tool_entry_title(entry: ToolEntry, i18n: &crate::i18n::I18n) -> String {
    match entry {
        ToolEntry::ServiceStart => i18n.text("action.clash.start").to_owned(),
        ToolEntry::ServiceStop => i18n.text("action.clash.stop").to_owned(),
        ToolEntry::ServiceReload => i18n.text("action.clash.reload").to_owned(),
        ToolEntry::OutboundIp => i18n.text("clash.outbound.title").to_owned(),
        ToolEntry::ServiceStatus => i18n.text("clash.service.title").to_owned(),
        ToolEntry::SocksSecretCard => i18n.text("cmd.socks_secret").to_owned(),
        ToolEntry::ViewLogs => i18n.text("clash.tools.view_logs").to_owned(),
        ToolEntry::CqsAction(id) => i18n.text(&format!("action.{id}")).to_owned(),
    }
}
