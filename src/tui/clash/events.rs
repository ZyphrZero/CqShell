//! Keyboard and mouse event routing for the dedicated Mihomo visual management workspace.

use super::{
    layout::{ClashBodyGeom, ClashModalGeometry, OutboundRowAction, proxy_card_delay_hit},
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
        layout::{LayoutRegions, ThumbAction, hit_best, hit_opt, hit_rect},
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

            // Select node (Selector / URLTest / Fallback); re-selecting an already-pinned
            // node on an automatic group unfixes it back to automatic mode.
            KeyCode::Enter => {
                return Ok(self.clash_select_current_node());
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
                return Ok(self.clash_ping_current_node());
            }

            // Explicit one-click fastest reachable node selection
            KeyCode::Char('f') => {
                return Ok(self.clash_select_fastest_node());
            }

            // Unfix pinned URLTest / Fallback group back to automatic
            KeyCode::Char('x') => {
                return Ok(self.clash_unfix_current_group());
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

    fn clash_select_current_node(&mut self) -> Effect {
        let Some((g, n, supports_select, is_already_pinned)) =
            self.clash.current_group().and_then(|group| {
                self.clash.selected_node().map(|node| {
                    (
                        group.name.clone(),
                        node.name.clone(),
                        group.kind.supports_select(),
                        group.kind.supports_unfix()
                            && group.fixed.as_deref() == Some(node.name.as_str()),
                    )
                })
            })
        else {
            return Effect::None;
        };
        if !supports_select {
            self.clash_toast_key("clash.toast.group_readonly", Tone::Warning);
            return Effect::None;
        }
        if is_already_pinned {
            self.clash.apply_optimistic_unfix(&g);
            return Effect::ClashCmd(ClashMonitorCmd::UnfixGroup { group: g });
        }
        self.clash.apply_optimistic_select(&g, &n);
        Effect::ClashCmd(ClashMonitorCmd::SelectProxy { group: g, node: n })
    }

    fn clash_ping_current_node(&mut self) -> Effect {
        if let (Some(group), Some(node)) = (self.clash.current_group(), self.clash.selected_node())
        {
            let g = group.name.clone();
            let n = node.name.clone();
            if self.clash.testing_nodes.insert(n.clone()) {
                return Effect::ClashCmd(ClashMonitorCmd::TestNodeDelay {
                    group: g,
                    node: n,
                    url: self.clash.delay_url.clone(),
                    timeout_ms: self.clash.delay_timeout_ms,
                });
            }
        }
        Effect::None
    }

    fn clash_select_fastest_node(&mut self) -> Effect {
        if let Some((g, n, _ms)) = self.clash.fastest_node_in_current_group() {
            self.clash.apply_optimistic_select(&g, &n);
            return Effect::ClashCmd(ClashMonitorCmd::SelectProxy { group: g, node: n });
        }
        self.clash_toast_key("clash.toast.no_tested_nodes", Tone::Warning);
        Effect::None
    }

    fn clash_unfix_current_group(&mut self) -> Effect {
        if let Some(group) = self.clash.current_group() {
            if group.kind.supports_unfix() {
                let g = group.name.clone();
                self.clash.apply_optimistic_unfix(&g);
                return Effect::ClashCmd(ClashMonitorCmd::UnfixGroup { group: g });
            }
            self.clash_toast_key("clash.toast.unfix_auto_only", Tone::Info);
        }
        Effect::None
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

    pub(in crate::tui) fn clash_thumb_action(&mut self, action: ThumbAction) -> Result<Effect> {
        let idle = self.worker.is_none();
        match action {
            ThumbAction::ClashNextTab => Ok(self.cycle_clash_tab(true)),
            ThumbAction::ClashCycleMode => {
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
                Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                    mode: next.to_owned(),
                }))
            }
            ThumbAction::ClashPrimary => match self.clash.tab {
                ClashTab::Overview => self.clash_thumb_action(ThumbAction::ClashCycleMode),
                ClashTab::Proxies => {
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
                    Ok(Effect::None)
                }
                ClashTab::Profiles => {
                    if let Some(prof) = self.clash.selected_profile()
                        && !prof.is_local
                    {
                        let name = prof.name.clone();
                        if !self.clash.updating_all_profiles
                            && self.clash.busy_profiles.insert(name.clone())
                        {
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProfile { name }));
                        }
                        return Ok(Effect::None);
                    }
                    if !self.clash.updating_all_profiles && self.clash.busy_profiles.is_empty() {
                        self.clash.updating_all_profiles = true;
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateAllProfiles));
                    }
                    Ok(Effect::None)
                }
                ClashTab::Connections | ClashTab::Rules => Ok(Effect::ClashRefresh),
                ClashTab::Tools => {
                    self.clash.tools_log_mode = !self.clash.tools_log_mode;
                    if self.clash.tools_log_mode {
                        self.clash.log_scroll = 0;
                        self.clash.log_loading = true;
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
                    }
                    Ok(Effect::None)
                }
            },
            ThumbAction::ClashSecondary => match self.clash.tab {
                ClashTab::Overview => Ok(self.open_service_status_modal(None)),
                ClashTab::Proxies => Ok(self.clash_select_fastest_node()),
                ClashTab::Profiles if idle => self.clash_trigger_action("clash.sub.add"),
                ClashTab::Connections if !self.clash.snapshot.connections.is_empty() => {
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
                    Ok(Effect::None)
                }
                ClashTab::Rules => {
                    self.clash.filtering = true;
                    Ok(Effect::None)
                }
                ClashTab::Tools => Ok(Effect::ClashRefresh),
                _ => Ok(Effect::None),
            },
            _ => Ok(Effect::None),
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
        let density = self.resolved_density(area);
        let slop = density.slop_enabled();
        let geom = ClashModalGeometry::compute(area, &modal, &self.clash, &self.ctx.i18n, density);
        if !geom.contains(pos) {
            self.clash.modal = None;
            return Ok(Effect::None);
        }
        match modal {
            ClashModal::GroupPicker { .. } => {
                if let Some(idx) = hit_best(geom.item_rows.iter().copied(), pos, slop) {
                    self.clash.select_group_index(idx);
                    self.clash.modal = None;
                }
            }
            ClashModal::Providers { show_rules, .. } => {
                let mut tab_hits = Vec::new();
                if let Some(r) = geom.subtab_proxy {
                    tab_hits.push((r, false));
                }
                if let Some(r) = geom.subtab_rules {
                    tab_hits.push((r, true));
                }
                if let Some(next_rules) = hit_best(tab_hits, pos, slop) {
                    if let Some(ClashModal::Providers {
                        show_rules: sr,
                        cursor,
                    }) = self.clash.modal.as_mut()
                    {
                        *sr = next_rules;
                        *cursor = 0;
                    }
                    return Ok(Effect::None);
                }
                if let Some(idx) = hit_best(geom.item_rows.iter().copied(), pos, slop) {
                    if let Some(ClashModal::Providers { cursor, .. }) = self.clash.modal.as_mut() {
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
            ClashModal::OutboundIp { .. } => {
                let mut btns = Vec::new();
                if let Some(r) = geom.outbound_close_btn {
                    btns.push((r, 0u8));
                }
                if let Some(r) = geom.outbound_refresh_btn {
                    btns.push((r, 1u8));
                }
                if let Some(r) = geom.outbound_copy_btn {
                    btns.push((r, 2u8));
                }
                if let Some(r) = geom.outbound_hide_btn {
                    btns.push((r, 3u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        self.clash.modal = None;
                        return Ok(Effect::None);
                    }
                    Some(1) => {
                        self.clash.exit_probing = true;
                        self.clash.exit_probe_error = None;
                        return Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit));
                    }
                    Some(2) => {
                        return Ok(self.execute_outbound_row_action(OutboundRowAction::CopyIp));
                    }
                    Some(3) => {
                        return Ok(
                            self.execute_outbound_row_action(OutboundRowAction::ToggleHideIp)
                        );
                    }
                    _ => {}
                }
                if let Some(action) = hit_best(geom.outbound_row_hits.iter().copied(), pos, false) {
                    return Ok(self.execute_outbound_row_action(action));
                }
            }
            ClashModal::ServiceStatus { cursor, manage } => {
                let mut btns = Vec::new();
                if let Some(r) = geom.service_close_btn {
                    btns.push((r, 0u8));
                }
                if let Some(r) = geom.service_manage_btn {
                    btns.push((r, 1u8));
                }
                if let Some(r) = geom.service_check_btn {
                    btns.push((r, 2u8));
                }
                if let Some(r) = geom.service_check_all_btn {
                    btns.push((r, 3u8));
                }
                if !manage && let Some(r) = geom.service_copy_ip_btn {
                    btns.push((r, 4u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        self.clash.modal = None;
                        return Ok(Effect::None);
                    }
                    Some(1) => {
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
                    Some(2) => {
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
                    Some(3) => {
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
                    Some(4) => {
                        return Ok(self.copy_focused_service_ip(cursor));
                    }
                    _ => {}
                }
                if let Some(idx) = hit_best(geom.item_rows.iter().copied(), pos, slop) {
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
            ClashModal::Confirm { .. } => {
                let mut btns = Vec::new();
                if let Some(r) = geom.confirm_yes {
                    btns.push((r, true));
                }
                if let Some(r) = geom.confirm_no {
                    btns.push((r, false));
                }
                match hit_best(btns, pos, slop) {
                    Some(true) => return Ok(self.execute_clash_confirm()),
                    Some(false) => {
                        self.clash.modal = None;
                    }
                    None => {}
                }
            }
            ClashModal::DelaySettings { .. } => {
                if let Some(idx) = hit_best(
                    geom.field_rects
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(i, r)| (r, i)),
                    pos,
                    slop,
                ) && let Some(ClashModal::DelaySettings { active_field, .. }) =
                    self.clash.modal.as_mut()
                {
                    *active_field = idx;
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
        let slop = regions.density.slop_enabled();
        let pos = Position::new(event.column, event.row);
        let right_click = match event.kind {
            MouseEventKind::Moved => {
                self.hover = Some(pos);
                return Ok(Effect::None);
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = matches!(event.kind, MouseEventKind::ScrollUp);
                if let ClashBodyGeom::Proxies(px) = &geom.body
                    && px.group_bar.contains(pos)
                {
                    self.clash.cycle_group(!up);
                    return Ok(Effect::None);
                }
                if let ClashBodyGeom::Overview(ov) = &geom.body {
                    if ov.overview_max_scroll > 0 {
                        if up {
                            self.clash.overview_scroll =
                                self.clash.overview_scroll.saturating_sub(1);
                        } else {
                            self.clash.overview_scroll =
                                (self.clash.overview_scroll + 1).min(ov.overview_max_scroll);
                        }
                    } else if ov.groups_card.contains(pos) || ov.group_can_prev || ov.group_can_next
                    {
                        if up {
                            self.clash.overview_group_page =
                                self.clash.overview_group_page.saturating_sub(1);
                        } else if ov.group_can_next {
                            self.clash.overview_group_page =
                                self.clash.overview_group_page.saturating_add(1);
                        }
                    }
                    return Ok(Effect::None);
                }
                return Ok(self.clash_scroll(up));
            }
            MouseEventKind::Down(MouseButton::Left) => false,
            MouseEventKind::Down(MouseButton::Right) => true,
            _ => return Ok(Effect::None),
        };

        // Top tab bar & focus button (left-click only)
        if !right_click {
            #[derive(Clone, Copy)]
            enum TopHit {
                Prev,
                Next,
                Tab(ClashTab),
                Focus,
            }
            let mut top_hits: Vec<(Rect, TopHit)> = Vec::new();
            if let Some(r) = geom.tab_prev_hit {
                top_hits.push((r, TopHit::Prev));
            }
            if let Some(r) = geom.tab_next_hit {
                top_hits.push((r, TopHit::Next));
            }
            for pill in &geom.tab_pills {
                top_hits.push((pill.rect, TopHit::Tab(pill.tab)));
            }
            if let Some(r) = geom.focus_btn {
                top_hits.push((r, TopHit::Focus));
            }
            if let Some(hit) = hit_best(top_hits, pos, slop) {
                return match hit {
                    TopHit::Prev => Ok(self.cycle_clash_tab(false)),
                    TopHit::Next => Ok(self.cycle_clash_tab(true)),
                    TopHit::Tab(tab) => Ok(self.switch_clash_tab(tab)),
                    TopHit::Focus => {
                        self.clash.fullscreen = !self.clash.fullscreen;
                        Ok(Effect::None)
                    }
                };
            }
        } else if !matches!(geom.body, ClashBodyGeom::Proxies(_)) {
            return Ok(Effect::None);
        }

        let idle = self.worker.is_none();
        match &geom.body {
            ClashBodyGeom::Unavailable(unavail) => {
                let mut hits = Vec::new();
                if idle && let Some(r) = unavail.cta {
                    hits.push((r, 0u8));
                }
                if idle
                    && self.clash.busy_hint.is_none()
                    && let Some(r) = unavail.secondary_cta
                {
                    hits.push((r, 1u8));
                }
                match hit_best(hits, pos, slop) {
                    Some(0) => return self.clash_trigger_unavailable_cta(),
                    Some(1) => return self.clash_trigger_action("clash.controller.configure"),
                    _ => {}
                }
            }
            ClashBodyGeom::TrafficFullscreen(tf) => {
                let mut hits = Vec::new();
                if let Some(r) = tf.close_btn {
                    hits.push((r, 0u8));
                }
                if let Some(r) = tf.mode_chip {
                    hits.push((r, 1u8));
                }
                if let Some(r) = tf.window_chip {
                    hits.push((r, 2u8));
                }
                if let Some(r) = tf.scale_chip {
                    hits.push((r, 3u8));
                }
                if let Some(r) = tf.pause_chip {
                    hits.push((r, 4u8));
                }
                if let Some(r) = tf.breakdown_chip {
                    hits.push((r, 5u8));
                }
                match hit_best(hits, pos, slop) {
                    Some(0) => self.clash.traffic_fullscreen = false,
                    Some(1) => self.clash.traffic_mode = self.clash.traffic_mode.next(),
                    Some(2) => self.clash.traffic_window = self.clash.traffic_window.next(),
                    Some(3) => self.clash.traffic_scale = self.clash.traffic_scale.toggle(),
                    Some(4) => self.clash.toggle_traffic_pause(),
                    Some(5) => self.clash.traffic_breakdown = self.clash.traffic_breakdown.next(),
                    _ => {}
                }
            }
            ClashBodyGeom::Overview(ov) => {
                #[derive(Clone, Copy)]
                enum OvBtn {
                    DeckPrev,
                    DeckNext,
                    GroupPrev,
                    GroupNext,
                    TrafficMode,
                    TrafficWindow,
                    TrafficScale,
                    TrafficPause,
                    TrafficExpand,
                    OutboundBadge,
                    OutboundDetail,
                    StartStop,
                    Reload,
                    Mode(&'static str),
                    ProfileSwitch,
                    ProfileUpdate,
                    ServiceCheck,
                    SocksSecret,
                    WebUi,
                    ExitProbe,
                }
                let mut btns: Vec<(Rect, OvBtn)> = Vec::new();
                if ov.overview_can_prev
                    && let Some(r) = ov.overview_prev_btn
                {
                    btns.push((r, OvBtn::DeckPrev));
                }
                if ov.overview_can_next
                    && let Some(r) = ov.overview_next_btn
                {
                    btns.push((r, OvBtn::DeckNext));
                }
                if ov.group_can_prev
                    && let Some(r) = ov.group_prev_btn
                {
                    btns.push((r, OvBtn::GroupPrev));
                }
                if ov.group_can_next
                    && let Some(r) = ov.group_next_btn
                {
                    btns.push((r, OvBtn::GroupNext));
                }
                if let Some(r) = ov.traffic_mode_chip {
                    btns.push((r, OvBtn::TrafficMode));
                }
                if let Some(r) = ov.traffic_window_chip {
                    btns.push((r, OvBtn::TrafficWindow));
                }
                if let Some(r) = ov.traffic_scale_chip {
                    btns.push((r, OvBtn::TrafficScale));
                }
                if let Some(r) = ov.traffic_pause_chip {
                    btns.push((r, OvBtn::TrafficPause));
                }
                if let Some(r) = ov.traffic_expand_btn {
                    btns.push((r, OvBtn::TrafficExpand));
                }
                if let Some(r) = ov.outbound_badge_hit {
                    btns.push((r, OvBtn::OutboundBadge));
                }
                if let Some(r) = ov.outbound_ip_hit {
                    btns.push((r, OvBtn::OutboundDetail));
                }
                if let Some(r) = ov.outbound_detail_btn {
                    btns.push((r, OvBtn::OutboundDetail));
                }
                if let Some(r) = ov.start_stop_btn {
                    btns.push((r, OvBtn::StartStop));
                }
                if let Some(r) = ov.reload_btn {
                    btns.push((r, OvBtn::Reload));
                }
                for &(rect, mode) in &ov.mode_pills {
                    btns.push((rect, OvBtn::Mode(mode)));
                }
                if let Some(r) = ov.profile_switch_btn {
                    btns.push((r, OvBtn::ProfileSwitch));
                }
                if let Some(r) = ov.profile_update_btn {
                    btns.push((r, OvBtn::ProfileUpdate));
                }
                if let Some(r) = ov.service_check_btn {
                    btns.push((r, OvBtn::ServiceCheck));
                }
                if let Some(r) = ov.socks_secret_btn {
                    btns.push((r, OvBtn::SocksSecret));
                }
                if let Some(r) = ov.webui_btn {
                    btns.push((r, OvBtn::WebUi));
                }
                if let Some(r) = ov.exit_probe_btn {
                    btns.push((r, OvBtn::ExitProbe));
                }

                if let Some(hit) = hit_best(btns, pos, slop) {
                    return match hit {
                        OvBtn::DeckPrev => {
                            self.clash.overview_scroll =
                                self.clash.overview_scroll.saturating_sub(1);
                            Ok(Effect::None)
                        }
                        OvBtn::DeckNext => {
                            self.clash.overview_scroll =
                                (self.clash.overview_scroll + 1).min(ov.overview_max_scroll);
                            Ok(Effect::None)
                        }
                        OvBtn::GroupPrev => {
                            self.clash.overview_group_page =
                                self.clash.overview_group_page.saturating_sub(1);
                            Ok(Effect::None)
                        }
                        OvBtn::GroupNext => {
                            self.clash.overview_group_page =
                                self.clash.overview_group_page.saturating_add(1);
                            Ok(Effect::None)
                        }
                        OvBtn::TrafficMode => {
                            self.clash.traffic_mode = self.clash.traffic_mode.next();
                            Ok(Effect::None)
                        }
                        OvBtn::TrafficWindow => {
                            self.clash.traffic_window = self.clash.traffic_window.next();
                            Ok(Effect::None)
                        }
                        OvBtn::TrafficScale => {
                            self.clash.traffic_scale = self.clash.traffic_scale.toggle();
                            Ok(Effect::None)
                        }
                        OvBtn::TrafficPause => {
                            self.clash.toggle_traffic_pause();
                            Ok(Effect::None)
                        }
                        OvBtn::TrafficExpand => {
                            self.clash.traffic_fullscreen = true;
                            Ok(Effect::None)
                        }
                        OvBtn::OutboundBadge | OvBtn::ExitProbe => {
                            self.clash.exit_probing = true;
                            self.clash.exit_probe_error = None;
                            Ok(Effect::ClashCmd(ClashMonitorCmd::ProbeExit))
                        }
                        OvBtn::OutboundDetail => Ok(self.open_outbound_ip_modal()),
                        OvBtn::StartStop if idle => self.clash_trigger_action("clash.stop"),
                        OvBtn::Reload if idle => self.clash_trigger_action("clash.reload"),
                        OvBtn::Mode(mode) => Ok(Effect::ClashCmd(ClashMonitorCmd::SetMode {
                            mode: mode.to_owned(),
                        })),
                        OvBtn::ProfileSwitch => {
                            self.clash.set_tab(ClashTab::Profiles);
                            Ok(Effect::None)
                        }
                        OvBtn::ProfileUpdate => {
                            if let Some(prof) = self.clash.snapshot.active_profile()
                                && !prof.is_local
                            {
                                let name = prof.name.clone();
                                if !self.clash.updating_all_profiles
                                    && self.clash.busy_profiles.insert(name.clone())
                                {
                                    return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateProfile {
                                        name,
                                    }));
                                }
                            }
                            Ok(Effect::None)
                        }
                        OvBtn::ServiceCheck => Ok(self.open_service_status_modal(None)),
                        OvBtn::SocksSecret => self.open_socks_secret_modal(),
                        OvBtn::WebUi => self.clash_trigger_webui(idle),
                        OvBtn::StartStop | OvBtn::Reload => Ok(Effect::None),
                    };
                }

                // Prevent near-miss taps on a card's button strip from falling through to
                // whole-card navigation in touch mode.
                if slop {
                    let near_any_btn = [
                        ov.overview_prev_btn,
                        ov.overview_next_btn,
                        ov.group_prev_btn,
                        ov.group_next_btn,
                        ov.traffic_mode_chip,
                        ov.traffic_window_chip,
                        ov.traffic_scale_chip,
                        ov.traffic_pause_chip,
                        ov.traffic_expand_btn,
                        ov.outbound_detail_btn,
                        ov.start_stop_btn,
                        ov.reload_btn,
                        ov.profile_switch_btn,
                        ov.profile_update_btn,
                        ov.service_check_btn,
                        ov.socks_secret_btn,
                        ov.webui_btn,
                        ov.exit_probe_btn,
                    ]
                    .into_iter()
                    .flatten()
                    .chain(ov.mode_pills.iter().map(|(r, _)| *r))
                    .any(|r| {
                        pos.y >= r.y.saturating_sub(1)
                            && pos.y <= r.bottom()
                            && pos.x >= r.x.saturating_sub(1)
                            && pos.x <= r.right()
                    });
                    if near_any_btn {
                        return Ok(Effect::None);
                    }
                }

                if ov.traffic_plot.contains(pos) || ov.traffic_side.contains(pos) {
                    self.clash.traffic_fullscreen = true;
                    return Ok(Effect::None);
                }
                if let Some(target) = hit_best(ov.service_cell_hits.iter().copied(), pos, slop) {
                    return Ok(self.open_service_status_modal(Some(target)));
                }
                if let Some(g_idx) = hit_best(ov.group_card_hits.iter().copied(), pos, slop) {
                    self.clash.overview_group_cursor = g_idx;
                    self.clash.set_tab(ClashTab::Proxies);
                    self.clash.select_group_index(g_idx);
                    return Ok(Effect::None);
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
                if !right_click {
                    #[derive(Clone, Copy)]
                    enum PxBtn {
                        GroupPrev,
                        GroupNext,
                        Group(usize),
                        GroupPicker,
                        Test,
                        Hide,
                        Providers,
                        Unfix,
                        Ping,
                        Fastest,
                        Locate,
                        DelayUrl,
                        PagePrev,
                        PageNext,
                    }
                    let mut btns = Vec::new();
                    if let Some(r) = px.group_prev_hit {
                        btns.push((r, PxBtn::GroupPrev));
                    }
                    if let Some(r) = px.group_next_hit {
                        btns.push((r, PxBtn::GroupNext));
                    }
                    for pill in &px.group_pills {
                        btns.push((pill.rect, PxBtn::Group(pill.group_idx)));
                    }
                    if let Some(r) = px.group_picker_btn {
                        btns.push((r, PxBtn::GroupPicker));
                    }
                    if let Some(r) = px.test_btn {
                        btns.push((r, PxBtn::Test));
                    }
                    if let Some(r) = px.hide_chip {
                        btns.push((r, PxBtn::Hide));
                    }
                    if let Some(r) = px.providers_btn {
                        btns.push((r, PxBtn::Providers));
                    }
                    if let Some(r) = px.unfix_btn {
                        btns.push((r, PxBtn::Unfix));
                    }
                    if let Some(r) = px.ping_btn {
                        btns.push((r, PxBtn::Ping));
                    }
                    if let Some(r) = px.fastest_btn {
                        btns.push((r, PxBtn::Fastest));
                    }
                    if let Some(r) = px.locate_btn {
                        btns.push((r, PxBtn::Locate));
                    }
                    if let Some(r) = px.delay_url_btn {
                        btns.push((r, PxBtn::DelayUrl));
                    }
                    if px.can_prev
                        && let Some(r) = px.prev_btn
                    {
                        btns.push((r, PxBtn::PagePrev));
                    }
                    if px.can_next
                        && let Some(r) = px.next_btn
                    {
                        btns.push((r, PxBtn::PageNext));
                    }
                    if let Some(hit) = hit_best(btns, pos, slop) {
                        return match hit {
                            PxBtn::GroupPrev => {
                                self.clash.cycle_group(false);
                                Ok(Effect::None)
                            }
                            PxBtn::GroupNext => {
                                self.clash.cycle_group(true);
                                Ok(Effect::None)
                            }
                            PxBtn::Group(idx) => {
                                self.clash.select_group_index(idx);
                                Ok(Effect::None)
                            }
                            PxBtn::GroupPicker => {
                                self.clash.modal = Some(ClashModal::GroupPicker {
                                    cursor: self.clash.group_cursor,
                                });
                                Ok(Effect::None)
                            }
                            PxBtn::Test => {
                                if let Some(group) = self.clash.current_group() {
                                    let g = group.name.clone();
                                    if self.clash.testing_groups.insert(g.clone()) {
                                        return Ok(Effect::ClashCmd(
                                            ClashMonitorCmd::TestGroupDelay {
                                                group: g,
                                                url: self.clash.delay_url.clone(),
                                                timeout_ms: self.clash.delay_timeout_ms,
                                            },
                                        ));
                                    }
                                }
                                Ok(Effect::None)
                            }
                            PxBtn::Hide => {
                                self.clash.hide_timeout = !self.clash.hide_timeout;
                                self.clash.clamp_cursors();
                                Ok(Effect::None)
                            }
                            PxBtn::Providers => {
                                self.clash.modal = Some(ClashModal::Providers {
                                    show_rules: false,
                                    cursor: 0,
                                });
                                Ok(Effect::None)
                            }
                            PxBtn::Unfix => Ok(self.clash_unfix_current_group()),
                            PxBtn::Ping => Ok(self.clash_ping_current_node()),
                            PxBtn::Fastest => Ok(self.clash_select_fastest_node()),
                            PxBtn::Locate => {
                                if !self.clash.locate_active_node() {
                                    self.clash_toast_key("clash.toast.locate_miss", Tone::Info);
                                }
                                Ok(Effect::None)
                            }
                            PxBtn::DelayUrl => {
                                self.clash.modal = Some(ClashModal::DelaySettings {
                                    url: InputBuffer::new(self.clash.delay_url.clone()),
                                    timeout: InputBuffer::new(
                                        self.clash.delay_timeout_ms.to_string(),
                                    ),
                                    active_field: 0,
                                    error: None,
                                });
                                Ok(Effect::None)
                            }
                            PxBtn::PagePrev => {
                                let cols = self.clash.last_grid_cols.get().max(1);
                                let step = cols * px.grid_visible_rows.max(1);
                                self.clash.node_cursor =
                                    self.clash.node_cursor.saturating_sub(step);
                                Ok(Effect::None)
                            }
                            PxBtn::PageNext => {
                                let len = self.clash.filtered_nodes().len();
                                if len > 0 {
                                    let cols = self.clash.last_grid_cols.get().max(1);
                                    let step = cols * px.grid_visible_rows.max(1);
                                    self.clash.node_cursor =
                                        (self.clash.node_cursor + step).min(len - 1);
                                }
                                Ok(Effect::None)
                            }
                        };
                    }
                }
                if hit_opt(px.sort_chip, pos, slop) {
                    if right_click {
                        self.clash.proxy_sort_asc = !self.clash.proxy_sort_asc;
                    } else {
                        self.clash.proxy_sort = self.clash.proxy_sort.next();
                        self.clash.proxy_sort_asc = true;
                    }
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if let Some((rect, idx)) = hit_best(
                    px.node_hits.iter().copied().map(|(r, i)| (r, (r, i))),
                    pos,
                    slop,
                ) {
                    self.clash.node_cursor = idx;
                    if right_click
                        || proxy_card_delay_hit(rect).is_some_and(|d| hit_rect(d, pos, false))
                    {
                        return Ok(self.clash_ping_current_node());
                    }
                    return Ok(self.clash_select_current_node());
                }
            }
            ClashBodyGeom::Profiles(pf) => {
                let mut btns = Vec::new();
                if let Some(r) = pf.update_all_btn {
                    btns.push((r, 0u8));
                }
                if idle && let Some(r) = pf.add_btn {
                    btns.push((r, 1u8));
                }
                if idle && let Some(r) = pf.import_btn {
                    btns.push((r, 2u8));
                }
                if let Some(r) = pf.providers_btn {
                    btns.push((r, 3u8));
                }
                if pf.can_prev
                    && let Some(r) = pf.prev_btn
                {
                    btns.push((r, 4u8));
                }
                if pf.can_next
                    && let Some(r) = pf.next_btn
                {
                    btns.push((r, 5u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        if !self.clash.updating_all_profiles && self.clash.busy_profiles.is_empty()
                        {
                            self.clash.updating_all_profiles = true;
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::UpdateAllProfiles));
                        }
                        return Ok(Effect::None);
                    }
                    Some(1) => return self.clash_trigger_action("clash.sub.add"),
                    Some(2) => return self.clash_trigger_action("clash.sub.import"),
                    Some(3) => {
                        self.clash.modal = Some(ClashModal::Providers {
                            show_rules: false,
                            cursor: 0,
                        });
                        return Ok(Effect::None);
                    }
                    Some(4) => {
                        let step = pf.item_hits.len().max(1);
                        self.clash.profile_cursor = self.clash.profile_cursor.saturating_sub(step);
                        return Ok(Effect::None);
                    }
                    Some(5) => {
                        let len = self.clash.filtered_profiles().len();
                        if len > 0 {
                            let step = pf.item_hits.len().max(1);
                            self.clash.profile_cursor =
                                (self.clash.profile_cursor + step).min(len - 1);
                        }
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
                if let Some(idx) = hit_best(pf.item_hits.iter().copied(), pos, slop) {
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
            ClashBodyGeom::Connections(cn) => {
                let mut btns = Vec::new();
                if let Some(r) = cn.scope_chip {
                    btns.push((r, 0u8));
                }
                if let Some(r) = cn.sort_chip {
                    btns.push((r, 1u8));
                }
                if let Some(r) = cn.close_all_btn {
                    btns.push((r, 2u8));
                }
                if cn.can_prev
                    && let Some(r) = cn.prev_btn
                {
                    btns.push((r, 3u8));
                }
                if cn.can_next
                    && let Some(r) = cn.next_btn
                {
                    btns.push((r, 4u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        self.clash.conn_scope = self.clash.conn_scope.next();
                        self.clash.conn_facet = None;
                        self.clash.clamp_cursors();
                        return Ok(Effect::None);
                    }
                    Some(1) => {
                        self.clash.conn_sort = self.clash.conn_sort.next();
                        self.clash.clamp_cursors();
                        return Ok(Effect::None);
                    }
                    Some(2) if !self.clash.snapshot.connections.is_empty() => {
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
                    Some(3) => {
                        let step = (cn.list_inner.height / cn.row_height.max(1)).max(1) as usize;
                        self.clash.conn_cursor = self.clash.conn_cursor.saturating_sub(step);
                        self.clash.conn_inspector_scroll = 0;
                        return Ok(Effect::None);
                    }
                    Some(4) => {
                        let len = self.clash.filtered_connections().len();
                        if len > 0 {
                            let step =
                                (cn.list_inner.height / cn.row_height.max(1)).max(1) as usize;
                            self.clash.conn_cursor = (self.clash.conn_cursor + step).min(len - 1);
                            self.clash.conn_inspector_scroll = 0;
                        }
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
                if let Some(f_idx) = hit_best(
                    cn.facet_chips
                        .iter()
                        .enumerate()
                        .map(|(idx, (r, _))| (*r, idx)),
                    pos,
                    slop,
                ) {
                    self.clash.conn_facet = cn.facet_chips[f_idx].1.clone();
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if cn.list_inner.contains(pos) {
                    let row =
                        (pos.y.saturating_sub(cn.list_inner.y) / cn.row_height.max(1)) as usize;
                    let idx = cn.list_scroll + row;
                    if idx < self.clash.filtered_connections().len() {
                        self.clash.conn_cursor = idx;
                        self.clash.conn_inspector_scroll = 0;
                    }
                }
            }
            ClashBodyGeom::Rules(rl) => {
                let mut btns = Vec::new();
                if let Some(r) = rl.providers_btn {
                    btns.push((r, 0u8));
                }
                if rl.can_prev
                    && let Some(r) = rl.prev_btn
                {
                    btns.push((r, 1u8));
                }
                if rl.can_next
                    && let Some(r) = rl.next_btn
                {
                    btns.push((r, 2u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        self.clash.modal = Some(ClashModal::Providers {
                            show_rules: true,
                            cursor: 0,
                        });
                        return Ok(Effect::None);
                    }
                    Some(1) => {
                        let step = (rl.list_inner.height / rl.row_height.max(1)).max(1) as usize;
                        self.clash.rule_cursor = self.clash.rule_cursor.saturating_sub(step);
                        return Ok(Effect::None);
                    }
                    Some(2) => {
                        let len = self.clash.filtered_rules().len();
                        if len > 0 {
                            let step =
                                (rl.list_inner.height / rl.row_height.max(1)).max(1) as usize;
                            self.clash.rule_cursor = (self.clash.rule_cursor + step).min(len - 1);
                        }
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
                if let Some(f_idx) = hit_best(
                    rl.facet_chips
                        .iter()
                        .enumerate()
                        .map(|(idx, (r, _))| (*r, idx)),
                    pos,
                    slop,
                ) {
                    self.clash.rule_facet = rl.facet_chips[f_idx].1.clone();
                    self.clash.clamp_cursors();
                    return Ok(Effect::None);
                }
                if rl.list_inner.contains(pos) {
                    let row =
                        (pos.y.saturating_sub(rl.list_inner.y) / rl.row_height.max(1)) as usize;
                    let idx = rl.list_scroll + row;
                    if idx < self.clash.filtered_rules().len() {
                        self.clash.rule_cursor = idx;
                    }
                }
            }
            ClashBodyGeom::Tools(tl) => {
                let mut btns = Vec::new();
                if let Some(r) = tl.logs_btn {
                    btns.push((r, 0u8));
                }
                if tl.can_prev
                    && let Some(r) = tl.prev_btn
                {
                    btns.push((r, 1u8));
                }
                if tl.can_next
                    && let Some(r) = tl.next_btn
                {
                    btns.push((r, 2u8));
                }
                match hit_best(btns, pos, slop) {
                    Some(0) => {
                        self.clash.tools_log_mode = !self.clash.tools_log_mode;
                        if self.clash.tools_log_mode {
                            self.clash.log_loading = true;
                            return Ok(Effect::ClashCmd(ClashMonitorCmd::FetchLogs));
                        }
                        return Ok(Effect::None);
                    }
                    Some(1) => {
                        if self.clash.tools_log_mode {
                            self.clash.log_scroll = self.clash.log_scroll.saturating_sub(10);
                        } else {
                            let step =
                                (tl.list_inner.height / tl.row_height.max(1)).max(1) as usize;
                            self.clash.tool_cursor = self.clash.tool_cursor.saturating_sub(step);
                        }
                        return Ok(Effect::None);
                    }
                    Some(2) => {
                        if self.clash.tools_log_mode {
                            let max_s = self.clash.max_log_scroll.get();
                            self.clash.log_scroll =
                                self.clash.log_scroll.saturating_add(10).min(max_s);
                        } else {
                            let len = self.clash.filtered_tools(&self.ctx.i18n).len();
                            if len > 0 {
                                let step =
                                    (tl.list_inner.height / tl.row_height.max(1)).max(1) as usize;
                                self.clash.tool_cursor =
                                    (self.clash.tool_cursor + step).min(len - 1);
                            }
                        }
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
                if !self.clash.tools_log_mode && tl.list_inner.contains(pos) {
                    let row =
                        (pos.y.saturating_sub(tl.list_inner.y) / tl.row_height.max(1)) as usize;
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
