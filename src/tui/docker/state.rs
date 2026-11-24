//! State machine for the dedicated Docker management workspace.

use crate::{
    ops::docker::{
        actions::{DockerOp, ExecSpec},
        model::{
            ComposeProject, ContainerInspect, ContainerItem, DockerAvailability, DockerLogEntry,
            DockerSnapshot, DockerTab, ImageInspect, ImageItem, MAX_DOCKER_LOG_LINES, NetworkItem,
            VolumeItem,
        },
        monitor::{DockerEvent, LogStreamEvent, LogTarget},
    },
    tui::{input::InputBuffer, state::Tone},
};
use ratatui::layout::Rect;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

pub(in crate::tui) const INSPECT_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ContainerSubTab {
    Overview,
    Logs,
    Stats,
    Env,
}

impl ContainerSubTab {
    pub const ALL: [Self; 4] = [Self::Overview, Self::Logs, Self::Stats, Self::Env];

    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Logs => 1,
            Self::Stats => 2,
            Self::Env => 3,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        Self::ALL.get(idx).copied().unwrap_or(Self::Overview)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Overview => "docker.sub.overview",
            Self::Logs => "docker.sub.logs",
            Self::Stats => "docker.sub.stats",
            Self::Env => "docker.sub.env",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ComposeSubTab {
    Services,
    Logs,
    Yaml,
}

impl ComposeSubTab {
    pub const ALL: [Self; 3] = [Self::Services, Self::Logs, Self::Yaml];

