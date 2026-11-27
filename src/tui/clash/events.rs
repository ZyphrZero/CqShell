//! Keyboard and mouse event routing for the dedicated Mihomo visual management workspace.

use super::{
    layout::{ClashBodyGeom, ClashModalGeometry, OutboundRowAction},
    state::{ClashConfirmAction, ClashModal, ToolEntry},
};
use crate::{
    actions,
    error::Result,
    ops::clash::{
        controller::{validate_delay_timeout, validate_test_url},
        model::{ClashAvailability, ClashTab, ServiceTarget},
        monitor::ClashMonitorCmd,
    },
    tui::{
        input::InputBuffer,
        layout::LayoutRegions,
        palette,
        state::{App, Effect, NavTarget, Tone},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

impl App {
    pub(in crate::tui) fn clash_has_modal(&self) -> bool {
        self.clash.modal.is_some()
    }

    fn clash_toast_key(&mut self, key: &str, tone: Tone) {
        let msg = self.ctx.i18n.text(key).to_owned();
        self.toast(msg, tone);
    }

    pub(in crate::tui) fn clash_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let idle = self.worker.is_none();

        // 1. Local Mihomo modal (DelaySettings / GroupPicker / Providers / Confirm)
        if self.clash.modal.is_some() {
            return self.clash_modal_key(key);
        }

        // 2. Global Ctrl+K Command Palette always reachable
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('k' | 'K'))
        {
            self.command_palette = Some(palette::CommandPalette::default());
            return Ok(Effect::None);
        }

        // 3. Inline filter capture
        if self.clash.filtering {
            return self.clash_filter_key(key);
        }

        // 4. Fullscreen real-time traffic atlas overlay
        if self.clash.traffic_fullscreen {
            return self.clash_traffic_fullscreen_key(key);
        }

        // 5. Fullscreen redacted logs overlay
        if self.clash.tools_log_fullscreen {
            return self.clash_log_key(key, true);
        }

        // 5. Fullscreen connection inspector overlay
        if self.clash.conn_inspector_fullscreen {
            let max_s = self.clash.max_conn_inspector_scroll.get();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'i') => {
                    self.clash.conn_inspector_fullscreen = false;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.clash.conn_inspector_scroll =
                        self.clash.conn_inspector_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.clash.conn_inspector_scroll = self
                        .clash
                        .conn_inspector_scroll
                        .saturating_add(1)
                        .min(max_s);
                }
                KeyCode::PageUp => {
                    self.clash.conn_inspector_scroll =
                        self.clash.conn_inspector_scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    self.clash.conn_inspector_scroll = self
                        .clash
                        .conn_inspector_scroll
                        .saturating_add(10)
                        .min(max_s);
                }
                KeyCode::Char('x' | 'd') => {
                    if let Some(c) = self.clash.selected_connection()
                        && !c.closed
                    {
                        let id = c.id.clone();
                        self.clash.conn_inspector_fullscreen = false;
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::CloseConnection { id }));
                    }
                }
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.help = true;
                    self.help_scroll = 0;
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 6. Alt+1..6 / Alt+Left/Right switch Mihomo tabs; Alt+Up/Down reorders sidebar category
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(d @ '1'..='6') = key.code {
                let idx = (d as u8 - b'1') as usize;
                let tab = ClashTab::from_index(idx);
                return Ok(self.switch_clash_tab(tab));
            }
            if key.code == KeyCode::Left {
                return Ok(self.cycle_clash_tab(false));
            }
            if key.code == KeyCode::Right {
                return Ok(self.cycle_clash_tab(true));
            }
            if matches!(key.code, KeyCode::Up | KeyCode::Down) {
                if idle {
                    self.reorder_category(key.code == KeyCode::Down);
                }
                return Ok(Effect::None);
            }
        }

        // 7. Ctrl+U / Ctrl+D scroll right inspector / logs
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u' | 'U') => {
                    if self.clash.tab == ClashTab::Connections {
                        self.clash.conn_inspector_scroll =
                            self.clash.conn_inspector_scroll.saturating_sub(6);
                    } else if self.clash.tab == ClashTab::Tools && self.clash.tools_log_mode {
                        self.clash.log_scroll = self.clash.log_scroll.saturating_sub(10);
                    }
                    return Ok(Effect::None);
                }
                KeyCode::Char('d' | 'D') => {
                    if self.clash.tab == ClashTab::Connections {
                        let max_s = self.clash.max_conn_inspector_scroll.get();
                        self.clash.conn_inspector_scroll = self
                            .clash
                            .conn_inspector_scroll
                            .saturating_add(6)
                            .min(max_s);
                    } else if self.clash.tab == ClashTab::Tools && self.clash.tools_log_mode {
                        let max_s = self.clash.max_log_scroll.get();
                        self.clash.log_scroll = self.clash.log_scroll.saturating_add(10).min(max_s);
                    }
                    return Ok(Effect::None);
                }
                _ => {}
            }
        }

        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char(_))
        {
            return Ok(Effect::None);
        }

        // 8. Global shell & workspace keys
        match key.code {
            KeyCode::Esc => {
                if !idle {
                    self.request_cancel(false);
                } else if !self.clash.filter.text.is_empty() {
                    self.clash.filter.clear();
                    self.clash.clamp_cursors();
                    self.clash_toast_key("clash.toast.cleared_filter", Tone::Info);
                } else if self.clash.tab == ClashTab::Connections && self.clash.conn_facet.is_some()
                {
                    self.clash.conn_facet = None;
                    self.clash.clamp_cursors();
                } else if self.clash.tab == ClashTab::Rules && self.clash.rule_facet.is_some() {
                    self.clash.rule_facet = None;
                    self.clash.clamp_cursors();
                } else if self.clash.tab == ClashTab::Tools && self.clash.tools_log_mode {
                    self.clash.tools_log_mode = false;
                } else if self.clash.fullscreen && !self.clash.launched_standalone {
                    self.clash.fullscreen = false;
                }
                return Ok(Effect::None);
            }
            KeyCode::Char('[') => {
                self.toggle_sidebar();
                return Ok(Effect::None);
            }
            KeyCode::Char('T') => {
                self.open_theme_drawer();
                return Ok(Effect::None);
            }
            KeyCode::Char('L') => {
                self.cycle_language()?;
                return Ok(Effect::None);
            }
            KeyCode::Char('G') => {
                if self.viewport.height.saturating_sub(2) < 16 {
                    self.log_modal = !self.log_modal;
                } else {
                    self.logs_open = !self.logs_open;
                }
                return Ok(Effect::None);
            }
            KeyCode::F(2) => {
                self.log_modal = !self.log_modal;
                return Ok(Effect::None);
            }
            KeyCode::Char('S') => {
                return self.open_socks_secret_modal();
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
                return Ok(Effect::None);
            }
            KeyCode::Char('q' | 'Q') if idle => return Ok(Effect::Quit),
            KeyCode::Char('z' | 'F') => {
                self.clash.fullscreen = !self.clash.fullscreen;
                return Ok(Effect::None);
            }
            KeyCode::Char('r') => {
                if self.clash.tab == ClashTab::Tools && self.clash.tools_log_mode {
                    self.clash.log_loading = true;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
                }
                return Ok(Effect::ClashRefresh);
            }
            KeyCode::Char('R') => {
                if self.clash.tab == ClashTab::Overview && idle {
                    return self.clash_trigger_action("clash.reload");
                }
                return Ok(Effect::ClashRefresh);
            }
            KeyCode::Char(d @ '1'..='6') if self.clash.fullscreen => {
                let idx = (d as u8 - b'1') as usize;
                return Ok(self.switch_clash_tab(ClashTab::from_index(idx)));
            }
            KeyCode::Char(d @ '1'..='9') if !self.clash.fullscreen => {
                let idx = (d as u8 - b'1') as usize;
                if let Some(&group) = self.category_order.get(idx) {
                    self.goto_nav(NavTarget::Group(group));
                }
                return Ok(Effect::None);
            }
            KeyCode::Char('H') if !self.clash.fullscreen => {
                self.goto_nav(NavTarget::Overview);
                return Ok(Effect::None);
            }
            KeyCode::Char('0') if !self.clash.fullscreen => {
                self.goto_nav(NavTarget::Optimization);
                return Ok(Effect::None);
            }
            KeyCode::Char('O')
                if !self.clash.fullscreen
                    && !matches!(self.clash.tab, ClashTab::Proxies | ClashTab::Connections) =>
            {
                self.goto_nav(NavTarget::Optimization);
                return Ok(Effect::None);
            }
            // Tab / Shift+Tab: switch CqShell sidebar categories in embedded mode (parity with
            // main & Docker views), or cycle the 6 Mihomo workspace tabs uniformly in focus mode.
            KeyCode::Tab => {
                if self.clash.fullscreen {
                    return Ok(self.cycle_clash_tab(true));
                }
                self.cycle_nav(true);
                return Ok(Effect::None);
            }
            KeyCode::BackTab => {
                if self.clash.fullscreen {
                    return Ok(self.cycle_clash_tab(false));
                }
                self.cycle_nav(false);
                return Ok(Effect::None);
            }
            // `{` / `}` switch Mihomo tabs in both embedded and fullscreen modes without needing Alt.
            KeyCode::Char('{') => {
                return Ok(self.cycle_clash_tab(false));
            }
            KeyCode::Char('}') => {
                return Ok(self.cycle_clash_tab(true));
            }
            KeyCode::Char('/') if self.clash.tab != ClashTab::Overview => {
                self.clash.filtering = true;
                return Ok(Effect::None);
            }
            _ => {}
        }

        // Offline / stopped CTA handling on controller-dependent tabs
        let needs_ctrl = matches!(
            self.clash.tab,
            ClashTab::Overview | ClashTab::Proxies | ClashTab::Connections | ClashTab::Rules
        );
        if needs_ctrl && !self.clash.snapshot.availability.is_ready() {
            if key.code == KeyCode::Enter && idle {
                return self.clash_trigger_unavailable_cta();
            }
            if matches!(key.code, KeyCode::Char('c'))
                && idle
                && self.clash.snapshot.availability == ClashAvailability::ControllerUnconfigured
                && self.clash.busy_hint.is_none()
            {
                return self.clash_trigger_action("clash.controller.configure");
            }
            return Ok(Effect::None);
        }

        // 9. Tab-specific key dispatch
        match self.clash.tab {
            ClashTab::Overview => self.clash_overview_key(key, idle),
            ClashTab::Proxies => self.clash_proxies_key(key),
            ClashTab::Profiles => self.clash_profiles_key(key, idle),
            ClashTab::Connections => self.clash_connections_key(key),
            ClashTab::Rules => self.clash_rules_key(key),
            ClashTab::Tools => self.clash_tools_key(key, idle),
        }
    }

    fn switch_clash_tab(&mut self, tab: ClashTab) -> Effect {
        self.clash.set_tab(tab);
        if tab == ClashTab::Tools && self.clash.tools_log_mode && self.clash.logs.is_empty() {
            self.clash.log_loading = true;
            return Effect::ClashCmd(ClashMonitorCmd::FetchLogs);
        }
        Effect::None
    }

    fn cycle_clash_tab(&mut self, forward: bool) -> Effect {
        let len = ClashTab::ALL.len();
        let cur = self.clash.tab.index();
        let next = if forward {
            (cur + 1) % len
        } else {
            (cur + len - 1) % len
        };
        self.switch_clash_tab(ClashTab::from_index(next))
    }

    fn cycle_facet(slot: &mut Option<String>, facets: &[String], forward: bool) {
        if facets.is_empty() {
            *slot = None;
            return;
        }
        *slot = match slot.as_deref() {
            None => {
                if forward {
                    Some(facets[0].clone())
                } else {
                    Some(facets[facets.len() - 1].clone())
                }
            }
            Some(cur) => match facets.iter().position(|f| f == cur) {
                Some(pos) if forward => facets.get(pos + 1).cloned(),
                Some(0) => None,
                Some(pos) => facets.get(pos - 1).cloned(),
                None => {
                    if forward {
                        Some(facets[0].clone())
                    } else {
                        Some(facets[facets.len() - 1].clone())
                    }
                }
            },
        };
    }

    fn open_outbound_ip_modal(&mut self) -> Effect {
        self.clash.modal = Some(ClashModal::OutboundIp { scroll: 0 });
        if self.clash.snapshot.availability.is_ready()
            && self.clash.exit_probe.is_none()
            && !self.clash.exit_probing
        {
            self.clash.exit_probing = true;
            return Effect::ClashCmd(ClashMonitorCmd::ProbeExit);
        }
        Effect::None
    }

    fn open_service_status_modal(&mut self, focus: Option<ServiceTarget>) -> Effect {
        let enabled = self.clash.enabled_services();
        let cursor = focus
            .and_then(|t| enabled.iter().position(|&x| x == t))
            .unwrap_or(0);
        self.clash.modal = Some(ClashModal::ServiceStatus {
            cursor,
            manage: false,
        });
        if !self.clash.snapshot.availability.is_ready() || self.clash.any_service_probing() {
            return Effect::None;
        }
        if let Some(target) = focus {
            if self
                .clash
                .service_entry(target)
                .is_none_or(|e| !e.probing && e.result.is_none())
            {
                return self.trigger_service_check(vec![target], false);
            }
            return Effect::None;
        }
        let any_checked = enabled.iter().any(|t| {
            self.clash
                .service_entry(*t)
                .is_some_and(|e| e.probing || e.result.is_some())
        });
        if !any_checked {
            let pinned = self.clash.pinned_service_targets(4);
            return self.trigger_service_check(pinned, false);
        }
        Effect::None
    }

    fn trigger_service_check(&mut self, targets: Vec<ServiceTarget>, manual: bool) -> Effect {
        if targets.is_empty() {
            return Effect::None;
        }
        if !self.clash.snapshot.availability.is_ready() {
            self.clash_toast_key("clash.toast.service_need_running", Tone::Warning);
            return Effect::None;
        }
        for &target in &targets {
            self.clash.service_states.entry(target).or_default().probing = true;
        }
        Effect::ClashCmd(ClashMonitorCmd::CheckServices { targets, manual })
    }

    fn sync_service_prefs(&mut self) -> Effect {
        self.persist_clash_service_prefs();
        Effect::ClashCmd(ClashMonitorCmd::SetPinnedServices(
            self.clash.pinned_service_targets(4),
        ))
    }

    fn copy_focused_service_ip(&mut self, cursor: usize) -> Effect {
        let enabled = self.clash.enabled_services();
        if let Some(&target) = enabled.get(cursor)
            && let Some(ip) = self
                .clash
                .service_entry(target)
                .and_then(|e| e.result.as_ref())
                .and_then(|r| r.ip.as_deref())
                .map(ToOwned::to_owned)
        {
            let msg = self.ctx.i18n.format(
                "clash.toast.service_ip_copied",
                &[("service", target.label())],
            );
            self.toast(msg, Tone::Success);
            return Effect::Copy(ip);
        }
        Effect::None
    }

    fn execute_outbound_row_action(&mut self, action: OutboundRowAction) -> Effect {
        match action {
            OutboundRowAction::CopyIp => {
                if let Some(ip) = self.clash.exit_probe.as_ref().map(|p| p.ip.clone()) {
                    self.clash_toast_key("clash.toast.outbound_ip_copied", Tone::Success);
                    return Effect::Copy(ip);
                }
            }
            OutboundRowAction::CopyOrg => {
                if let Some(org) = self
                    .clash
                    .exit_probe
                    .as_ref()
                    .and_then(|p| p.organization_name())
                    .map(ToOwned::to_owned)
                {
                    let msg = self
                        .ctx
                        .i18n
                        .format("clash.toast.outbound_org_copied", &[("value", &org)]);
                    self.toast(msg, Tone::Success);
                    return Effect::Copy(org);
                }
            }
            OutboundRowAction::CopyAsn => {
                if let Some(asn) = self
                    .clash
                    .exit_probe
                    .as_ref()
                    .and_then(|p| p.quality_ready())
                    .and_then(|q| q.asn_label())
                {
                    let msg = self
                        .ctx
                        .i18n
                        .format("clash.toast.outbound_asn_copied", &[("value", &asn)]);
                    self.toast(msg, Tone::Success);
                    return Effect::Copy(asn);
                }
            }
            OutboundRowAction::ToggleHideIp => {
                self.toggle_clash_hide_ip();
                let key = if self.clash.hide_ip {
                    "clash.toast.outbound_hide_on"
                } else {
                    "clash.toast.outbound_hide_off"
                };
                self.clash_toast_key(key, Tone::Info);
            }
        }
        Effect::None
    }

    fn clash_trigger_action(&mut self, id: &str) -> Result<Effect> {
        let spec = actions::find(id)?;
        self.activate_action(spec)
    }

    fn clash_trigger_webui(&mut self, idle: bool) -> Result<Effect> {
        if let Some(url) = self.clash.snapshot.summary.panel_url.clone() {
            let key = if self.clash.snapshot.summary.panel_loopback {
                "clash.toast.webui_copied_tunnel"
            } else {
                "clash.toast.webui_copied"
            };
            let msg = self.ctx.i18n.format(key, &[("url", &url)]);
            self.toast(msg, Tone::Success);
            return Ok(Effect::Copy(url));
        }
        if idle {
            return self.clash_trigger_action("clash.panel.install");
        }
        Ok(Effect::None)
    }

    fn clash_trigger_unavailable_cta(&mut self) -> Result<Effect> {
        match self.clash.snapshot.availability {
            ClashAvailability::NotInstalled => self.clash_trigger_action("clash.install"),
            ClashAvailability::Stopped => self.clash_trigger_action("clash.start"),
            ClashAvailability::ControllerUnconfigured => {
                if self.clash.busy_hint.is_some() {
                    return Ok(Effect::None);
                }
                self.clash.busy_hint = Some("controller".into());
                Ok(Effect::ClashCmd(ClashMonitorCmd::EnableLocalController))
            }
            _ => Ok(Effect::ClashRefresh),
        }
    }

    fn clash_traffic_fullscreen_key(&mut self, key: KeyEvent) -> Result<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'v') => {
                self.clash.traffic_fullscreen = false;
            }
            KeyCode::Char('g') => {
                self.clash.traffic_mode = self.clash.traffic_mode.next();
            }
            KeyCode::Char('t') => {
                self.clash.traffic_window = self.clash.traffic_window.next();
            }
            KeyCode::Char('y') => {
                self.clash.traffic_scale = self.clash.traffic_scale.toggle();
            }
            KeyCode::Char(' ') => {
                self.clash.toggle_traffic_pause();
            }
            KeyCode::Char('b') => {
                self.clash.traffic_breakdown = self.clash.traffic_breakdown.next();
            }
            KeyCode::Char('r') => {
                return Ok(Effect::ClashRefresh);
            }
            KeyCode::Char('m') => {
                let next = match self
                    .clash
                    .snapshot
                    .summary
                    .mode
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "rule" => "global",
                    "global" => "direct",
                    _ => "rule",
                };
                self.clash.busy_hint = Some(next.to_owned());
                return Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                    mode: next.to_owned(),
                }));
            }
            KeyCode::Char('z' | 'F') => {
                self.clash.fullscreen = !self.clash.fullscreen;
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_overview_key(&mut self, key: KeyEvent, idle: bool) -> Result<Effect> {
        let group_count = self.clash.visible_groups().len();
        let group_cols = self.clash.last_overview_group_cols.get().max(1);
        match key.code {
            KeyCode::Char('v') => {
                self.clash.traffic_fullscreen = true;
            }
            KeyCode::Char('g') => {
                self.clash.traffic_mode = self.clash.traffic_mode.next();
            }
            KeyCode::Char('t') => {
                self.clash.traffic_window = self.clash.traffic_window.next();
            }
            KeyCode::Char('y') => {
                self.clash.traffic_scale = self.clash.traffic_scale.toggle();
            }
            KeyCode::Char(' ') => {
                self.clash.toggle_traffic_pause();
            }
            KeyCode::Char('m') => {
                let next = match self
                    .clash
                    .snapshot
                    .summary
                    .mode
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "rule" => "global",
                    "global" => "direct",
                    _ => "rule",
                };
                self.clash.busy_hint = Some(next.to_owned());
                return Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                    mode: next.to_owned(),
                }));
            }
            KeyCode::Char('s') if idle => {
                let action = if self.clash.snapshot.availability.is_ready() {
                    "clash.stop"
                } else {
                    "clash.start"
                };
                return self.clash_trigger_action(action);
            }
            KeyCode::Char('i' | 'I') => {
                return Ok(self.open_outbound_ip_modal());
            }
            KeyCode::Char('c' | 'C') => {
                return Ok(self.open_service_status_modal(None));
            }
            KeyCode::Char('N') => {
                self.clash.exit_probing = true;
                self.clash.exit_probe_error = None;
                return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
            }
            KeyCode::Char('w') => {
                return self.clash_trigger_webui(idle);
            }
            KeyCode::Char('p') => {
                return Ok(self.switch_clash_tab(ClashTab::Profiles));
            }
            KeyCode::Char('u') => {
                if let Some(prof) = self.clash.snapshot.active_profile()
                    && !prof.is_local
                {
                    let name = prof.name.clone();
                    if !self.clash.updating_all_profiles
                        && self.clash.busy_profiles.insert(name.clone())
                    {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { name }));
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') if group_count > 0 => {
                self.clash.overview_group_cursor =
                    self.clash.overview_group_cursor.saturating_sub(group_cols);
            }
            KeyCode::Down | KeyCode::Char('j') if group_count > 0 => {
                self.clash.overview_group_cursor =
                    (self.clash.overview_group_cursor + group_cols).min(group_count - 1);
            }
            KeyCode::Left | KeyCode::Char('h') if group_count > 0 => {
                self.clash.overview_group_cursor =
                    self.clash.overview_group_cursor.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Char('l') if group_count > 0 => {
                self.clash.overview_group_cursor =
                    (self.clash.overview_group_cursor + 1).min(group_count - 1);
            }
            KeyCode::Enter if group_count > 0 => {
                let target = self.clash.overview_group_cursor;
                self.clash.set_tab(ClashTab::Proxies);
                self.clash.select_group_index(target);
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_proxies_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let cols = self.clash.last_grid_cols.get().max(1);
        let node_count = self.clash.filtered_nodes().len();
        match key.code {
            // Policy group switching (`,/.` symmetric pair + `g` picker; keeps `Tab` free for shell/tabs)
            KeyCode::Char('.') => {
                self.clash.cycle_group(true);
            }
            KeyCode::Char(',') => {
                self.clash.cycle_group(false);
            }
            KeyCode::Char('g') => {
                if !self.clash.visible_groups().is_empty() {
                    self.clash.modal = Some(ClashModal::GroupPicker {
                        cursor: self.clash.group_cursor,
                    });
                }
            }
            // 2D grid navigation (`h` toggles hide timeout proxies; Arrow Left moves grid left)
            KeyCode::Left if node_count > 0 => {
                self.clash.node_cursor = self.clash.node_cursor.saturating_sub(1);
            }
            KeyCode::Char('h') => {
                self.clash.hide_timeout = !self.clash.hide_timeout;
                self.clash.clamp_cursors();
            }
            KeyCode::Right if node_count > 0 => {
                self.clash.node_cursor = (self.clash.node_cursor + 1).min(node_count - 1);
            }
            KeyCode::Char('l') if node_count > 0 => {
                self.clash.node_cursor = (self.clash.node_cursor + 1).min(node_count - 1);
            }
            KeyCode::Up | KeyCode::Char('k') if node_count > 0 => {
                self.clash.node_cursor = self.clash.node_cursor.saturating_sub(cols);
            }
            KeyCode::Down | KeyCode::Char('j') if node_count > 0 => {
                self.clash.node_cursor = (self.clash.node_cursor + cols).min(node_count - 1);
            }
            KeyCode::PageUp if node_count > 0 => {
                self.clash.node_cursor = self.clash.node_cursor.saturating_sub(cols * 5);
            }
            KeyCode::PageDown if node_count > 0 => {
                self.clash.node_cursor = (self.clash.node_cursor + cols * 5).min(node_count - 1);
            }
            KeyCode::Home => self.clash.node_cursor = 0,
            KeyCode::End if node_count > 0 => self.clash.node_cursor = node_count - 1,

            // Select node (Selector / URLTest / Fallback)
            KeyCode::Enter => {
                if let (Some(group), Some(node)) =
                    (self.clash.current_group(), self.clash.selected_node())
                {
                    if !group.kind.supports_select() {
                        self.clash_toast_key("clash.toast.group_readonly", Tone::Warning);
                    } else {
                        let g = group.name.clone();
                        let n = node.name.clone();
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::SelectProxy {
                            group: g,
                            node: n,
                        }));
                    }
                }
            }

            // One-click group delay test
            KeyCode::Char('t') => {
                if let Some(group) = self.clash.current_group() {
                    let g = group.name.clone();
                    if self.clash.testing_groups.insert(g.clone()) {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::TestGroupDelay {
                            group: g,
                            url: self.clash.delay_url.clone(),
                            timeout_ms: self.clash.delay_timeout_ms,
                        }));
                    }
                }
            }

            // Single-node delay test
            KeyCode::Char('p') => {
                if let (Some(group), Some(node)) =
                    (self.clash.current_group(), self.clash.selected_node())
                {
                    let g = group.name.clone();
                    let n = node.name.clone();
                    if self.clash.testing_nodes.insert(n.clone()) {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::TestNodeDelay {
                            group: g,
                            node: n,
                            url: self.clash.delay_url.clone(),
                            timeout_ms: self.clash.delay_timeout_ms,
                        }));
                    }
                }
            }

            // Explicit one-click fastest reachable node selection
            KeyCode::Char('f') => {
                if let Some((g, n, _ms)) = self.clash.fastest_node_in_current_group() {
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::SelectProxy {
                        group: g,
                        node: n,
                    }));
                }
                self.clash_toast_key("clash.toast.no_tested_nodes", Tone::Warning);
            }

            // Unfix pinned URLTest / Fallback group back to automatic
            KeyCode::Char('x') => {
                if let Some(group) = self.clash.current_group() {
                    if group.kind.supports_unfix() {
                        let g = group.name.clone();
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::UnfixGroup { group: g }));
                    }
                    self.clash_toast_key("clash.toast.unfix_auto_only", Tone::Info);
                }
            }

            // Sort cycle / flip direction
            KeyCode::Char('o') => {
                self.clash.proxy_sort = self.clash.proxy_sort.next();
                self.clash.proxy_sort_asc = true;
                self.clash.clamp_cursors();
            }
            KeyCode::Char('O') => {
                self.clash.proxy_sort_asc = !self.clash.proxy_sort_asc;
                self.clash.clamp_cursors();
            }

            // Locate currently selected node
            KeyCode::Char('c') => {
                if !self.clash.locate_active_node() {
                    self.clash_toast_key("clash.toast.locate_miss", Tone::Info);
                }
            }

            // Temporary delay URL / timeout settings modal (`d` per spec, `e` alias)
            KeyCode::Char('e' | 'd') => {
                self.clash.modal = Some(ClashModal::DelaySettings {
                    url: InputBuffer::new(self.clash.delay_url.clone()),
                    timeout: InputBuffer::new(self.clash.delay_timeout_ms.to_string()),
                    active_field: 0,
                    error: None,
                });
            }

            // External Proxy Providers modal
            KeyCode::Char('P') => {
                self.clash.modal = Some(ClashModal::Providers {
                    show_rules: false,
                    cursor: 0,
                });
            }

            // Mode cycle shortcut
            KeyCode::Char('m') => {
                let next = match self
                    .clash
                    .snapshot
                    .summary
                    .mode
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "rule" => "global",
                    "global" => "direct",
                    _ => "rule",
                };
                return Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                    mode: next.to_owned(),
                }));
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_profiles_key(&mut self, key: KeyEvent, idle: bool) -> Result<Effect> {
        let count = self.clash.filtered_profiles().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.clash.profile_cursor = self.clash.profile_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.clash.profile_cursor = (self.clash.profile_cursor + 1).min(count - 1);
            }
            KeyCode::Home => self.clash.profile_cursor = 0,
            KeyCode::End if count > 0 => self.clash.profile_cursor = count - 1,
            KeyCode::Enter => {
                if let Some(prof) = self.clash.selected_profile() {
                    let name = prof.name.clone();
                    self.clash.busy_hint = Some(name.clone());
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::SwitchProfile { name }));
                }
            }
            KeyCode::Char('u') => {
                if let Some(prof) = self.clash.selected_profile() {
                    if prof.is_local {
                        self.clash_toast_key("clash.toast.local_profile_no_url", Tone::Warning);
                    } else {
                        let name = prof.name.clone();
                        if !self.clash.updating_all_profiles
                            && self.clash.busy_profiles.insert(name.clone())
                        {
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { name }));
                        }
                    }
                }
            }
            KeyCode::Char('U') => {
                if !self.clash.updating_all_profiles && self.clash.busy_profiles.is_empty() {
                    self.clash.updating_all_profiles = true;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateAllProfiles));
                }
            }
            KeyCode::Char('a') if idle => return self.clash_trigger_action("clash.sub.add"),
            KeyCode::Char('i' | 'I') if idle => {
                return self.clash_trigger_action("clash.sub.import");
            }
            KeyCode::Char('d') => {
                if let Some(prof) = self.clash.selected_profile() {
                    let name = prof.name.clone();
                    let msg = self
                        .ctx
                        .i18n
                        .format("clash.confirm.remove_profile", &[("name", &name)]);
                    self.clash.modal = Some(ClashModal::Confirm {
                        title: self.ctx.i18n.text("clash.confirm.title").to_owned(),
                        message: msg,
                        danger: true,
                        accept: false,
                        action: ClashConfirmAction::RemoveProfile(name),
                    });
                }
            }
            KeyCode::Char('P') => {
                self.clash.modal = Some(ClashModal::Providers {
                    show_rules: false,
                    cursor: 0,
                });
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_connections_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let count = self.clash.filtered_connections().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.clash.conn_cursor = self.clash.conn_cursor.saturating_sub(1);
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.clash.conn_cursor = (self.clash.conn_cursor + 1).min(count - 1);
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::PageUp if count > 0 => {
                self.clash.conn_cursor = self.clash.conn_cursor.saturating_sub(10);
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::PageDown if count > 0 => {
                self.clash.conn_cursor = (self.clash.conn_cursor + 10).min(count - 1);
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::Home => {
                self.clash.conn_cursor = 0;
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::End if count > 0 => {
                self.clash.conn_cursor = count - 1;
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::Char('a') => {
                self.clash.conn_scope = self.clash.conn_scope.next();
                self.clash.conn_facet = None;
                self.clash.clamp_cursors();
            }
            KeyCode::Char('o') => {
                self.clash.conn_sort = self.clash.conn_sort.next();
                self.clash.conn_sort_asc = false;
                self.clash.clamp_cursors();
            }
            KeyCode::Char('O') => {
                self.clash.conn_sort_asc = !self.clash.conn_sort_asc;
                self.clash.clamp_cursors();
            }
            KeyCode::Char('.' | 'f') => {
                let facets = self.clash.available_conn_facets();
                Self::cycle_facet(&mut self.clash.conn_facet, &facets, true);
                self.clash.clamp_cursors();
            }
            KeyCode::Char(',') => {
                let facets = self.clash.available_conn_facets();
                Self::cycle_facet(&mut self.clash.conn_facet, &facets, false);
                self.clash.clamp_cursors();
            }
            KeyCode::Enter | KeyCode::Char('i') if count > 0 => {
                self.clash.conn_inspector_fullscreen = true;
                self.clash.conn_inspector_scroll = 0;
            }
            KeyCode::Char('x' | 'd') => {
                if let Some(conn) = self.clash.selected_connection()
                    && !conn.closed
                {
                    let id = conn.id.clone();
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::CloseConnection { id }));
                }
            }
            KeyCode::Char('X' | 'D') if !self.clash.snapshot.connections.is_empty() => {
                self.clash.modal = Some(ClashModal::Confirm {
                    title: self.ctx.i18n.text("clash.confirm.title").to_owned(),
                    message: self
                        .ctx
                        .i18n
                        .text("clash.confirm.close_all_conns")
                        .to_owned(),
                    danger: true,
                    accept: false,
                    action: ClashConfirmAction::CloseAllConnections,
                });
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_rules_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let count = self.clash.filtered_rules().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.clash.rule_cursor = self.clash.rule_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.clash.rule_cursor = (self.clash.rule_cursor + 1).min(count - 1);
            }
            KeyCode::PageUp if count > 0 => {
                self.clash.rule_cursor = self.clash.rule_cursor.saturating_sub(15);
            }
            KeyCode::PageDown if count > 0 => {
                self.clash.rule_cursor = (self.clash.rule_cursor + 15).min(count - 1);
            }
            KeyCode::Home => self.clash.rule_cursor = 0,
            KeyCode::End if count > 0 => self.clash.rule_cursor = count - 1,
            KeyCode::Char('.' | 'f') => {
                let facets = self.clash.available_rule_facets();
                Self::cycle_facet(&mut self.clash.rule_facet, &facets, true);
                self.clash.clamp_cursors();
            }
            KeyCode::Char(',') => {
                let facets = self.clash.available_rule_facets();
                Self::cycle_facet(&mut self.clash.rule_facet, &facets, false);
                self.clash.clamp_cursors();
            }
            KeyCode::Char('P') => {
                self.clash.modal = Some(ClashModal::Providers {
                    show_rules: true,
                    cursor: 0,
                });
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_tools_key(&mut self, key: KeyEvent, idle: bool) -> Result<Effect> {
        if self.clash.tools_log_mode {
            return self.clash_log_key(key, false);
        }
        let tools = self.clash.filtered_tools(&self.ctx.i18n);
        let count = tools.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.clash.tool_cursor = self.clash.tool_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.clash.tool_cursor = (self.clash.tool_cursor + 1).min(count - 1);
            }
            KeyCode::Home => self.clash.tool_cursor = 0,
            KeyCode::End if count > 0 => self.clash.tool_cursor = count - 1,
            KeyCode::Char('l') => {
                self.clash.tools_log_mode = true;
                self.clash.log_scroll = 0;
                self.clash.log_loading = true;
                return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
            }
            KeyCode::Enter if idle => {
                if let Some(item) = tools.get(self.clash.tool_cursor) {
                    return self.execute_clash_tool_entry(item.entry);
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn execute_clash_tool_entry(&mut self, entry: ToolEntry) -> Result<Effect> {
        match entry {
            ToolEntry::ServiceStart => self.clash_trigger_action("clash.start"),
            ToolEntry::ServiceStop => self.clash_trigger_action("clash.stop"),
            ToolEntry::ServiceReload => self.clash_trigger_action("clash.reload"),
            ToolEntry::OutboundIp => Ok(self.open_outbound_ip_modal()),
            ToolEntry::ServiceStatus => Ok(self.open_service_status_modal(None)),
            ToolEntry::SocksSecretCard => self.open_socks_secret_modal(),
            ToolEntry::ViewLogs => {
                self.clash.tools_log_mode = true;
                self.clash.log_scroll = 0;
                self.clash.log_loading = true;
                Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs))
            }
            ToolEntry::CqsAction(id) => self.clash_trigger_action(id),
        }
    }

    fn clash_log_key(&mut self, key: KeyEvent, fullscreen: bool) -> Result<Effect> {
        let max_s = self.clash.max_log_scroll.get();
        match key.code {
            KeyCode::Esc if fullscreen => {
                if !self.clash.filter.text.is_empty() {
                    self.clash.filter.clear();
                    self.clash.log_scroll = 0;
                    self.clash_toast_key("clash.toast.cleared_filter", Tone::Info);
                } else {
                    self.clash.tools_log_fullscreen = false;
                }
            }
            KeyCode::Char('q') if fullscreen => {
                self.clash.tools_log_fullscreen = false;
            }
            KeyCode::Char('/') => {
                self.clash.filtering = true;
            }
            KeyCode::Char('y') => {
                let lines = self.clash.filtered_logs();
                if lines.is_empty() {
                    self.clash_toast_key("clash.toast.logs_empty_copy", Tone::Info);
                } else {
                    let count = lines.len().to_string();
                    let text = lines.join("\n");
                    let msg = self
                        .ctx
                        .i18n
                        .format("clash.toast.logs_copied", &[("lines", &count)]);
                    self.toast(msg, Tone::Success);
                    return Ok(Effect::Copy(text));
                }
            }
            KeyCode::Char('l') => {
                self.clash.tools_log_fullscreen = false;
                self.clash.tools_log_mode = false;
            }
            KeyCode::Char('f') => {
                self.clash.tools_log_fullscreen = !self.clash.tools_log_fullscreen;
            }
            KeyCode::Char('r') => {
                self.clash.log_loading = true;
                return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.clash.log_scroll = self.clash.log_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.clash.log_scroll = self.clash.log_scroll.saturating_add(1).min(max_s);
            }
            KeyCode::PageUp => {
                self.clash.log_scroll = self.clash.log_scroll.saturating_sub(15);
            }
            KeyCode::PageDown => {
                self.clash.log_scroll = self.clash.log_scroll.saturating_add(15).min(max_s);
            }
            KeyCode::Home => self.clash.log_scroll = 0,
            KeyCode::End => self.clash.log_scroll = max_s,
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_filter_key(&mut self, key: KeyEvent) -> Result<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.clash.filtering = false;
                self.clash.filter.clear();
                self.clash.clamp_cursors();
            }
            KeyCode::Enter => {
                self.clash.filtering = false;
            }
            KeyCode::Backspace => {
                self.clash.filter.backspace();
                self.clash.clamp_cursors();
            }
            KeyCode::Delete => {
                self.clash.filter.delete();
                self.clash.clamp_cursors();
            }
            KeyCode::Left => self.clash.filter.left(),
            KeyCode::Right => self.clash.filter.right(),
            KeyCode::Home => self.clash.filter.home(),
            KeyCode::End => self.clash.filter.end(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clash.filter.insert(c);
                self.clash.clamp_cursors();
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn clash_modal_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let groups_len = self.clash.visible_groups().len();
        let pp_len = self.clash.snapshot.proxy_providers.len();
        let rp_len = self.clash.snapshot.rule_providers.len();
        let mut selected_group_idx = None;
        let mut provider_cmd = None;
        let mut save_delay: Option<(String, u64)> = None;
        let mut close_modal = false;
        let mut run_confirm = false;
        let mut outbound_refresh = false;
        let mut outbound_action: Option<OutboundRowAction> = None;
        let mut service_check: Option<Vec<ServiceTarget>> = None;
        let mut service_copy_cursor: Option<usize> = None;
        let mut service_toggle: Option<ServiceTarget> = None;
        let mut service_move: Option<(usize, bool)> = None;
        let mut service_reset = false;
        let max_outbound_scroll = self.clash.max_outbound_modal_scroll.get();
        let ordered_services = self.clash.ordered_services();
        let enabled_services = self.clash.enabled_services();

        let Some(modal) = self.clash.modal.as_mut() else {
            return Ok(Effect::None);
        };
        match modal {
            ClashModal::DelaySettings {
                url,
                timeout,
                active_field,
                error,
            } => match key.code {
                KeyCode::Esc => {
                    close_modal = true;
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    *active_field = usize::from(*active_field == 0);
                }
                KeyCode::Enter => {
                    let u = url.text.trim().to_owned();
                    let t = timeout.text.trim().to_owned();
                    if let Err(err) = validate_test_url(&u) {
                        *error = Some(err.detail);
                        return Ok(Effect::None);
                    }
                    match validate_delay_timeout(&t) {
                        Ok(ms) => {
                            let effective = if u.is_empty() {
                                crate::ops::clash::controller::DEFAULT_DELAY_URL.to_owned()
                            } else {
                                u
                            };
                            save_delay = Some((effective, ms));
                            close_modal = true;
                        }
                        Err(err) => {
                            *error = Some(err.detail);
                        }
                    }
                }
                KeyCode::Backspace => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.backspace();
                    *error = None;
                }
                KeyCode::Delete => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.delete();
                    *error = None;
                }
                KeyCode::Left => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.left();
                }
                KeyCode::Right => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.right();
                }
                KeyCode::Home => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.home();
                }
                KeyCode::End => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.end();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let buf = if *active_field == 0 { url } else { timeout };
                    buf.insert(c);
                    *error = None;
                }
                _ => {}
            },
            ClashModal::GroupPicker { cursor } => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'g') => {
                    close_modal = true;
                }
                KeyCode::Up | KeyCode::Char('k') if groups_len > 0 => {
                    *cursor = cursor.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') if groups_len > 0 => {
                    *cursor = (*cursor + 1).min(groups_len - 1);
                }
                KeyCode::Enter if groups_len > 0 => {
                    selected_group_idx = Some(*cursor);
                    close_modal = true;
                }
                _ => {}
            },
            ClashModal::Providers { show_rules, cursor } => {
                let len = if *show_rules { rp_len } else { pp_len };
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q' | 'P') => {
                        close_modal = true;
                    }
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                        *show_rules = !*show_rules;
                        *cursor = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') if len > 0 => {
                        *cursor = cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                        *cursor = (*cursor + 1).min(len - 1);
                    }
                    KeyCode::Char('u') | KeyCode::Enter if len > 0 => {
                        provider_cmd = Some((*show_rules, *cursor, false));
                    }
                    KeyCode::Char('t') if !*show_rules && len > 0 => {
                        provider_cmd = Some((false, *cursor, true));
                    }
                    _ => {}
                }
            }
            ClashModal::OutboundIp { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'i' | 'I') => {
                    close_modal = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *scroll = scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *scroll = scroll.saturating_add(1).min(max_outbound_scroll);
                }
                KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(8);
                }
                KeyCode::PageDown => {
                    *scroll = scroll.saturating_add(8).min(max_outbound_scroll);
                }
                KeyCode::Home => {
                    *scroll = 0;
                }
                KeyCode::End => {
                    *scroll = max_outbound_scroll;
                }
                KeyCode::Char('r' | 'R' | 'N') => {
                    outbound_refresh = true;
                }
                KeyCode::Char('1' | 'c' | 'y') => {
                    outbound_action = Some(OutboundRowAction::CopyIp);
                }
                KeyCode::Char('2') => {
                    outbound_action = Some(OutboundRowAction::CopyOrg);
                }
                KeyCode::Char('3') => {
                    outbound_action = Some(OutboundRowAction::CopyAsn);
                }
                KeyCode::Char('h' | 'H' | ' ') => {
                    outbound_action = Some(OutboundRowAction::ToggleHideIp);
                }
                _ => {}
            },
            ClashModal::ServiceStatus { cursor, manage } => {
                let len = if *manage {
                    ordered_services.len()
                } else {
                    enabled_services.len()
                };
                if len > 0 && *cursor >= len {
                    *cursor = len - 1;
                }
                match key.code {
                    KeyCode::Esc => {
                        if *manage {
                            let focused = ordered_services.get(*cursor).copied();
                            *manage = false;
                            *cursor = focused
                                .and_then(|t| enabled_services.iter().position(|&x| x == t))
                                .unwrap_or(0);
                        } else {
                            close_modal = true;
                        }
                    }
                    KeyCode::Char('q' | 'Q') => {
                        close_modal = true;
                    }
                    KeyCode::Char('c' | 'C') if !*manage => {
                        close_modal = true;
                    }
                    KeyCode::Char('m' | 'M') | KeyCode::Tab => {
                        let focused = if *manage {
                            ordered_services.get(*cursor).copied()
                        } else {
                            enabled_services.get(*cursor).copied()
                        };
                        *manage = !*manage;
                        let next_list = if *manage {
                            &ordered_services
                        } else {
                            &enabled_services
                        };
                        *cursor = focused
                            .and_then(|t| next_list.iter().position(|&x| x == t))
                            .unwrap_or(0);
                    }
                    KeyCode::Up if *manage && key.modifiers.contains(KeyModifiers::ALT) => {
                        service_move = Some((*cursor, true));
                    }
                    KeyCode::Down if *manage && key.modifiers.contains(KeyModifiers::ALT) => {
                        service_move = Some((*cursor, false));
                    }
                    KeyCode::Char('K' | '[') if *manage => {
                        service_move = Some((*cursor, true));
                    }
                    KeyCode::Char('J' | ']') if *manage => {
                        service_move = Some((*cursor, false));
                    }
                    KeyCode::Up | KeyCode::Char('k') if len > 0 => {
                        *cursor = cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                        *cursor = (*cursor + 1).min(len - 1);
                    }
                    KeyCode::PageUp if len > 0 => {
                        *cursor = cursor.saturating_sub(6);
                    }
                    KeyCode::PageDown if len > 0 => {
                        *cursor = (*cursor + 6).min(len - 1);
                    }
                    KeyCode::Home => {
                        *cursor = 0;
                    }
                    KeyCode::End if len > 0 => {
                        *cursor = len - 1;
                    }
                    KeyCode::Char(' ') | KeyCode::Enter if *manage => {
                        if let Some(&target) = ordered_services.get(*cursor) {
                            service_toggle = Some(target);
                        }
                    }
                    KeyCode::Char('r' | 'R') if *manage => {
                        service_reset = true;
                    }
                    KeyCode::Enter | KeyCode::Char('r' | 'R') if !*manage => {
                        if let Some(&target) = enabled_services.get(*cursor) {
                            service_check = Some(vec![target]);
                        }
                    }
                    KeyCode::Char('a' | 'A' | 't') if !*manage => {
                        service_check = Some(enabled_services.clone());
                    }
                    KeyCode::Char('y' | 'Y') if !*manage => {
                        service_copy_cursor = Some(*cursor);
                    }
                    _ => {}
                }
            }
            ClashModal::Confirm { accept, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                    close_modal = true;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    *accept = !*accept;
                }
                KeyCode::Char('y' | 'Y') => {
                    run_confirm = true;
                }
                KeyCode::Enter => {
                    if *accept {
                        run_confirm = true;
                    } else {
                        close_modal = true;
                    }
                }
                _ => {}
            },
        }

        if run_confirm {
            return Ok(self.execute_clash_confirm());
        }
        if outbound_refresh {
            self.clash.exit_probing = true;
            self.clash.exit_probe_error = None;
            return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
        }
        if let Some(action) = outbound_action {
            return Ok(self.execute_outbound_row_action(action));
        }
        if let Some(targets) = service_check {
            return Ok(self.trigger_service_check(targets, true));
        }
        if let Some(cur) = service_copy_cursor {
            return Ok(self.copy_focused_service_ip(cur));
        }
        if let Some(target) = service_toggle {
            if self.clash.toggle_service_enabled(target) {
                return Ok(self.sync_service_prefs());
            }
            self.clash_toast_key("clash.toast.service_keep_one", Tone::Warning);
            return Ok(Effect::None);
        }
        if let Some((idx, up)) = service_move {
            if let Some(next_idx) = self.clash.move_service_order(idx, up) {
                if let Some(ClashModal::ServiceStatus { cursor, .. }) = self.clash.modal.as_mut() {
                    *cursor = next_idx;
                }
                return Ok(self.sync_service_prefs());
            }
            return Ok(Effect::None);
        }
        if service_reset {
            self.clash.reset_service_prefs();
            if let Some(ClashModal::ServiceStatus { cursor, .. }) = self.clash.modal.as_mut() {
                *cursor = 0;
            }
            self.clash_toast_key("clash.toast.service_prefs_reset", Tone::Info);
            return Ok(self.sync_service_prefs());
        }
        if let Some((u, ms)) = save_delay {
            self.clash.delay_url = u;
            self.clash.delay_timeout_ms = ms;
            self.clash_toast_key("clash.toast.delay_settings_saved", Tone::Success);
        }
        if let Some(idx) = selected_group_idx {
            self.clash.select_group_index(idx);
        }
        if close_modal {
            self.clash.modal = None;
        }
        if let Some((is_rules, idx, healthcheck)) = provider_cmd {
            if is_rules {
                if let Some(rp) = self.clash.snapshot.rule_providers.get(idx) {
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateRuleProvider {
                        name: rp.name.clone(),
                    }));
                }
            } else if let Some(pp) = self.clash.snapshot.proxy_providers.get(idx) {
                let name = pp.name.clone();
                if healthcheck {
                    return Ok(Effect::ClashCmd(
                        ClashMonitorCmd::HealthcheckProxyProvider { name },
                    ));
                }
                return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProxyProvider {
                    name,
                }));
            }
        }
        Ok(Effect::None)
    }

    fn execute_clash_confirm(&mut self) -> Effect {
        let Some(ClashModal::Confirm { action, .. }) = self.clash.modal.take() else {
            return Effect::None;
        };
        match action {
            ClashConfirmAction::RemoveProfile(name) => {
                Effect::ClashCmd(ClashMonitorCmd::RemoveProfile { name })
            }
            ClashConfirmAction::CloseAllConnections => {
                Effect::ClashCmd(ClashMonitorCmd::CloseAllConnections)
            }
        }
    }

    pub(in crate::tui) fn clash_modal_mouse(
        &mut self,
        event: MouseEvent,
        area: Rect,
    ) -> Result<Effect> {
        let pos = Position::new(event.column, event.row);
        if matches!(event.kind, MouseEventKind::Moved) {
            self.hover = Some(pos);
            return Ok(Effect::None);
        }
        if matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let up = matches!(event.kind, MouseEventKind::ScrollUp);
            let groups_len = self.clash.visible_groups().len();
            let pp_len = self.clash.snapshot.proxy_providers.len();
            let rp_len = self.clash.snapshot.rule_providers.len();
            let enabled_services_len = self.clash.enabled_services().len();
            match self.clash.modal.as_mut() {
                Some(ClashModal::GroupPicker { cursor }) if groups_len > 0 => {
                    *cursor = if up {
                        cursor.saturating_sub(1)
                    } else {
                        (*cursor + 1).min(groups_len - 1)
                    };
                }
                Some(ClashModal::Providers { show_rules, cursor }) => {
                    let len = if *show_rules { rp_len } else { pp_len };
                    if len > 0 {
                        *cursor = if up {
                            cursor.saturating_sub(1)
                        } else {
                            (*cursor + 1).min(len - 1)
                        };
                    }
                }
                Some(ClashModal::OutboundIp { scroll }) => {
                    let max_s = self.clash.max_outbound_modal_scroll.get();
                    *scroll = if up {
                        scroll.saturating_sub(2)
                    } else {
                        scroll.saturating_add(2).min(max_s)
                    };
                }
                Some(ClashModal::ServiceStatus { cursor, manage }) => {
                    let len = if *manage {
                        ServiceTarget::ALL.len()
                    } else {
                        enabled_services_len
                    };
                    if len > 0 {
                        *cursor = if up {
                            cursor.saturating_sub(1)
                        } else {
                            (*cursor + 1).min(len - 1)
                        };
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(Effect::None);
        }
        let Some(modal) = self.clash.modal.clone() else {
            return Ok(Effect::None);
        };
        let geom = ClashModalGeometry::compute(area, &modal, &self.clash, &self.ctx.i18n);
        if !geom.contains(pos) {
            self.clash.modal = None;
            return Ok(Effect::None);
        }
        match modal {
            ClashModal::GroupPicker { .. } => {
                for (rect, idx) in geom.item_rows {
                    if rect.contains(pos) {
                        self.clash.select_group_index(idx);
                        self.clash.modal = None;
                        break;
                    }
                }
            }
            ClashModal::Providers { show_rules, .. } => {
                if geom.subtab_proxy.is_some_and(|r| r.contains(pos)) {
                    if let Some(ClashModal::Providers {
                        show_rules: sr,
                        cursor,
                    }) = self.clash.modal.as_mut()
                    {
                        *sr = false;
                        *cursor = 0;
                    }
                    return Ok(Effect::None);
                }
                if geom.subtab_rules.is_some_and(|r| r.contains(pos)) {
                    if let Some(ClashModal::Providers {
                        show_rules: sr,
                        cursor,
                    }) = self.clash.modal.as_mut()
                    {
                        *sr = true;
                        *cursor = 0;
                    }
                    return Ok(Effect::None);
                }
                for (rect, idx) in geom.item_rows {
                    if rect.contains(pos) {
                        if let Some(ClashModal::Providers { cursor, .. }) =
                            self.clash.modal.as_mut()
                        {
                            *cursor = idx;
                        }
                        if show_rules {
                            if let Some(rp) = self.clash.snapshot.rule_providers.get(idx) {
                                return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateRuleProvider {
                                    name: rp.name.clone(),
                                }));
                            }
                        } else if let Some(pp) = self.clash.snapshot.proxy_providers.get(idx) {
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProxyProvider {
                                name: pp.name.clone(),
                            }));
                        }
                    }
                }
            }
            ClashModal::OutboundIp { .. } => {
                if geom.outbound_close_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = None;
                    return Ok(Effect::None);
                }
                if geom.outbound_refresh_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.exit_probing = true;
                    self.clash.exit_probe_error = None;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
                }
                if geom.outbound_copy_btn.is_some_and(|r| r.contains(pos)) {
                    return Ok(self.execute_outbound_row_action(OutboundRowAction::CopyIp));
                }
                if geom.outbound_hide_btn.is_some_and(|r| r.contains(pos)) {
                    return Ok(self.execute_outbound_row_action(OutboundRowAction::ToggleHideIp));
                }
                for (rect, action) in geom.outbound_row_hits {
                    if rect.contains(pos) {
                        return Ok(self.execute_outbound_row_action(action));
                    }
                }
            }
            ClashModal::ServiceStatus { cursor, manage } => {
                if geom.service_close_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = None;
                    return Ok(Effect::None);
                }
                if geom.service_manage_btn.is_some_and(|r| r.contains(pos)) {
                    let ordered = self.clash.ordered_services();
                    let enabled = self.clash.enabled_services();
                    let focused = if manage {
                        ordered.get(cursor).copied()
                    } else {
                        enabled.get(cursor).copied()
                    };
                    let next_manage = !manage;
                    let next_list = if next_manage { &ordered } else { &enabled };
                    let next_cur = focused
                        .and_then(|t| next_list.iter().position(|&x| x == t))
                        .unwrap_or(0);
                    self.clash.modal = Some(ClashModal::ServiceStatus {
                        cursor: next_cur,
                        manage: next_manage,
                    });
                    return Ok(Effect::None);
                }
                if geom.service_check_btn.is_some_and(|r| r.contains(pos)) {
                    if manage {
                        let ordered = self.clash.ordered_services();
                        if let Some(&target) = ordered.get(cursor) {
                            if self.clash.toggle_service_enabled(target) {
                                return Ok(self.sync_service_prefs());
                            }
                            self.clash_toast_key("clash.toast.service_keep_one", Tone::Warning);
                        }
                        return Ok(Effect::None);
                    }
                    let enabled = self.clash.enabled_services();
                    if let Some(&target) = enabled.get(cursor) {
                        return Ok(self.trigger_service_check(vec![target], true));
                    }
                    return Ok(Effect::None);
                }
                if geom.service_check_all_btn.is_some_and(|r| r.contains(pos)) {
                    if manage {
                        self.clash.reset_service_prefs();
                        self.clash.modal = Some(ClashModal::ServiceStatus {
                            cursor: 0,
                            manage: true,
                        });
                        self.clash_toast_key("clash.toast.service_prefs_reset", Tone::Info);
                        return Ok(self.sync_service_prefs());
                    }
                    let enabled = self.clash.enabled_services();
                    return Ok(self.trigger_service_check(enabled, true));
                }
                if !manage && geom.service_copy_ip_btn.is_some_and(|r| r.contains(pos)) {
                    return Ok(self.copy_focused_service_ip(cursor));
                }
                for (rect, idx) in geom.item_rows {
                    if rect.contains(pos) {
                        if let Some(ClashModal::ServiceStatus { cursor: c, .. }) =
                            self.clash.modal.as_mut()
                        {
                            *c = idx;
                        }
                        if manage {
                            let ordered = self.clash.ordered_services();
                            if let Some(&target) = ordered.get(idx) {
                                if self.clash.toggle_service_enabled(target) {
                                    return Ok(self.sync_service_prefs());
                                }
                                self.clash_toast_key("clash.toast.service_keep_one", Tone::Warning);
                            }
                            return Ok(Effect::None);
                        }
                        let enabled = self.clash.enabled_services();
                        if let Some(&target) = enabled.get(idx) {
                            return Ok(self.trigger_service_check(vec![target], true));
                        }
                        return Ok(Effect::None);
                    }
                }
            }
            ClashModal::Confirm { .. } => {
                if geom.confirm_yes.is_some_and(|r| r.contains(pos)) {
                    return Ok(self.execute_clash_confirm());
                }
                if geom.confirm_no.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = None;
                }
            }
            ClashModal::DelaySettings { .. } => {
                for (idx, rect) in geom.field_rects.iter().enumerate() {
                    if rect.contains(pos)
                        && let Some(ClashModal::DelaySettings { active_field, .. }) =
                            self.clash.modal.as_mut()
                    {
                        *active_field = idx;
                        break;
                    }
                }
            }
        }
        Ok(Effect::None)
    }

    pub(in crate::tui) fn clash_mouse(
        &mut self,
        event: MouseEvent,
        regions: &LayoutRegions,
    ) -> Result<Effect> {
        let Some(geom) = &regions.clash else {
            return Ok(Effect::None);
        };
        let pos = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Moved => {
                self.hover = Some(pos);
                return Ok(Effect::None);
            }
            MouseEventKind::ScrollUp => {
                return Ok(self.clash_scroll(true));
            }
            MouseEventKind::ScrollDown => {
                return Ok(self.clash_scroll(false));
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return Ok(Effect::None),
        }

        // Top tab bar & focus button
        for pill in &geom.tab_pills {
            if pill.rect.contains(pos) {
                return Ok(self.switch_clash_tab(pill.tab));
            }
        }
        if geom.focus_btn.is_some_and(|r| r.contains(pos)) {
            self.clash.fullscreen = !self.clash.fullscreen;
            return Ok(Effect::None);
        }

        let idle = self.worker.is_none();
        match &geom.body {
            ClashBodyGeom::Unavailable(unavail) => {
                if idle && unavail.cta.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_unavailable_cta();
                }
                if idle
                    && self.clash.busy_hint.is_none()
                    && unavail.secondary_cta.is_some_and(|r| r.contains(pos))
                {
                    return self.clash_trigger_action("clash.controller.configure");
                }
            }
            ClashBodyGeom::TrafficFullscreen(tf) => {
                if tf.close_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_fullscreen = false;
                    return Ok(Effect::None);
                }
                if tf.mode_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_mode = self.clash.traffic_mode.next();
                    return Ok(Effect::None);
                }
                if tf.window_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_window = self.clash.traffic_window.next();
                    return Ok(Effect::None);
                }
                if tf.scale_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_scale = self.clash.traffic_scale.toggle();
                    return Ok(Effect::None);
                }
                if tf.pause_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.toggle_traffic_pause();
                    return Ok(Effect::None);
                }
                if tf.breakdown_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_breakdown = self.clash.traffic_breakdown.next();
                    return Ok(Effect::None);
                }
            }
            ClashBodyGeom::Overview(ov) => {
                if ov.traffic_mode_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_mode = self.clash.traffic_mode.next();
                    return Ok(Effect::None);
                }
                if ov.traffic_window_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_window = self.clash.traffic_window.next();
                    return Ok(Effect::None);
                }
                if ov.traffic_scale_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.traffic_scale = self.clash.traffic_scale.toggle();
                    return Ok(Effect::None);
                }
                if ov.traffic_pause_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.toggle_traffic_pause();
                    return Ok(Effect::None);
                }
                if ov.traffic_expand_btn.is_some_and(|r| r.contains(pos))
                    || ov.traffic_plot.contains(pos)
                    || ov.traffic_side.contains(pos)
                {
                    self.clash.traffic_fullscreen = true;
                    return Ok(Effect::None);
                }
                if ov.outbound_badge_hit.is_some_and(|r| r.contains(pos)) {
                    self.clash.exit_probing = true;
                    self.clash.exit_probe_error = None;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
                }
                if ov.outbound_ip_hit.is_some_and(|r| r.contains(pos))
                    || ov.outbound_detail_btn.is_some_and(|r| r.contains(pos))
                {
                    return Ok(self.open_outbound_ip_modal());
                }
                if idle && ov.start_stop_btn.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_action("clash.stop");
                }
                if idle && ov.reload_btn.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_action("clash.reload");
                }
                for &(rect, mode) in &ov.mode_pills {
                    if rect.contains(pos) {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                            mode: mode.to_owned(),
                        }));
                    }
                }
                if ov.profile_switch_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.set_tab(ClashTab::Profiles);
                    return Ok(Effect::None);
                }
                if ov.profile_update_btn.is_some_and(|r| r.contains(pos))
                    && let Some(prof) = self.clash.snapshot.active_profile()
                    && !prof.is_local
                {
                    let name = prof.name.clone();
                    if !self.clash.updating_all_profiles
                        && self.clash.busy_profiles.insert(name.clone())
                    {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { name }));
                    }
                }
                if ov.service_check_btn.is_some_and(|r| r.contains(pos)) {
                    return Ok(self.open_service_status_modal(None));
                }
                for &(rect, target) in &ov.service_cell_hits {
                    if rect.contains(pos) {
                        return Ok(self.open_service_status_modal(Some(target)));
                    }
                }
                if ov.socks_secret_btn.is_some_and(|r| r.contains(pos)) {
                    return self.open_socks_secret_modal();
                }
                if ov.webui_btn.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_webui(idle);
                }
                if ov.exit_probe_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.exit_probing = true;
                    self.clash.exit_probe_error = None;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
                }
                for &(rect, g_idx) in &ov.group_card_hits {
                    if rect.contains(pos) {
                        self.clash.overview_group_cursor = g_idx;
                        self.clash.set_tab(ClashTab::Proxies);
                        self.clash.select_group_index(g_idx);
                        return Ok(Effect::None);
                    }
                }
                if ov.connections_card.contains(pos) {
                    self.clash.set_tab(ClashTab::Connections);
                    return Ok(Effect::None);
                }
                if ov.profile_card.contains(pos) {
                    self.clash.set_tab(ClashTab::Profiles);
                    return Ok(Effect::None);
                }
            }
            ClashBodyGeom::Proxies(px) => {
                for pill in &px.group_pills {
                    if pill.rect.contains(pos) {
                        self.clash.select_group_index(pill.group_idx);
                        return Ok(Effect::None);
                    }
                }
                if px.group_picker_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = Some(ClashModal::GroupPicker {
                        cursor: self.clash.group_cursor,
                    });
                    return Ok(Effect::None);
                }
                if px.test_btn.is_some_and(|r| r.contains(pos))
                    && let Some(group) = self.clash.current_group()
                {
                    let g = group.name.clone();
                    if self.clash.testing_groups.insert(g.clone()) {
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::TestGroupDelay {
                            group: g,
                            url: self.clash.delay_url.clone(),
                            timeout_ms: self.clash.delay_timeout_ms,
                        }));
                    }
                }
                if px.sort_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.proxy_sort = self.clash.proxy_sort.next();
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if px.hide_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.hide_timeout = !self.clash.hide_timeout;
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if px.providers_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = Some(ClashModal::Providers {
                        show_rules: false,
                        cursor: 0,
                    });
                    return Ok(Effect::None);
                }
                for &(rect, idx) in &px.node_hits {
                    if rect.contains(pos) {
                        let already_focused = self.clash.node_cursor == idx;
                        self.clash.node_cursor = idx;
                        if already_focused
                            && let (Some(group), Some(node)) =
                                (self.clash.current_group(), self.clash.selected_node())
                            && group.kind.supports_select()
                        {
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::SelectProxy {
                                group: group.name.clone(),
                                node: node.name.clone(),
                            }));
                        }
                        return Ok(Effect::None);
                    }
                }
            }
            ClashBodyGeom::Profiles(pf) => {
                if pf.update_all_btn.is_some_and(|r| r.contains(pos))
                    && !self.clash.updating_all_profiles
                    && self.clash.busy_profiles.is_empty()
                {
                    self.clash.updating_all_profiles = true;
                    return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateAllProfiles));
                }
                if idle && pf.add_btn.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_action("clash.sub.add");
                }
                if idle && pf.import_btn.is_some_and(|r| r.contains(pos)) {
                    return self.clash_trigger_action("clash.sub.import");
                }
                if pf.providers_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = Some(ClashModal::Providers {
                        show_rules: false,
                        cursor: 0,
                    });
                    return Ok(Effect::None);
                }
                for &(rect, idx) in &pf.item_hits {
                    if rect.contains(pos) {
                        let already = self.clash.profile_cursor == idx;
                        self.clash.profile_cursor = idx;
                        if already && let Some(prof) = self.clash.selected_profile() {
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::SwitchProfile {
                                name: prof.name.clone(),
                            }));
                        }
                        return Ok(Effect::None);
                    }
                }
            }
            ClashBodyGeom::Connections(cn) => {
                if cn.scope_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.conn_scope = self.clash.conn_scope.next();
                    self.clash.conn_facet = None;
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if cn.sort_chip.is_some_and(|r| r.contains(pos)) {
                    self.clash.conn_sort = self.clash.conn_sort.next();
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if cn.close_all_btn.is_some_and(|r| r.contains(pos))
                    && !self.clash.snapshot.connections.is_empty()
                {
                    self.clash.modal = Some(ClashModal::Confirm {
                        title: self.ctx.i18n.text("clash.confirm.title").to_owned(),
                        message: self
                            .ctx
                            .i18n
                            .text("clash.confirm.close_all_conns")
                            .to_owned(),
                        danger: true,
                        accept: false,
                        action: ClashConfirmAction::CloseAllConnections,
                    });
                    return Ok(Effect::None);
                }
                for (rect, facet) in &cn.facet_chips {
                    if rect.contains(pos) {
                        self.clash.conn_facet = facet.clone();
                        self.clash.clamp_cursors();
                        return Ok(Effect::None);
                    }
                }
                if cn.list_inner.contains(pos) {
                    let row = pos.y.saturating_sub(cn.list_inner.y) as usize;
                    let idx = cn.list_scroll + row;
                    if idx < self.clash.filtered_connections().len() {
                        self.clash.conn_cursor = idx;
                        self.clash.conn_inspector_scroll = 0;
                    }
                }
            }
            ClashBodyGeom::Rules(rl) => {
                if rl.providers_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.modal = Some(ClashModal::Providers {
                        show_rules: true,
                        cursor: 0,
                    });
                    return Ok(Effect::None);
                }
                for (rect, facet) in &rl.facet_chips {
                    if rect.contains(pos) {
                        self.clash.rule_facet = facet.clone();
                        self.clash.clamp_cursors();
                        return Ok(Effect::None);
                    }
                }
                if rl.list_inner.contains(pos) {
                    let row = pos.y.saturating_sub(rl.list_inner.y) as usize;
                    let idx = rl.list_scroll + row;
                    if idx < self.clash.filtered_rules().len() {
                        self.clash.rule_cursor = idx;
                    }
                }
            }
            ClashBodyGeom::Tools(tl) => {
                if tl.logs_btn.is_some_and(|r| r.contains(pos)) {
                    self.clash.tools_log_mode = !self.clash.tools_log_mode;
                    if self.clash.tools_log_mode {
                        self.clash.log_loading = true;
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
                    }
                    return Ok(Effect::None);
                }
                if !self.clash.tools_log_mode && tl.list_inner.contains(pos) {
                    let row = pos.y.saturating_sub(tl.list_inner.y) as usize;
                    let idx = tl.list_scroll + row;
                    let tools = self.clash.filtered_tools(&self.ctx.i18n);
                    if let Some(item) = tools.get(idx) {
                        let already = self.clash.tool_cursor == idx;
                        self.clash.tool_cursor = idx;
                        if already && idle {
                            return self.execute_clash_tool_entry(item.entry);
                        }
                    }
                }
            }
            ClashBodyGeom::Fullscreen(_) => {}
        }
        Ok(Effect::None)
    }

    fn clash_scroll(&mut self, up: bool) -> Effect {
        match self.clash.tab {
            ClashTab::Proxies => {
                let cols = self.clash.last_grid_cols.get().max(1);
                let len = self.clash.filtered_nodes().len();
                if len > 0 {
                    if up {
                        self.clash.node_cursor = self.clash.node_cursor.saturating_sub(cols);
                    } else {
                        self.clash.node_cursor = (self.clash.node_cursor + cols).min(len - 1);
                    }
                }
            }
            ClashTab::Profiles => {
                let len = self.clash.filtered_profiles().len();
                if len > 0 {
                    if up {
                        self.clash.profile_cursor = self.clash.profile_cursor.saturating_sub(1);
                    } else {
                        self.clash.profile_cursor = (self.clash.profile_cursor + 1).min(len - 1);
                    }
                }
            }
            ClashTab::Connections => {
                if self.clash.conn_inspector_fullscreen {
                    let max_s = self.clash.max_conn_inspector_scroll.get();
                    if up {
                        self.clash.conn_inspector_scroll =
                            self.clash.conn_inspector_scroll.saturating_sub(3);
                    } else {
                        self.clash.conn_inspector_scroll = self
                            .clash
                            .conn_inspector_scroll
                            .saturating_add(3)
                            .min(max_s);
                    }
                } else {
                    let len = self.clash.filtered_connections().len();
                    if len > 0 {
                        if up {
                            self.clash.conn_cursor = self.clash.conn_cursor.saturating_sub(3);
                        } else {
                            self.clash.conn_cursor = (self.clash.conn_cursor + 3).min(len - 1);
                        }
                    }
                }
            }
            ClashTab::Rules => {
                let len = self.clash.filtered_rules().len();
                if len > 0 {
                    if up {
                        self.clash.rule_cursor = self.clash.rule_cursor.saturating_sub(3);
                    } else {
                        self.clash.rule_cursor = (self.clash.rule_cursor + 3).min(len - 1);
                    }
                }
            }
            ClashTab::Tools => {
                if self.clash.tools_log_mode || self.clash.tools_log_fullscreen {
                    let max_s = self.clash.max_log_scroll.get();
                    if up {
                        self.clash.log_scroll = self.clash.log_scroll.saturating_sub(3);
                    } else {
                        self.clash.log_scroll = self.clash.log_scroll.saturating_add(3).min(max_s);
                    }
                } else {
                    let len = self.clash.filtered_tools(&self.ctx.i18n).len();
                    if len > 0 {
                        if up {
                            self.clash.tool_cursor = self.clash.tool_cursor.saturating_sub(1);
                        } else {
                            self.clash.tool_cursor = (self.clash.tool_cursor + 1).min(len - 1);
                        }
                    }
                }
            }
            ClashTab::Overview => {}
        }
        Effect::None
    }
}