    pub fn index(self) -> usize {
        match self {
            Self::Services => 0,
            Self::Logs => 1,
            Self::Yaml => 2,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        Self::ALL.get(idx).copied().unwrap_or(Self::Services)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Services => "docker.sub.services",
            Self::Logs => "docker.sub.logs",
            Self::Yaml => "docker.sub.yaml",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ImageSubTab {
    Overview,
    History,
}

impl ImageSubTab {
    pub const ALL: [Self; 2] = [Self::Overview, Self::History];

    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::History => 1,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        Self::ALL.get(idx).copied().unwrap_or(Self::Overview)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Overview => "docker.sub.overview",
            Self::History => "docker.sub.history",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ContainerSort {
    State,
    Name,
    Cpu,
    Memory,
    Created,
}

impl ContainerSort {
    pub fn next(self) -> Self {
        match self {
            Self::State => Self::Name,
            Self::Name => Self::Cpu,
            Self::Cpu => Self::Memory,
            Self::Memory => Self::Created,
            Self::Created => Self::State,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::State => "docker.sort.state",
            Self::Name => "docker.sort.name",
            Self::Cpu => "docker.sort.cpu",
            Self::Memory => "docker.sort.mem",
            Self::Created => "docker.sort.created",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ImageSort {
    Size,
    Name,
    Created,
}

impl ImageSort {
    pub fn next(self) -> Self {
        match self {
            Self::Size => Self::Name,
            Self::Name => Self::Created,
            Self::Created => Self::Size,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Size => "docker.sort.size",
            Self::Name => "docker.sort.name",
            Self::Created => "docker.sort.created",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum VolumeSort {
    Name,
    Size,
    Refs,
}

impl VolumeSort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Refs,
            Self::Refs => Self::Name,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Name => "docker.sort.name",
            Self::Size => "docker.sort.size",
            Self::Refs => "docker.col.refs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum NetworkSort {
    Name,
    Driver,
    Containers,
}

impl NetworkSort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Driver,
            Self::Driver => Self::Containers,
            Self::Containers => Self::Name,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Name => "docker.sort.name",
            Self::Driver => "docker.col.driver",
            Self::Containers => "docker.col.containers",
        }
    }
}

/// Interactive items listed on the Daemon tab (combining systemd service controls, Prune
/// wizard, and the 6 existing CqShell `docker.*` transactional actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum DaemonEntry {
    ServiceStart,
    ServiceStop,
    ServiceRestart,
    PruneWizard,
    CqsAction(&'static str),
}

impl DaemonEntry {
    pub const ALL: [Self; 10] = [
        Self::ServiceStart,
        Self::ServiceStop,
        Self::ServiceRestart,
        Self::PruneWizard,
        Self::CqsAction("docker.install"),
        Self::CqsAction("docker.status"),
        Self::CqsAction("docker.mirror"),
        Self::CqsAction("docker.ipv6"),
        Self::CqsAction("docker.clean"),
        Self::CqsAction("docker.uninstall"),
    ];
}

#[derive(Debug, Clone)]
pub(in crate::tui) enum MenuAction {
    Op(DockerOp),
    ExecQuick {
        container_id: String,
        container_name: String,
    },
    ExecCustom {
        container_id: String,
        container_name: String,
    },
    OpenLogs,
    JumpProject(String),
    Copy(String),
    OpenPrune,
    OpenPull,
    OpenCreateNetwork,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ActionMenuItem {
    pub label_key: &'static str,
    pub shortcut: &'static str,
    pub tone: Tone,
    pub disabled_toast: Option<&'static str>,
    pub action: MenuAction,
}

#[derive(Debug)]
pub(in crate::tui) enum DockerModal {
    ActionMenu {
        title: String,
        items: Vec<ActionMenuItem>,
        cursor: usize,
    },
    PullImage {
        reference: InputBuffer,
        error: Option<String>,
    },
    CreateNetwork {
        name: InputBuffer,
        subnet: InputBuffer,
        ipv6: bool,
        field: usize,
        error: Option<String>,
    },
    ExecForm {
        container_id: String,
        container_name: String,
        shell: InputBuffer,
        user: InputBuffer,
        workdir: InputBuffer,
        field: usize,
        error: Option<String>,
    },
    PruneWizard {
        containers: bool,
        images: bool,
        all_unused_images: bool,
        volumes: bool,
        networks: bool,
        build_cache: bool,
        cursor: usize,
    },
}

impl DockerModal {
    pub fn prune_default() -> Self {
        Self::PruneWizard {
            containers: true,
            images: true,
            all_unused_images: false,
            volumes: false,
            networks: true,
            build_cache: true,
            cursor: 0,
        }
    }

    pub fn pull_default() -> Self {
        Self::PullImage {
            reference: InputBuffer::new(String::new()),
            error: None,
        }
    }

    pub fn network_default() -> Self {
        Self::CreateNetwork {
            name: InputBuffer::new(String::new()),
            subnet: InputBuffer::new(String::new()),
            ipv6: false,
            field: 0,
            error: None,
        }
    }

    pub fn exec_default(container_id: String, container_name: String) -> Self {
        Self::ExecForm {
            container_id,
            container_name,
            shell: InputBuffer::new("auto".into()),
            user: InputBuffer::new(String::new()),
            workdir: InputBuffer::new(String::new()),
            field: 0,
            error: None,
        }
    }
}

/// Live streaming log buffer for the currently inspected container or Compose project.
#[derive(Debug, Clone)]
pub(in crate::tui) struct LogViewState {
    pub target: Option<LogTarget>,
    pub lines: VecDeque<DockerLogEntry>,
    pub scroll: usize,
    pub follow: bool,
    pub timestamps: bool,
    pub wrap: bool,
    pub search: InputBuffer,
    pub searching: bool,
    pub fullscreen: bool,
    pub ended: bool,
}

impl Default for LogViewState {
    fn default() -> Self {
        Self {
            target: None,
            lines: VecDeque::new(),
            scroll: 0,
            follow: true,
            timestamps: true,
            wrap: false,
            search: InputBuffer::new(String::new()),
            searching: false,
            fullscreen: false,
            ended: false,
        }
    }
}

impl LogViewState {
    pub fn reset_for(&mut self, target: Option<LogTarget>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.lines.clear();
        self.scroll = 0;
        self.follow = true;
        self.searching = false;
        self.ended = false;
    }

    pub fn push_batch(&mut self, entries: Vec<DockerLogEntry>) {
        let added = entries.len();
        let mut popped = 0usize;
        for entry in entries {
            if self.lines.len() >= MAX_DOCKER_LOG_LINES {
                self.lines.pop_front();
                popped += 1;
            }
            self.lines.push_back(entry);
        }
        if self.follow {
            self.scroll = 0;
        } else if self.scroll > 0 && added > popped {
            self.scroll = self.scroll.saturating_add(added - popped);
        }
    }

    pub fn max_scroll(&self, width: usize, visible_h: usize) -> usize {
        use unicode_width::UnicodeWidthStr;
        let entries = self.filtered_lines();
        if entries.is_empty() {
            return 0;
        }
        let w = width.max(8);
        let mut total = 0usize;
        for entry in entries {
            if self.wrap {
                let prefix_w = if self.timestamps {
                    entry.timestamp.as_deref().map_or(0, |t| t.width() + 2)
                } else {
                    0
                };
                let msg_w = w.saturating_sub(prefix_w).max(8);
                total += crate::tui::render::wrap_cells(&entry.message, msg_w).len();
            } else {
                total += 1;
            }
        }
        total.saturating_sub(visible_h)
    }

    pub fn filtered_lines(&self) -> Vec<&DockerLogEntry> {
        let q = self.search.text.trim().to_ascii_lowercase();
        if q.is_empty() {
            self.lines.iter().collect()
        } else {
            self.lines
                .iter()
                .filter(|entry| {
                    entry.message.to_ascii_lowercase().contains(&q)
                        || entry
                            .timestamp
                            .as_ref()
                            .is_some_and(|t| t.to_ascii_lowercase().contains(&q))
                })
                .collect()
        }
    }

    pub fn plain_export(&self) -> String {
        self.filtered_lines()
            .into_iter()
            .map(|e| match &e.timestamp {
                Some(ts) if self.timestamps => format!("{ts}  {}", e.message),
                _ => e.message.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(in crate::tui) struct DockerState {
    pub tab: DockerTab,
    pub fullscreen: bool,
    pub launched_standalone: bool,
    pub focus_inspector: bool,
    pub snapshot: DockerSnapshot,
    pub cursors: [usize; 6],
    pub selected_ids: [BTreeSet<String>; 6],
    pub filter: InputBuffer,
    pub filtering: bool,
    pub project_filter: Option<String>,
    pub only_running: bool,
    pub show_dangling: bool,
    pub container_sort: ContainerSort,
    pub container_sort_asc: bool,
    pub image_sort: ImageSort,
    pub image_sort_asc: bool,
    pub volume_sort: VolumeSort,
    pub volume_sort_asc: bool,
    pub network_sort: NetworkSort,
    pub network_sort_asc: bool,
    pub container_sub: ContainerSubTab,
    pub compose_sub: ComposeSubTab,
    pub image_sub: ImageSubTab,
    pub show_raw_json: bool,
    pub reveal_secrets: bool,
    pub inspector_scroll: u16,
    pub inspector_fullscreen: bool,
    pub logs: LogViewState,
    pub container_inspects: BTreeMap<String, ContainerInspect>,
    pub image_inspects: BTreeMap<String, ImageInspect>,
    pub compose_yamls: BTreeMap<String, String>,
    pub pending_inspect: Option<(Instant, String)>,
    pub last_selected_key: Option<String>,
    pub modal: Option<DockerModal>,
    pub active_ever: bool,
    pub last_cmd_rect: Cell<Option<Rect>>,
    pub max_inspector_scroll: Cell<u16>,
    pub last_logs_viewport: Cell<(usize, usize)>,
}

impl Default for DockerState {
    fn default() -> Self {
        Self {
            tab: DockerTab::Containers,
            fullscreen: false,
            launched_standalone: false,
            focus_inspector: false,
            snapshot: DockerSnapshot::default(),
            cursors: [0; 6],
            selected_ids: Default::default(),
            filter: InputBuffer::new(String::new()),
            filtering: false,
            project_filter: None,
            only_running: false,
            show_dangling: false,
            container_sort: ContainerSort::State,
            container_sort_asc: true,
            image_sort: ImageSort::Size,
            image_sort_asc: false,
            volume_sort: VolumeSort::Name,
            volume_sort_asc: true,
            network_sort: NetworkSort::Name,
            network_sort_asc: true,
            container_sub: ContainerSubTab::Overview,
            compose_sub: ComposeSubTab::Services,
            image_sub: ImageSubTab::Overview,
            show_raw_json: false,
            reveal_secrets: false,
            inspector_scroll: 0,
            inspector_fullscreen: false,
            logs: LogViewState::default(),
            container_inspects: BTreeMap::new(),
            image_inspects: BTreeMap::new(),
            compose_yamls: BTreeMap::new(),
            pending_inspect: None,
            last_selected_key: None,
            modal: None,
            active_ever: false,
            last_cmd_rect: Cell::new(None),
            max_inspector_scroll: Cell::new(0),
            last_logs_viewport: Cell::new((80, 16)),
        }
    }
}

impl DockerState {
    pub fn effective_logs_max_scroll(&self) -> usize {
        let (w, h) = self.last_logs_viewport.get();
        self.logs.max_scroll(w, h)
    }

    pub fn invalidate_inspects(&mut self) {
        self.container_inspects.clear();
        self.image_inspects.clear();
        self.compose_yamls.clear();
        self.pending_inspect = None;
        self.last_selected_key = None;
    }

    pub fn set_tab(&mut self, tab: DockerTab) {
        if self.tab != tab {
            self.tab = tab;
            self.inspector_scroll = 0;
            self.inspector_fullscreen = false;
            self.logs.fullscreen = false;
            self.logs.searching = false;
            self.show_raw_json = false;
            self.reveal_secrets = false;
            self.filtering = false;
            self.clamp_cursors();
        }
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let len = DockerTab::ALL.len();
        let next = if forward {
            (self.tab.index() + 1) % len
        } else {
            (self.tab.index() + len - 1) % len
        };
        self.set_tab(DockerTab::from_index(next));
    }

    pub fn filtered_containers(&self) -> Vec<&ContainerItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&ContainerItem> = self
            .snapshot
            .containers
            .iter()
            .filter(|c| !self.only_running || c.state.is_running())
            .filter(|c| {
                self.project_filter
                    .as_ref()
                    .is_none_or(|p| c.compose_project.as_deref() == Some(p.as_str()))
            })
            .filter(|c| {
                q.is_empty()
                    || c.name.to_ascii_lowercase().contains(&q)
                    || c.image.to_ascii_lowercase().contains(&q)
                    || c.status.to_ascii_lowercase().contains(&q)
                    || c.short_id.to_ascii_lowercase().contains(&q)
                    || c.compose_project
                        .as_deref()
                        .is_some_and(|p| p.to_ascii_lowercase().contains(&q))
            })
            .collect();

        items.sort_by(|a, b| {
            let ord = match self.container_sort {
                ContainerSort::State => a.state.cmp(&b.state).then_with(|| a.name.cmp(&b.name)),
                ContainerSort::Name => a.name.cmp(&b.name),
                ContainerSort::Cpu => {
                    let ca = self
                        .snapshot
                        .stats
                        .get(&a.id)
                        .map_or(-1.0, |s| s.cpu_percent);
                    let cb = self
                        .snapshot
                        .stats
                        .get(&b.id)
                        .map_or(-1.0, |s| s.cpu_percent);
                    cb.total_cmp(&ca).then_with(|| a.name.cmp(&b.name))
                }
                ContainerSort::Memory => {
                    let ma = self
                        .snapshot
                        .stats
                        .get(&a.id)
                        .map_or(0, |s| s.mem_used_bytes);
                    let mb = self
                        .snapshot
                        .stats
                        .get(&b.id)
                        .map_or(0, |s| s.mem_used_bytes);
                    mb.cmp(&ma).then_with(|| a.name.cmp(&b.name))
                }
                ContainerSort::Created => b.created_at.cmp(&a.created_at),
            };
            if self.container_sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
        items
    }

    pub fn filtered_compose(&self) -> Vec<&ComposeProject> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&ComposeProject> = self
            .snapshot
            .compose
            .iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_ascii_lowercase().contains(&q)
                    || p.status.to_ascii_lowercase().contains(&q)
                    || p.config_files
                        .iter()
                        .any(|f| f.to_ascii_lowercase().contains(&q))
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    pub fn filtered_images(&self) -> Vec<&ImageItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&ImageItem> = self
            .snapshot
            .images
            .iter()
            .filter(|img| self.show_dangling || !img.dangling)
            .filter(|img| {
                q.is_empty()
                    || img.repository.to_ascii_lowercase().contains(&q)
                    || img.tag.to_ascii_lowercase().contains(&q)
                    || img.short_id.to_ascii_lowercase().contains(&q)
            })
            .collect();
        items.sort_by(|a, b| {
            let ord = match self.image_sort {
                ImageSort::Size => a
                    .size_bytes
                    .cmp(&b.size_bytes)
                    .then_with(|| a.repository.cmp(&b.repository)),
                ImageSort::Name => a
                    .repository
                    .cmp(&b.repository)
                    .then_with(|| a.tag.cmp(&b.tag)),
                ImageSort::Created => a.created_at.cmp(&b.created_at),
            };
            if self.image_sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
        items
    }

    pub fn filtered_volumes(&self) -> Vec<&VolumeItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&VolumeItem> = self
            .snapshot
            .volumes
            .iter()
            .filter(|v| {
                q.is_empty()
                    || v.name.to_ascii_lowercase().contains(&q)
                    || v.driver.to_ascii_lowercase().contains(&q)
                    || v.mountpoint.to_ascii_lowercase().contains(&q)
            })
            .collect();
        items.sort_by(|a, b| {
            let ord = match self.volume_sort {
                VolumeSort::Name => a.name.cmp(&b.name),
                VolumeSort::Size => a
                    .size_bytes
                    .unwrap_or(0)
                    .cmp(&b.size_bytes.unwrap_or(0))
                    .then_with(|| a.name.cmp(&b.name)),
                VolumeSort::Refs => a
                    .used_by
                    .len()
                    .cmp(&b.used_by.len())
                    .then_with(|| a.name.cmp(&b.name)),
            };
            if self.volume_sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
        items
    }

    pub fn filtered_networks(&self) -> Vec<&NetworkItem> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        let mut items: Vec<&NetworkItem> = self
            .snapshot
            .networks
            .iter()
            .filter(|n| {
                q.is_empty()
                    || n.name.to_ascii_lowercase().contains(&q)
                    || n.driver.to_ascii_lowercase().contains(&q)
                    || n.subnet.to_ascii_lowercase().contains(&q)
            })
            .collect();
        items.sort_by(|a, b| {
            let ord = match self.network_sort {
                NetworkSort::Name => a.name.cmp(&b.name),
                NetworkSort::Driver => a.driver.cmp(&b.driver).then_with(|| a.name.cmp(&b.name)),
                NetworkSort::Containers => a
                    .containers
                    .len()
                    .cmp(&b.containers.len())
                    .then_with(|| a.name.cmp(&b.name)),
            };
            if self.network_sort_asc {
                ord
            } else {
                ord.reverse()
            }
        });
        items
    }

    pub fn visible_count(&self) -> usize {
        match self.tab {
            DockerTab::Containers => self.filtered_containers().len(),
            DockerTab::Compose => self.filtered_compose().len(),
            DockerTab::Images => self.filtered_images().len(),
            DockerTab::Volumes => self.filtered_volumes().len(),
            DockerTab::Networks => self.filtered_networks().len(),
            DockerTab::Daemon => DaemonEntry::ALL.len(),
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursors[self.tab.index()]
    }

    pub fn set_cursor(&mut self, pos: usize) {
        let count = self.visible_count();
        let idx = self.tab.index();
        let next = if count == 0 { 0 } else { pos.min(count - 1) };
        if self.cursors[idx] != next {
            self.cursors[idx] = next;
            self.inspector_scroll = 0;
            self.reveal_secrets = false;
        }
    }

    pub fn move_cursor(&mut self, down: bool) {
        let count = self.visible_count();
        if count == 0 {
            return;
        }
        let cur = self.cursor();
        let next = if down {
            (cur + 1) % count
        } else {
            (cur + count - 1) % count
        };
        self.set_cursor(next);
    }

    pub fn clamp_cursors(&mut self) {
        let len = self.visible_count();
        let idx = self.tab.index();
        if len == 0 {
            self.cursors[idx] = 0;
        } else if self.cursors[idx] >= len {
            self.cursors[idx] = len - 1;
        }
    }

    pub fn selected_container(&self) -> Option<&ContainerItem> {
        self.filtered_containers().get(self.cursors[0]).copied()
    }

    pub fn selected_compose(&self) -> Option<&ComposeProject> {
        self.filtered_compose().get(self.cursors[1]).copied()
    }

    pub fn selected_image(&self) -> Option<&ImageItem> {
        self.filtered_images().get(self.cursors[2]).copied()
    }

    pub fn selected_volume(&self) -> Option<&VolumeItem> {
        self.filtered_volumes().get(self.cursors[3]).copied()
    }

    pub fn selected_network(&self) -> Option<&NetworkItem> {
        self.filtered_networks().get(self.cursors[4]).copied()
    }

    pub fn selected_daemon_entry(&self) -> DaemonEntry {
        DaemonEntry::ALL
            .get(self.cursors[5])
            .copied()
            .unwrap_or(DaemonEntry::ServiceStart)
    }

    /// Unique key of the currently highlighted item for multi-select & inspect tracking.
    pub fn current_item_key(&self) -> Option<String> {
        match self.tab {
            DockerTab::Containers => self.selected_container().map(|c| c.id.clone()),
            DockerTab::Compose => self.selected_compose().map(|p| p.name.clone()),
            DockerTab::Images => self.selected_image().map(|i| i.id.clone()),
            DockerTab::Volumes => self.selected_volume().map(|v| v.name.clone()),
            DockerTab::Networks => self.selected_network().map(|n| n.id.clone()),
            DockerTab::Daemon => None,
        }
    }

    pub fn toggle_multi_select(&mut self) {
        if matches!(self.tab, DockerTab::Compose | DockerTab::Daemon) {
            return;
        }
        let Some(key) = self.current_item_key() else {
            return;
        };
        // Protect built-in networks and mounted volumes from accidental multi-select removal.
        if self.tab == DockerTab::Networks && self.selected_network().is_some_and(|n| n.builtin) {
            return;
        }
        if self.tab == DockerTab::Volumes
            && self
                .selected_volume()
                .is_some_and(|v| !v.used_by.is_empty())
        {
            return;
        }
        let set = &mut self.selected_ids[self.tab.index()];
        if !set.remove(&key) {
            set.insert(key);
        }
    }

    pub fn selected_count(&self) -> usize {
        self.selected_ids[self.tab.index()].len()
    }

    pub fn clear_selection(&mut self) {
        self.selected_ids[self.tab.index()].clear();
    }

    pub fn desired_log_target(&self) -> Option<LogTarget> {
        if !self.snapshot.availability.is_ready() {
            return None;
        }
        match self.tab {
            DockerTab::Containers
                if self.container_sub == ContainerSubTab::Logs || self.logs.fullscreen =>
            {
                self.selected_container()
                    .map(|c| LogTarget::Container { id: c.id.clone() })
            }
            DockerTab::Compose
                if self.compose_sub == ComposeSubTab::Logs || self.logs.fullscreen =>
            {
                self.selected_compose().map(|p| LogTarget::Compose {
                    project: p.name.clone(),
                    config_files: p.config_files.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn stats_needed(&self) -> bool {
        self.snapshot.availability.is_ready() && self.tab == DockerTab::Containers
    }

    pub fn cycle_sort(&mut self) {
        match self.tab {
            DockerTab::Containers => {
                self.container_sort = self.container_sort.next();
                self.container_sort_asc = true;
            }
            DockerTab::Images => {
                self.image_sort = self.image_sort.next();
                self.image_sort_asc = matches!(self.image_sort, ImageSort::Name);
            }
            DockerTab::Volumes => {
                self.volume_sort = self.volume_sort.next();
                self.volume_sort_asc = matches!(self.volume_sort, VolumeSort::Name);
            }
            DockerTab::Networks => {
                self.network_sort = self.network_sort.next();
                self.network_sort_asc = !matches!(self.network_sort, NetworkSort::Containers);
            }
            DockerTab::Compose | DockerTab::Daemon => {}
        }
        self.clamp_cursors();
    }

    pub fn sort_label_key(&self) -> Option<&'static str> {
        match self.tab {
            DockerTab::Containers => Some(self.container_sort.label_key()),
            DockerTab::Images => Some(self.image_sort.label_key()),
            DockerTab::Volumes => Some(self.volume_sort.label_key()),
            DockerTab::Networks => Some(self.network_sort.label_key()),
            DockerTab::Compose | DockerTab::Daemon => None,
        }
    }

    pub fn sort_ascending(&self) -> bool {
        match self.tab {
            DockerTab::Containers => self.container_sort_asc,
            DockerTab::Images => self.image_sort_asc,
            DockerTab::Volumes => self.volume_sort_asc,
            DockerTab::Networks => self.network_sort_asc,
            DockerTab::Compose | DockerTab::Daemon => true,
        }
    }

    /// Click a table header column index to sort (or toggle direction if already active).
    pub fn sort_by_column(&mut self, col_idx: usize) {
        match self.tab {
            DockerTab::Containers => {
                let target = match col_idx {
                    0 => ContainerSort::State,
                    1 => ContainerSort::Name,
                    2 => ContainerSort::Cpu,
                    3 => ContainerSort::Memory,
                    _ => ContainerSort::Created,
                };
                if self.container_sort == target {
                    self.container_sort_asc = !self.container_sort_asc;
                } else {
                    self.container_sort = target;
                    self.container_sort_asc = true;
                }
            }
            DockerTab::Images => {
                let target = match col_idx {
                    0 => ImageSort::Name,
                    1 => ImageSort::Size,
                    _ => ImageSort::Created,
                };
                if self.image_sort == target {
                    self.image_sort_asc = !self.image_sort_asc;
                } else {
                    self.image_sort = target;
                    self.image_sort_asc = matches!(target, ImageSort::Name);
                }
            }
            DockerTab::Volumes => {
                let target = match col_idx {
                    0 => VolumeSort::Name,
                    1 => VolumeSort::Size,
                    _ => VolumeSort::Refs,
                };
                if self.volume_sort == target {
                    self.volume_sort_asc = !self.volume_sort_asc;
                } else {
                    self.volume_sort = target;
                    self.volume_sort_asc = matches!(target, VolumeSort::Name);
                }
            }
            DockerTab::Networks => {
                let target = match col_idx {
                    0 => NetworkSort::Name,
                    1 => NetworkSort::Driver,
                    _ => NetworkSort::Containers,
                };
                if self.network_sort == target {
                    self.network_sort_asc = !self.network_sort_asc;
                } else {
                    self.network_sort = target;
                    self.network_sort_asc = !matches!(target, NetworkSort::Containers);
                }
            }
            DockerTab::Compose | DockerTab::Daemon => {}
        }
    }

    /// Equivalent read-only CLI command shown in the inspector strip (and clickable to copy).
    pub fn current_cli_hint(&self) -> Option<String> {
        match self.tab {
            DockerTab::Containers => self.selected_container().map(|c| match self.container_sub {
                ContainerSubTab::Logs => format!("docker logs -f --tail 200 {}", c.name),
                ContainerSubTab::Stats => format!("docker stats {}", c.name),
                _ => format!("docker inspect {}", c.name),
            }),
            DockerTab::Compose => self.selected_compose().map(|p| match self.compose_sub {
                ComposeSubTab::Logs => format!("docker compose -p {} logs -f", p.name),
                _ => format!("docker compose -p {} ps", p.name),
            }),
            DockerTab::Images => self.selected_image().map(|img| match self.image_sub {
                ImageSubTab::History => format!("docker image history {}", img.display_ref()),
                ImageSubTab::Overview => format!("docker image inspect {}", img.short_id),
            }),
            DockerTab::Volumes => self
                .selected_volume()
                .map(|v| format!("docker volume inspect {}", v.name)),
            DockerTab::Networks => self
                .selected_network()
                .map(|n| format!("docker network inspect {}", n.name)),
            DockerTab::Daemon => match self.selected_daemon_entry() {
                DaemonEntry::ServiceStart => Some("systemctl start docker".into()),
                DaemonEntry::ServiceStop => Some("systemctl stop docker".into()),
                DaemonEntry::ServiceRestart => Some("systemctl restart docker".into()),
                DaemonEntry::PruneWizard => Some("docker system df".into()),
                DaemonEntry::CqsAction(id) => Some(format!("cqs run {id}")),
            },
        }
    }

    /// Collects target `(ids, names)` for either the current multi-selection or the focused container.
    pub fn container_targets(&self) -> Option<(Vec<String>, Vec<String>)> {
        let sel = &self.selected_ids[0];
        if !sel.is_empty() {
            let mut ids = Vec::new();
            let mut names = Vec::new();
            for c in &self.snapshot.containers {
                if sel.contains(&c.id) {
                    ids.push(c.id.clone());
                    names.push(c.name.clone());
                }
            }
            if !ids.is_empty() {
                return Some((ids, names));
            }
        }
        self.selected_container()
            .map(|c| (vec![c.id.clone()], vec![c.name.clone()]))
    }

    /// Builds the contextual action menu (`Enter`) for the currently focused resource.
    pub fn build_action_menu(&self) -> Option<DockerModal> {
        match self.tab {
            DockerTab::Containers => {
                let (ids, names) = self.container_targets()?;
                let multi = ids.len() > 1;
                let first_name = names.first().cloned().unwrap_or_default();
                let title = if multi {
                    format!("{first_name} ({})", ids.len())
                } else {
                    first_name
                };
                let target_states: Vec<crate::ops::docker::model::ContainerState> = self
                    .snapshot
                    .containers
                    .iter()
                    .filter(|c| ids.contains(&c.id))
                    .map(|c| c.state)
                    .collect();
                let any_running = target_states.iter().any(|s| s.is_running());
                let all_paused = !target_states.is_empty()
                    && target_states
                        .iter()
                        .all(|s| *s == crate::ops::docker::model::ContainerState::Paused);
                let mut items = Vec::new();
                if any_running {
                    items.push(ActionMenuItem {
                        label_key: "docker.op.stop",
                        shortcut: "s",
                        tone: Tone::Warning,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ContainerStop {
                            ids: ids.clone(),
                            names: names.clone(),
                        }),
                    });
                } else {
                    items.push(ActionMenuItem {
                        label_key: "docker.op.start",
                        shortcut: "s",
                        tone: Tone::Success,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ContainerStart {
                            ids: ids.clone(),
                            names: names.clone(),
                        }),
                    });
                }
                items.push(ActionMenuItem {
                    label_key: "docker.op.restart",
                    shortcut: "u",
                    tone: Tone::Info,
                    disabled_toast: None,
                    action: MenuAction::Op(DockerOp::ContainerRestart {
                        ids: ids.clone(),
                        names: names.clone(),
                    }),
                });
                if all_paused {
                    items.push(ActionMenuItem {
                        label_key: "docker.op.unpause",
                        shortcut: "p",
                        tone: Tone::Success,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ContainerUnpause {
                            ids: ids.clone(),
                            names: names.clone(),
                        }),
                    });
                } else {
                    items.push(ActionMenuItem {
                        label_key: "docker.op.pause",
                        shortcut: "p",
                        tone: Tone::Muted,
                        disabled_toast: (!any_running).then_some("docker.toast.not_running"),
                        action: MenuAction::Op(DockerOp::ContainerPause {
                            ids: ids.clone(),
                            names: names.clone(),
                        }),
                    });
                }
                if !multi
                    && let Some(c) = self
                        .snapshot
                        .containers
                        .iter()
                        .find(|c| c.id == ids[0])
                        .or_else(|| self.selected_container())
                {
                    items.push(ActionMenuItem {
                        label_key: "docker.op.exec",
                        shortcut: "e",
                        tone: Tone::Accent,
                        disabled_toast: (!c.state.is_running())
                            .then_some("docker.toast.not_running"),
                        action: MenuAction::ExecQuick {
                            container_id: c.id.clone(),
                            container_name: c.name.clone(),
                        },
                    });
                    items.push(ActionMenuItem {
                        label_key: "docker.op.exec_custom",
                        shortcut: "E",
                        tone: Tone::Plain,
                        disabled_toast: (!c.state.is_running())
                            .then_some("docker.toast.not_running"),
                        action: MenuAction::ExecCustom {
                            container_id: c.id.clone(),
                            container_name: c.name.clone(),
                        },
                    });
                    items.push(ActionMenuItem {
                        label_key: "docker.op.logs",
                        shortcut: "l",
                        tone: Tone::Plain,
                        disabled_toast: None,
                        action: MenuAction::OpenLogs,
                    });
                }
                let copy_short_id = self
                    .snapshot
                    .containers
                    .iter()
                    .find(|c| c.id == ids[0])
                    .map(|c| c.short_id.clone())
                    .unwrap_or_else(|| ids[0].chars().take(12).collect());
                items.push(ActionMenuItem {
                    label_key: if any_running {
                        "docker.op.remove_container_force"
                    } else {
                        "docker.op.remove_container"
                    },
                    shortcut: "d",
                    tone: Tone::Danger,
                    disabled_toast: None,
                    action: MenuAction::Op(DockerOp::ContainerRemove {
                        ids,
                        names,
                        force: any_running,
                    }),
                });
                items.push(ActionMenuItem {
                    label_key: "docker.op.copy_id",
                    shortcut: "y",
                    tone: Tone::Muted,
                    disabled_toast: None,
                    action: MenuAction::Copy(copy_short_id),
                });
                Some(DockerModal::ActionMenu {
                    title,
                    items,
                    cursor: 0,
                })
            }
            DockerTab::Compose => {
                let p = self.selected_compose()?;
                let items = vec![
                    ActionMenuItem {
                        label_key: "docker.op.compose_jump",
                        shortcut: "j",
                        tone: Tone::Accent,
                        disabled_toast: None,
                        action: MenuAction::JumpProject(p.name.clone()),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.compose_up",
                        shortcut: "u",
                        tone: Tone::Success,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ComposeUp {
                            project: p.name.clone(),
                            config_files: p.config_files.clone(),
                        }),
                    },
                    ActionMenuItem {
                        label_key: if p.running_count > 0 {
                            "docker.op.compose_stop"
                        } else {
                            "docker.op.compose_start"
                        },
                        shortcut: "s",
                        tone: Tone::Warning,
                        disabled_toast: None,
                        action: MenuAction::Op(if p.running_count > 0 {
                            DockerOp::ComposeStop {
                                project: p.name.clone(),
                                config_files: p.config_files.clone(),
                            }
                        } else {
                            DockerOp::ComposeStart {
                                project: p.name.clone(),
                                config_files: p.config_files.clone(),
                            }
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.compose_restart",
                        shortcut: "r",
                        tone: Tone::Info,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ComposeRestart {
                            project: p.name.clone(),
                            config_files: p.config_files.clone(),
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.compose_pull",
                        shortcut: "p",
                        tone: Tone::Plain,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ComposePull {
                            project: p.name.clone(),
                            config_files: p.config_files.clone(),
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.logs",
                        shortcut: "l",
                        tone: Tone::Plain,
                        disabled_toast: None,
                        action: MenuAction::OpenLogs,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.compose_down",
                        shortcut: "d",
                        tone: Tone::Danger,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ComposeDown {
                            project: p.name.clone(),
                            config_files: p.config_files.clone(),
                            volumes: false,
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.compose_down_vol",
                        shortcut: "D",
                        tone: Tone::Danger,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ComposeDown {
                            project: p.name.clone(),
                            config_files: p.config_files.clone(),
                            volumes: true,
                        }),
                    },
                ];
                Some(DockerModal::ActionMenu {
                    title: p.name.clone(),
                    items,
                    cursor: 0,
                })
            }
            DockerTab::Images => {
                let img = self.selected_image()?;
                let (ids, labels) = self.image_targets()?;
                let items = vec![
                    ActionMenuItem {
                        label_key: "docker.op.image_pull",
                        shortcut: "p",
                        tone: Tone::Accent,
                        disabled_toast: None,
                        action: MenuAction::OpenPull,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.remove_image",
                        shortcut: "d",
                        tone: Tone::Danger,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ImageRemove {
                            ids: ids.clone(),
                            labels: labels.clone(),
                            force: false,
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.remove_image_force",
                        shortcut: "D",
                        tone: Tone::Danger,
                        disabled_toast: None,
                        action: MenuAction::Op(DockerOp::ImageRemove {
                            ids,
                            labels,
                            force: true,
                        }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.prune",
                        shortcut: "P",
                        tone: Tone::Warning,
                        disabled_toast: None,
                        action: MenuAction::OpenPrune,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.copy_id",
                        shortcut: "y",
                        tone: Tone::Muted,
                        disabled_toast: None,
                        action: MenuAction::Copy(img.display_ref()),
                    },
                ];
                Some(DockerModal::ActionMenu {
                    title: img.display_ref(),
                    items,
                    cursor: 0,
                })
            }
            DockerTab::Volumes => {
                let names = self.volume_targets()?;
                let in_use = self
                    .snapshot
                    .volumes
                    .iter()
                    .any(|v| names.contains(&v.name) && !v.used_by.is_empty());
                let first_vol = self
                    .snapshot
                    .volumes
                    .iter()
                    .find(|v| v.name == names[0])
                    .or_else(|| self.selected_volume());
                let title = if names.len() > 1 {
                    format!("{} ({})", names[0], names.len())
                } else {
                    names[0].clone()
                };
                let copy_mount = first_vol
                    .map(|v| v.mountpoint.clone())
                    .unwrap_or_else(|| names[0].clone());
                let items = vec![
                    ActionMenuItem {
                        label_key: "docker.op.remove_volume",
                        shortcut: "d",
                        tone: Tone::Danger,
                        disabled_toast: in_use.then_some("docker.toast.volume_in_use"),
                        action: MenuAction::Op(DockerOp::VolumeRemove { names }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.prune",
                        shortcut: "P",
                        tone: Tone::Warning,
                        disabled_toast: None,
                        action: MenuAction::OpenPrune,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.copy_path",
                        shortcut: "y",
                        tone: Tone::Muted,
                        disabled_toast: None,
                        action: MenuAction::Copy(copy_mount),
                    },
                ];
                Some(DockerModal::ActionMenu {
                    title,
                    items,
                    cursor: 0,
                })
            }
            DockerTab::Networks => {
                let (ids, names) = self.network_targets()?;
                let any_builtin = self
                    .snapshot
                    .networks
                    .iter()
                    .any(|n| ids.contains(&n.id) && n.builtin);
                let title = if names.len() > 1 {
                    format!("{} ({})", names[0], names.len())
                } else {
                    names[0].clone()
                };
                let copy_name = names[0].clone();
                let items = vec![
                    ActionMenuItem {
                        label_key: "docker.op.create_network",
                        shortcut: "c",
                        tone: Tone::Accent,
                        disabled_toast: None,
                        action: MenuAction::OpenCreateNetwork,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.remove_network",
                        shortcut: "d",
                        tone: Tone::Danger,
                        disabled_toast: any_builtin.then_some("docker.toast.builtin_protected"),
                        action: MenuAction::Op(DockerOp::NetworkRemove { ids, names }),
                    },
                    ActionMenuItem {
                        label_key: "docker.op.prune",
                        shortcut: "P",
                        tone: Tone::Warning,
                        disabled_toast: None,
                        action: MenuAction::OpenPrune,
                    },
                    ActionMenuItem {
                        label_key: "docker.op.copy_id",
                        shortcut: "y",
                        tone: Tone::Muted,
                        disabled_toast: None,
                        action: MenuAction::Copy(copy_name),
                    },
                ];
                Some(DockerModal::ActionMenu {
                    title,
                    items,
                    cursor: 0,
                })
            }
            DockerTab::Daemon => None,
        }
    }

    pub fn image_targets(&self) -> Option<(Vec<String>, Vec<String>)> {
        let sel = &self.selected_ids[2];
        if !sel.is_empty() {
            let mut ids = Vec::new();
            let mut labels = Vec::new();
            for img in &self.snapshot.images {
                if sel.contains(&img.id) {
                    ids.push(img.id.clone());
                    labels.push(img.display_ref());
                }
            }
            if !ids.is_empty() {
                return Some((ids, labels));
            }
        }
        self.selected_image()
            .map(|i| (vec![i.id.clone()], vec![i.display_ref()]))
    }

    pub fn volume_targets(&self) -> Option<Vec<String>> {
        let sel = &self.selected_ids[3];
        if !sel.is_empty() {
            let names: Vec<String> = self
                .snapshot
                .volumes
                .iter()
                .filter(|v| sel.contains(&v.name))
                .map(|v| v.name.clone())
                .collect();
            if !names.is_empty() {
                return Some(names);
            }
        }
        self.selected_volume().map(|v| vec![v.name.clone()])
    }

    pub fn network_targets(&self) -> Option<(Vec<String>, Vec<String>)> {
        let sel = &self.selected_ids[4];
        if !sel.is_empty() {
            let mut ids = Vec::new();
            let mut names = Vec::new();
            for n in &self.snapshot.networks {
                if sel.contains(&n.id) && !n.builtin {
                    ids.push(n.id.clone());
                    names.push(n.name.clone());
                }
            }
            if !ids.is_empty() {
                return Some((ids, names));
            }
        }
        self.selected_network()
            .map(|n| (vec![n.id.clone()], vec![n.name.clone()]))
    }

    pub fn apply_event(&mut self, event: DockerEvent) {
        match event {
            DockerEvent::Snapshot(mut snap) => {
                // Invalidate cached container inspects when state/status changes or container disappears
                let prev_states: BTreeMap<&str, (crate::ops::docker::model::ContainerState, &str)> =
                    self.snapshot
                        .containers
                        .iter()
                        .map(|c| (c.id.as_str(), (c.state, c.status.as_str())))
                        .collect();
                let next_states: BTreeMap<&str, (crate::ops::docker::model::ContainerState, &str)> =
                    snap.containers
                        .iter()
                        .map(|c| (c.id.as_str(), (c.state, c.status.as_str())))
                        .collect();
                let mut reset_pending = false;
                self.container_inspects.retain(|id, _| {
                    let keep = next_states.get(id.as_str()) == prev_states.get(id.as_str())
                        && next_states.contains_key(id.as_str());
                    if !keep && self.last_selected_key.as_deref() == Some(id.as_str()) {
                        reset_pending = true;
                    }
                    keep
                });
                if reset_pending {
                    self.last_selected_key = None;
                }

                // Preserve live stats & sparkline history only for currently running containers
                let running_ids: BTreeSet<&str> = snap
                    .containers
                    .iter()
                    .filter(|c| c.state.is_running())
                    .map(|c| c.id.as_str())
                    .collect();
                let mut stats = std::mem::take(&mut self.snapshot.stats);
                stats.retain(|id, _| running_ids.contains(id.as_str()));
                snap.stats = stats;
                let mut cpu_hist = std::mem::take(&mut self.snapshot.cpu_history);
                cpu_hist.retain(|id, _| running_ids.contains(id.as_str()));
                snap.cpu_history = cpu_hist;
                let mut mem_hist = std::mem::take(&mut self.snapshot.mem_history);
                mem_hist.retain(|id, _| running_ids.contains(id.as_str()));
                snap.mem_history = mem_hist;
                self.snapshot = *snap;

                // Prune stale multi-selection keys and removed image inspects
                let c_ids: BTreeSet<&str> = self
                    .snapshot
                    .containers
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect();
                self.selected_ids[0].retain(|id| c_ids.contains(id.as_str()));
                let i_ids: BTreeSet<&str> =
                    self.snapshot.images.iter().map(|i| i.id.as_str()).collect();
                self.selected_ids[2].retain(|id| i_ids.contains(id.as_str()));
                self.image_inspects
                    .retain(|id, _| i_ids.contains(id.as_str()));
                let v_names: BTreeSet<&str> = self
                    .snapshot
                    .volumes
                    .iter()
                    .map(|v| v.name.as_str())
                    .collect();
                self.selected_ids[3].retain(|name| v_names.contains(name.as_str()));
                let n_ids: BTreeSet<&str> = self
                    .snapshot
                    .networks
                    .iter()
                    .map(|n| n.id.as_str())
                    .collect();
                self.selected_ids[4].retain(|id| n_ids.contains(id.as_str()));
                self.clamp_cursors();
            }
            DockerEvent::Stats(stats) => {
                self.snapshot.record_stats(stats);
            }
            DockerEvent::ContainerInspect { id, result } => {
                if let Ok(inspect) = result {
                    self.container_inspects.insert(id, *inspect);
                }
            }
            DockerEvent::ImageInspect { id, result } => {
                if let Ok(inspect) = result {
                    self.image_inspects.insert(id, *inspect);
                }
            }
            DockerEvent::ComposeYaml { path, result } => {
                if let Ok(text) = result {
                    self.compose_yamls.insert(path, text);
                }
            }
        }
    }

    pub fn apply_log_event(&mut self, event: LogStreamEvent) {
        match event {
            LogStreamEvent::Batch(entries) => self.logs.push_batch(entries),
            LogStreamEvent::Ended(_) => self.logs.ended = true,
        }
    }

    pub fn running_badge(&self) -> Option<usize> {
        if !self.active_ever || !matches!(self.snapshot.availability, DockerAvailability::Ready) {
            return None;
        }
        Some(self.snapshot.running_containers())
    }

    /// Build a quick exec spec for the currently focused running container.
    pub fn quick_exec_spec(&self) -> Option<ExecSpec> {
        let c = self.selected_container()?;
        if !c.state.is_running() {
            return None;
        }
        Some(ExecSpec {
            container_id: c.id.clone(),
            container_name: c.name.clone(),
            shell: "auto".into(),
            user: String::new(),
            workdir: String::new(),
        })
    }
}
