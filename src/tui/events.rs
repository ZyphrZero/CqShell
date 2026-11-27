//! Keyboard and mouse event routing following the Overlay Priority Stack:
//! Confirmation -> Loading -> SocksSecret -> Picker -> Form -> PromptPreview -> DockerModal ->
//! Notice -> Help -> Full Logs -> Command Palette -> Sidebar Overlay -> Theme Drawer -> Main Workspace.

use super::{
    layout::{
        self, CommandPaletteGeometry, ConfirmationGeometry, FormGeometry, LayoutRegions,
        McpAccessGeometry, PickerGeometry, PromptPreviewGeometry, SocksSecretGeometry, ThumbAction,
        hit_best, hit_opt, hit_rect,
    },
    palette::{self, PaletteItem},
    selection::{self, DragSelection, TextPane},
    state::{App, Effect, NavTarget, OPTIMIZATION_GROUP, Row, step},
};
use crate::error::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(420);

impl App {
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Result<Effect> {
        self.drag_select = None;
        // Global interrupt: Ctrl+C always interrupts; Q while running requests graceful exit.
        let ctrl_c = matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_c || (self.worker.is_some() && matches!(key.code, KeyCode::Char('q' | 'Q'))) {
            if self.worker.is_some() {
                self.request_cancel(true);
                return Ok(Effect::None);
            }
            return Ok(Effect::Quit);
        }

        // 1. Confirmation modal (highest priority, never bypassed)
        if self.confirmation.is_some() {
            return self.confirm_key(key);
        }
        // 2. Async listing loader
        if self.loading.is_some() {
            if key.code == KeyCode::Esc {
                self.cancel_loading();
            }
            return Ok(Effect::None);
        }
        // 2b. Transient SOCKS5 / 5h credential reveal modal (never written to task logs)
        if self.socks_secret.is_some() {
            return self.socks_secret_key(key);
        }
        // 2c. Transient MCP access & Bearer token copy modal (always masked on screen)
        if self.mcp_access.is_some() {
            return self.mcp_access_key(key);
        }
        // 3. Form / Picker
        if self.form.as_ref().is_some_and(|form| form.picker.is_some()) {
            return self.picker_key(key);
        }
        if self.form.is_some() {
            return self.form_key(key);
        }
        // 3b. Interactive Terminal Prompt Preview Studio
        if self.prompt_preview.is_some() {
            return self.prompt_preview_key(key);
        }
        // 3c. Docker-specific modals (ActionMenu / Pull / CreateNetwork / ExecForm / PruneWizard)
        if self.docker_has_modal() {
            return self.docker_key(key);
        }
        // 3d. Mihomo-specific modals (DelaySettings / GroupPicker / Providers / Confirm)
        if self.clash_has_modal() {
            return self.clash_key(key);
        }
        // 4. Notice dialog
        if let Some(notice) = self.notice.clone() {
            match key.code {
                KeyCode::Char('c' | 'C' | 'y' | 'Y') => return Ok(self.copy_plain(notice)),
                KeyCode::Char('v' | 'V') => {
                    return Ok(Effect::PlainView {
                        title: self.ctx.i18n.text("ui.notice").to_owned(),
                        content: notice,
                    });
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q') => {
                    self.notice = None;
                }
                _ => {}
            }
            return Ok(Effect::None);
        }
        // 5. Help modal
        if self.help {
            let max_s = super::render::help_max_scroll(self, self.viewport);
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q' | '?') | KeyCode::F(1) => {
                    self.help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.min(max_s).saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(max_s);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.min(max_s).saturating_sub(6);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(6).min(max_s);
                }
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::End => self.help_scroll = max_s,
                _ => {}
            }
            return Ok(Effect::None);
        }
        // 6. Full-screen logs modal
        if self.log_modal {
            match key.code {
                KeyCode::Esc | KeyCode::Char('g' | 'G' | 'q' | 'Q') | KeyCode::F(2) => {
                    self.log_modal = false;
                }
                KeyCode::Char('c' | 'C' | 'y' | 'Y') => return Ok(self.copy_logs()),
                KeyCode::Char('v' | 'V') => return Ok(self.open_plain_view_logs()),
                code => self.log_key(code, true),
            }
            return Ok(Effect::None);
        }
        // 7. Enhanced Command Palette
        if self.command_palette.is_some() {
            return self.palette_key(key);
        }
        // 8. Compact-mode Sidebar Drawer Overlay
        if self.sidebar_overlay {
            return self.sidebar_overlay_key(key);
        }
        // 9. Right-docked Appearance & Theme Drawer
        if self.theme_drawer.is_some() {
            return self.theme_drawer_key(key);
        }
        // 10. Main Workspace
        if self.is_docker_view() {
            return self.docker_key(key);
        }
        if self.is_clash_view() {
            return self.clash_key(key);
        }
        self.main_key(key)
    }

    fn main_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let idle = self.worker.is_none();

        // Ctrl+K opens enhanced Command Palette; Ctrl+U/D scroll inspector on narrow viewports
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('k' | 'K') => {
                    self.command_palette = Some(palette::CommandPalette::default());
                    return Ok(Effect::None);
                }
                KeyCode::Char('u' | 'U') if self.nav != NavTarget::Overview => {
                    self.inspector_scroll = self.inspector_scroll.saturating_sub(2);
                    return Ok(Effect::None);
                }
                KeyCode::Char('d' | 'D') if self.nav != NavTarget::Overview => {
                    let max_s = self.max_inspector_scroll.get();
                    self.inspector_scroll = self.inspector_scroll.saturating_add(2).min(max_s);
                    return Ok(Effect::None);
                }
                _ => {}
            }
        }

        // Alt+Up/Down reorders current functional category
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down)
        {
            if idle {
                self.reorder_category(key.code == KeyCode::Down);
            }
            return Ok(Effect::None);
        }

        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char(_))
        {
            return Ok(Effect::None);
        }

        let overview_list = self.nav == NavTarget::Overview
            && LayoutRegions::compute(self.viewport, self)
                .overview
                .is_some_and(|ov| ov.is_list_mode());

        match key.code {
            KeyCode::Esc => {
                if !idle {
                    self.request_cancel(false);
                }
            }
            // Overview quick-card grid/list navigation vs Module list navigation
            KeyCode::Up | KeyCode::Char('k') => {
                if self.nav == NavTarget::Overview {
                    self.quick_cursor = if overview_list {
                        (self.quick_cursor + 3) % 4
                    } else {
                        (self.quick_cursor + 2) % 4
                    };
                } else {
                    self.move_row(false);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.nav == NavTarget::Overview {
                    self.quick_cursor = if overview_list {
                        (self.quick_cursor + 1) % 4
                    } else {
                        (self.quick_cursor + 2) % 4
                    };
                } else {
                    self.move_row(true);
                }
            }
            KeyCode::Left => {
                if self.nav == NavTarget::Overview {
                    self.quick_cursor = if overview_list {
                        (self.quick_cursor + 3) % 4
                    } else {
                        self.quick_cursor ^ 1
                    };
                } else if let Some(group) = self.active_group()
                    && self.cycle_section_tab(group, false)
                {
                } else {
                    self.cycle_nav(false);
                }
            }
            KeyCode::Right => {
                if self.nav == NavTarget::Overview {
                    self.quick_cursor = if overview_list {
                        (self.quick_cursor + 1) % 4
                    } else {
                        self.quick_cursor ^ 1
                    };
                } else if let Some(group) = self.active_group()
                    && self.cycle_section_tab(group, true)
                {
                } else {
                    self.cycle_nav(true);
                }
            }
            KeyCode::PageUp if !self.logs_open => {
                if let Some(group) = self.active_group() {
                    self.jump_section(group, false);
                }
            }
            KeyCode::PageDown if !self.logs_open => {
                if let Some(group) = self.active_group() {
                    self.jump_section(group, true);
                }
            }
            KeyCode::BackTab => self.cycle_nav(false),
            KeyCode::Tab => self.cycle_nav(true),
            KeyCode::Home => {
                if let Some(group) = self.active_group() {
                    self.positions[group] = 0;
                } else {
                    self.quick_cursor = 0;
                }
            }
            KeyCode::End => {
                if let Some(group) = self.active_group() {
                    let count = self.selectable_count(group);
                    self.positions[group] = count.saturating_sub(1);
                } else {
                    self.quick_cursor = 3;
                }
            }
            KeyCode::Char('h' | 'H') => self.goto_nav(NavTarget::Overview),
            KeyCode::Char('o' | 'O' | '0') => self.goto_nav(NavTarget::Optimization),
            KeyCode::Char(digit @ '1'..='9') => {
                let n = digit as usize - '0' as usize;
                if let Some(&group) = self.category_order.get(n - 1) {
                    self.goto_nav(NavTarget::Group(group));
                }
            }
            KeyCode::Char('[') => self.toggle_sidebar(),
            KeyCode::Char('/') => {
                self.command_palette = Some(palette::CommandPalette::default());
            }
            KeyCode::Char('t' | 'T') => self.open_theme_drawer(),
            KeyCode::Char('p' | 'P') if idle => return self.open_github_proxy_form(),
            KeyCode::Char('S') => return self.open_socks_secret_modal(),
            KeyCode::Char('M') => return self.open_mcp_access_modal(),
            KeyCode::Char('r' | 'R') => return Ok(Effect::Refresh),
            KeyCode::Char('n' | 'N') => return Ok(Effect::PublicIp),
            KeyCode::Char('g' | 'G') => {
                if self.viewport.height.saturating_sub(2) < 16 {
                    self.log_modal = !self.log_modal;
                } else {
                    self.logs_open = !self.logs_open;
                }
            }
            KeyCode::F(2) => self.log_modal = !self.log_modal,
            KeyCode::Char('c') => return Ok(self.copy_current_command()),
            KeyCode::Char('C') => return Ok(self.copy_current_inspector()),
            KeyCode::Char('y' | 'Y') => {
                return Ok(
                    if !self.logs.is_empty() || self.nav == NavTarget::Overview {
                        self.copy_logs()
                    } else {
                        self.copy_current_inspector()
                    },
                );
            }
            KeyCode::Char('v' | 'V') => return Ok(self.open_plain_view_context()),
            KeyCode::Char('l' | 'L') => self.cycle_language()?,
            KeyCode::Char('q' | 'Q') if idle => return Ok(Effect::Quit),
            KeyCode::Enter if idle => return self.activate(),
            KeyCode::Char(' ') if idle => self.toggle_selected()?,
            KeyCode::Char('x' | 'X') | KeyCode::F(5) if idle => return self.optimization_jobs(),
            KeyCode::Char('a' | 'A') if idle => self.recommend()?,
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            code => self.log_key(code, false),
        }
        Ok(Effect::None)
    }

    fn sidebar_overlay_key(&mut self, key: KeyEvent) -> Result<Effect> {
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down)
        {
            if self.worker.is_none() {
                self.reorder_category(key.code == KeyCode::Down);
            }
            return Ok(Effect::None);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('[') | KeyCode::Enter => {
                self.sidebar_overlay = false;
            }
            KeyCode::Char('t' | 'T') => {
                self.sidebar_overlay = false;
                self.open_theme_drawer();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let order = self.nav_order();
                let cur = order.iter().position(|t| *t == self.nav);
                self.nav = order[step(cur, order.len(), false)];
                self.view_changed = self.now;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let order = self.nav_order();
                let cur = order.iter().position(|t| *t == self.nav);
                self.nav = order[step(cur, order.len(), true)];
                self.view_changed = self.now;
            }
            KeyCode::Char('h' | 'H') => self.goto_nav(NavTarget::Overview),
            KeyCode::Char('o' | 'O' | '0') => self.goto_nav(NavTarget::Optimization),
            KeyCode::Char(digit @ '1'..='9') => {
                let n = digit as usize - '0' as usize;
                if let Some(&group) = self.category_order.get(n - 1) {
                    self.goto_nav(NavTarget::Group(group));
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn theme_drawer_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let Some(drawer) = &self.theme_drawer else {
            return Ok(Effect::None);
        };
        let last = drawer.active_count().saturating_sub(1);
        let cur = drawer.active_cursor();
        let next_tab = drawer.tab.toggle();
        match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h' | 'l') => self.switch_theme_drawer_tab(next_tab),
            KeyCode::Up | KeyCode::Char('k') => self.preview_theme_cursor(cur.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.preview_theme_cursor((cur + 1).min(last)),
            KeyCode::Home => self.preview_theme_cursor(0),
            KeyCode::End => self.preview_theme_cursor(last),
            KeyCode::Char('b' | 'B') => self.toggle_transparent_bg(false),
            KeyCode::Char('m' | 'M') => self.toggle_reduced_motion(false),
            KeyCode::Char('d' | 'D') => self.cycle_ui_density(false),
            KeyCode::Esc | KeyCode::Char('t' | 'T') => self.cancel_theme_drawer(),
            KeyCode::Enter => self.save_theme_drawer(),
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn palette_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let specs = crate::actions::specs_slice();
        let results = {
            let pal = self.command_palette.as_ref().expect("open palette");
            palette::search(
                &pal.query.text,
                specs,
                &self.ctx.i18n,
                &self.category_order,
                &self.records,
            )
        };
        let count = results.count();
        let last = count.saturating_sub(1);
        let geom = CommandPaletteGeometry::compute(
            self.viewport,
            &results,
            self.command_palette.as_ref().unwrap().cursor,
            self.resolved_density(self.viewport),
        );
        let page = (geom.list.height / geom.row_height.max(1))
            .saturating_sub(1)
            .max(1) as usize;

        match key.code {
            KeyCode::Esc => {
                self.command_palette = None;
                return Ok(Effect::None);
            }
            KeyCode::Enter => {
                let cursor = self.command_palette.as_ref().unwrap().cursor;
                let item = results.item_at(cursor).cloned();
                self.command_palette = None;
                return self.execute_palette_item(item, false);
            }
            KeyCode::Tab => {
                let cursor = self.command_palette.as_ref().unwrap().cursor;
                let item = results.item_at(cursor).cloned();
                self.command_palette = None;
                return self.execute_palette_item(item, true);
            }
            KeyCode::Down if count > 0 => {
                let pal = self.command_palette.as_mut().unwrap();
                pal.cursor = (pal.cursor + 1) % count;
            }
            KeyCode::Up if count > 0 => {
                let pal = self.command_palette.as_mut().unwrap();
                pal.cursor = (pal.cursor + count - 1) % count;
            }
            KeyCode::PageDown if count > 0 => {
                let pal = self.command_palette.as_mut().unwrap();
                pal.cursor = (pal.cursor + page).min(last);
            }
            KeyCode::PageUp => {
                let pal = self.command_palette.as_mut().unwrap();
                pal.cursor = pal.cursor.saturating_sub(page);
            }
            KeyCode::Left => self.command_palette.as_mut().unwrap().query.left(),
            KeyCode::Right => self.command_palette.as_mut().unwrap().query.right(),
            KeyCode::Home => self.command_palette.as_mut().unwrap().query.cursor = 0,
            KeyCode::End => {
                let q = &mut self.command_palette.as_mut().unwrap().query;
                q.cursor = q.text.len();
            }
            _ => {
                let pal = self.command_palette.as_mut().unwrap();
                let mut edited = true;
                match key.code {
                    KeyCode::Backspace => pal.query.backspace(),
                    KeyCode::Delete => pal.query.delete(),
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        pal.query.clear();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        pal.query.insert(c);
                    }
                    _ => edited = false,
                }
                if edited {
                    pal.cursor = 0;
                }
            }
        }
        Ok(Effect::None)
    }

    fn execute_palette_item(
        &mut self,
        item: Option<PaletteItem>,
        jump_only: bool,
    ) -> Result<Effect> {
        let Some(item) = item else {
            return Ok(Effect::None);
        };
        match item {
            PaletteItem::Command { command, .. } => self.run_workbench_command(command),
            PaletteItem::Action { spec, .. } => {
                self.jump_to_action(&spec);
                if jump_only {
                    Ok(Effect::None)
                } else {
                    self.activate_action(spec)
                }
            }
        }
    }

    fn picker_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let (highlighted, count, page) = {
            let form = self.form.as_ref().expect("active form");
            let picker = form.picker.as_ref().expect("open picker");
            let choices = form.choices(form.index);
            let matches = picker.matches(choices);
            let geom = PickerGeometry::compute(
                self.viewport,
                matches.len(),
                picker.cursor,
                self.resolved_density(self.viewport),
            );
            (
                matches.get(picker.cursor).map(|c| c.value.clone()),
                matches.len(),
                (geom.list.height / geom.row_height.max(1))
                    .saturating_sub(1)
                    .max(1) as usize,
            )
        };
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.close_picker(),
            KeyCode::Enter => {
                if let Some(value) = highlighted {
                    return self.pick_choice(value);
                }
            }
            KeyCode::Down if count > 0 => {
                let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                picker.cursor = (picker.cursor + 1) % count;
            }
            KeyCode::Up if count > 0 => {
                let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                picker.cursor = (picker.cursor + count - 1) % count;
            }
            KeyCode::PageDown if count > 0 => {
                let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                picker.cursor = (picker.cursor + page).min(last);
            }
            KeyCode::PageUp => {
                let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                picker.cursor = picker.cursor.saturating_sub(page);
            }
            KeyCode::Home => self.form.as_mut().unwrap().picker.as_mut().unwrap().cursor = 0,
            KeyCode::End => self.form.as_mut().unwrap().picker.as_mut().unwrap().cursor = last,
            KeyCode::Left => self
                .form
                .as_mut()
                .unwrap()
                .picker
                .as_mut()
                .unwrap()
                .filter
                .left(),
            KeyCode::Right => self
                .form
                .as_mut()
                .unwrap()
                .picker
                .as_mut()
                .unwrap()
                .filter
                .right(),
            _ => {
                let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                let mut edited = true;
                match key.code {
                    KeyCode::Backspace => picker.filter.backspace(),
                    KeyCode::Delete => picker.filter.delete(),
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        picker.filter.clear();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        picker.filter.insert(c);
                    }
                    _ => edited = false,
                }
                if edited {
                    picker.cursor = 0;
                }
            }
        }
        Ok(Effect::None)
    }

    fn socks_secret_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let max_s = super::render::socks_secret_max_scroll(self, self.viewport);
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q' | 'S') => {
                self.close_socks_secret();
            }
            KeyCode::Char('1' | 'c' | 'C') => {
                if let Some(modal) = &self.socks_secret {
                    let uri = modal.creds.socks5h_uri.clone();
                    return Ok(self.copy_socks_secret_field(uri, "ui.socks_secret_field_socks5h"));
                }
            }
            KeyCode::Char('2') => {
                if let Some(modal) = &self.socks_secret {
                    let uri = modal.creds.socks5_uri.clone();
                    return Ok(self.copy_socks_secret_field(uri, "ui.socks_secret_field_socks5"));
                }
            }
            KeyCode::Char('3' | 'p' | 'P') => {
                return Ok(self.copy_socks_secret_password());
            }
            KeyCode::Char('4') => {
                if let Some(modal) = &self.socks_secret {
                    let cmd = modal.creds.curl_cmd.clone();
                    return Ok(self.copy_socks_secret_field(cmd, "ui.socks_secret_field_curl"));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = modal.scroll.min(max_s).saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = modal.scroll.saturating_add(1).min(max_s);
                }
            }
            KeyCode::PageUp => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = modal.scroll.min(max_s).saturating_sub(6);
                }
            }
            KeyCode::PageDown => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = modal.scroll.saturating_add(6).min(max_s);
                }
            }
            KeyCode::Home => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = 0;
                }
            }
            KeyCode::End => {
                if let Some(modal) = self.socks_secret.as_mut() {
                    modal.scroll = max_s;
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn mcp_access_key(&mut self, key: KeyEvent) -> Result<Effect> {
        use crate::mcp::config::{ClientFormat, TransportMode};
        let max_s = super::render::mcp_access_max_scroll(self, self.viewport);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                self.close_mcp_access();
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.transport = m.transport.next();
                    m.scroll = 0;
                }
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.transport = m.transport.prev();
                    m.scroll = 0;
                }
            }
            KeyCode::Char('f' | 'F') => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.client = m.client.next();
                    m.scroll = 0;
                }
            }
            KeyCode::Char('1') => {
                if let Some(m) = &self.mcp_access {
                    let text = m.access.copy_snippet(m.transport, ClientFormat::Json);
                    return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_json"));
                }
            }
            KeyCode::Char('2') => {
                if let Some(m) = &self.mcp_access {
                    let text = m.access.copy_snippet(m.transport, ClientFormat::Claude);
                    return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_claude"));
                }
            }
            KeyCode::Char('3') => {
                if let Some(m) = &self.mcp_access {
                    let text = m.access.copy_snippet(m.transport, ClientFormat::Codex);
                    return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_codex"));
                }
            }
            KeyCode::Char('4') => {
                if let Some(m) = &self.mcp_access {
                    if m.transport == TransportMode::Http
                        && let Some(tok) = m.access.token.clone()
                    {
                        return Ok(self.copy_mcp_field(tok, "ui.mcp_access_field_token"));
                    }
                    let tunnel = m.access.ssh_tunnel_cmd();
                    return Ok(self.copy_mcp_field(tunnel, "ui.mcp_access_field_tunnel"));
                }
            }
            KeyCode::Char('r' | 'R') => {
                self.rotate_mcp_access_token();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = m.scroll.min(max_s).saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = m.scroll.saturating_add(1).min(max_s);
                }
            }
            KeyCode::PageUp => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = m.scroll.min(max_s).saturating_sub(6);
                }
            }
            KeyCode::PageDown => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = m.scroll.saturating_add(6).min(max_s);
                }
            }
            KeyCode::Home => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = 0;
                }
            }
            KeyCode::End => {
                if let Some(m) = self.mcp_access.as_mut() {
                    m.scroll = max_s;
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn form_key(&mut self, key: KeyEvent) -> Result<Effect> {
        if key.code == KeyCode::Esc {
            self.form = None;
            return Ok(Effect::None);
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.submit_form();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s' | 'S'))
            && self
                .form
                .as_ref()
                .is_some_and(|f| f.spec.id == "clash.socks.configure")
        {
            return self.open_socks_secret_modal();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('m' | 'M'))
            && self
                .form
                .as_ref()
                .is_some_and(|f| f.spec.id.starts_with("ai.mcp."))
        {
            return self.open_mcp_access_modal();
        }
        let form = self.form.as_mut().expect("active form");
        let n = form.values.len();
        form.error = None;
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.index = (form.index + 1) % (n + 2),
            KeyCode::BackTab | KeyCode::Up => form.index = (form.index + n + 1) % (n + 2),
            KeyCode::Enter if form.index == n => return self.submit_form(),
            KeyCode::Enter if form.index == n + 1 => self.form = None,
            KeyCode::Enter if form.listed(form.index) => form.open_picker(String::new(), false),
            KeyCode::Enter => return self.advance_form(),
            KeyCode::Left => self.cycle_field(false),
            KeyCode::Right => self.cycle_field(true),
            KeyCode::Char(' ') if form.index >= n => {
                if form.index == n {
                    return self.submit_form();
                }
                self.form = None;
            }
            KeyCode::Char(c)
                if form.index < n
                    && form.listed(form.index)
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let seed = if c == ' ' {
                    String::new()
                } else {
                    c.to_string()
                };
                form.open_picker(seed, false);
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && form.index < n
                    && form.spec.fields[form.index].editable() =>
            {
                form.values[form.index].clear();
            }
            KeyCode::Home if form.index < n => form.values[form.index].cursor = 0,
            KeyCode::End if form.index < n => {
                let len = form.values[form.index].text.len();
                form.values[form.index].cursor = len;
            }
            KeyCode::Backspace if form.index < n && form.spec.fields[form.index].editable() => {
                form.values[form.index].backspace();
            }
            KeyCode::Delete if form.index < n && form.spec.fields[form.index].editable() => {
                form.values[form.index].delete();
            }
            KeyCode::Char(c)
                if form.index < n
                    && form.spec.fields[form.index].editable()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                form.values[form.index].insert(c);
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn prompt_preview_key(&mut self, key: KeyEvent) -> Result<Effect> {
        use crate::ops::terminal::{COLORS, SHELLS, STYLES};
        let max_toml = self.prompt_preview_max_scroll(self.viewport);
        let Some(preview) = self.prompt_preview.as_mut() else {
            return Ok(Effect::None);
        };
        let style_count = STYLES.len();
        let color_count = COLORS.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                self.prompt_preview = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                preview.style_idx = (preview.style_idx + style_count - 1) % style_count;
                preview.toml_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                preview.style_idx = (preview.style_idx + 1) % style_count;
                preview.toml_scroll = 0;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                preview.color_idx = (preview.color_idx + color_count - 1) % color_count;
                preview.toml_scroll = 0;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                preview.color_idx = (preview.color_idx + 1) % color_count;
                preview.toml_scroll = 0;
            }
            KeyCode::Home => {
                if preview.show_toml {
                    preview.toml_scroll = 0;
                } else {
                    preview.style_idx = 0;
                }
            }
            KeyCode::End => {
                if preview.show_toml {
                    preview.toml_scroll = max_toml;
                } else {
                    preview.style_idx = style_count - 1;
                }
            }
            KeyCode::Char('t' | 'T') => {
                preview.time = !preview.time;
            }
            KeyCode::Char('g' | 'G') => {
                preview.git = !preview.git;
            }
            KeyCode::Char('s' | 'S') => {
                preview.shell_idx = (preview.shell_idx + 1) % SHELLS.len();
            }
            KeyCode::Char('v' | 'V') | KeyCode::Tab => {
                preview.show_toml = !preview.show_toml;
                preview.toml_scroll = 0;
            }
            KeyCode::PageUp if preview.show_toml => {
                preview.toml_scroll = preview.toml_scroll.saturating_sub(8);
            }
            KeyCode::PageDown if preview.show_toml => {
                preview.toml_scroll = preview.toml_scroll.saturating_add(8).min(max_toml);
            }
            KeyCode::Enter | KeyCode::Char('a' | 'A') => {
                return self.apply_prompt_preview();
            }
            KeyCode::Char('c' | 'C') => {
                let text = if preview.show_toml {
                    crate::ops::terminal::render(&preview.options()).unwrap_or_default()
                } else {
                    let params = preview.parameters();
                    if let Ok(spec) = crate::actions::find("terminal.apply") {
                        super::preview::single_words(&spec, &params)
                            .iter()
                            .map(|w| w.shown().into_owned())
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else {
                        crate::ops::terminal::render(&preview.options()).unwrap_or_default()
                    }
                };
                return Ok(self.copy_plain(text));
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn confirm_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let density = self.resolved_density(self.viewport);
        let confirm = self.confirmation.as_mut().expect("active confirm");
        let geom = ConfirmationGeometry::compute(self.viewport, confirm, &self.ctx.i18n, density);
        let page = geom.body.height.saturating_sub(1).max(1);
        let max_scroll = geom.max_scroll();

        let answer = match key.code {
            KeyCode::Esc | KeyCode::Char('n' | 'N') => Some(false),
            KeyCode::Char('y' | 'Y') => Some(true),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                confirm.accept = !confirm.accept;
                None
            }
            KeyCode::Char('c' | 'C') => {
                let text = confirm.text.clone();
                return Ok(self.copy_plain(text));
            }
            KeyCode::Char('v' | 'V') => {
                let content = confirm.text.clone();
                let title = self.ctx.i18n.text("ui.confirm").to_owned();
                return Ok(Effect::PlainView { title, content });
            }
            KeyCode::Enter => Some(confirm.accept),
            KeyCode::Up | KeyCode::Char('k') => {
                confirm.scroll = confirm.scroll.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                confirm.scroll = confirm.scroll.saturating_add(1).min(max_scroll);
                None
            }
            KeyCode::PageUp => {
                confirm.scroll = confirm.scroll.saturating_sub(page);
                None
            }
            KeyCode::PageDown => {
                confirm.scroll = confirm.scroll.saturating_add(page).min(max_scroll);
                None
            }
            _ => None,
        };

        if let Some(answer) = answer {
            let confirm = self.confirmation.take().expect("active confirm");
            let _ = confirm.reply.send(answer);
        }
        Ok(Effect::None)
    }

    fn log_key(&mut self, code: KeyCode, modal: bool) {
        let page = i32::from(
            layout::log_body(self, self.viewport)
                .height
                .saturating_sub(1)
                .max(1),
        );
        let delta = match code {
            KeyCode::PageUp => page,
            KeyCode::PageDown => -page,
            KeyCode::Up | KeyCode::Char('k') if modal => 1,
            KeyCode::Down | KeyCode::Char('j') if modal => -1,
            KeyCode::Home if modal => i32::MAX,
            KeyCode::End if modal => i32::MIN,
            _ => return,
        };
        self.scroll_logs(delta);
    }

    pub(super) fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        self.hover = Some(point);
        if !layout::fits(area) {
            return Ok(Effect::None);
        }

        // Active border-free pane drag selection: strictly clamped inside the pane's content bounds.
        match event.kind {
            MouseEventKind::Moved => return Ok(Effect::None),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = &mut self.drag_select
                    && sel.dragging
                {
                    sel.focus = DragSelection::clamp(sel.bounds, point);
                }
                return Ok(Effect::None);
            }
            MouseEventKind::Drag(_) => return Ok(Effect::None),
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(mut sel) = self.drag_select
                    && sel.dragging
                {
                    sel.dragging = false;
                    sel.focus = DragSelection::clamp(sel.bounds, point);
                    if sel.has_range() {
                        let extracted = self
                            .selectable_panes
                            .borrow()
                            .iter()
                            .rev()
                            .find(|p| p.pane == sel.pane)
                            .map(|p| selection::extract_text(p, &sel))
                            .unwrap_or_default();
                        self.drag_select = Some(sel);
                        if !extracted.is_empty() {
                            return Ok(self.copy_selection(extracted));
                        }
                        return Ok(Effect::None);
                    }
                    self.drag_select = None;
                    if sel.pane == TextPane::Inspector
                        && self.last_cmd_rect.get().is_some_and(|r| r.contains(point))
                    {
                        return Ok(self.copy_current_command());
                    }
                }
                return Ok(Effect::None);
            }
            MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown => {
                self.drag_select = None;
            }
            _ => {}
        }

        let density = self.resolved_density(area);
        let slop = density.slop_enabled();

        // 1. Confirmation (never dismissed by clicking outside!)
        if let Some(confirm) = &mut self.confirmation {
            let geom = ConfirmationGeometry::compute(area, confirm, &self.ctx.i18n, density);
            match event.kind {
                MouseEventKind::ScrollUp => confirm.scroll = confirm.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => {
                    confirm.scroll = confirm.scroll.saturating_add(3).min(geom.max_scroll());
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let answer = hit_best(
                        [(geom.buttons[0], false), (geom.buttons[1], true)],
                        point,
                        slop,
                    );
                    if let Some(answer) = answer {
                        let confirm = self.confirmation.take().expect("confirm");
                        let _ = confirm.reply.send(answer);
                        return Ok(Effect::None);
                    }
                    if geom.body.contains(point) {
                        self.drag_select =
                            Some(DragSelection::start(TextPane::Confirm, geom.body, point));
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 2. Loading dialog
        if self.loading.is_some() {
            return Ok(Effect::None);
        }

        // 2b. Transient SOCKS5 / 5h masked credential copy modal
        if let Some(modal) = &self.socks_secret {
            let geom = SocksSecretGeometry::compute(
                area,
                &modal.creds,
                &self.ctx.i18n,
                self.theme.active.flags.ascii,
                density,
            );
            let max_s = super::render::socks_secret_max_scroll(self, area);
            match event.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(m) = self.socks_secret.as_mut() {
                        m.scroll = m.scroll.min(max_s).saturating_sub(2);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(m) = self.socks_secret.as_mut() {
                        m.scroll = m.scroll.saturating_add(2).min(max_s);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let btn = hit_best(
                        [
                            (geom.btn_close, 0u8),
                            (geom.btn_socks5h, 1u8),
                            (geom.btn_socks5, 2u8),
                            (geom.btn_password, 3u8),
                            (geom.btn_curl, 4u8),
                        ],
                        point,
                        slop,
                    );
                    if !geom.modal.contains(point) || btn == Some(0) {
                        self.close_socks_secret();
                    } else if btn == Some(1) {
                        let uri = modal.creds.socks5h_uri.clone();
                        return Ok(
                            self.copy_socks_secret_field(uri, "ui.socks_secret_field_socks5h")
                        );
                    } else if btn == Some(2) {
                        let uri = modal.creds.socks5_uri.clone();
                        return Ok(
                            self.copy_socks_secret_field(uri, "ui.socks_secret_field_socks5")
                        );
                    } else if btn == Some(3) {
                        return Ok(self.copy_socks_secret_password());
                    } else if btn == Some(4) {
                        let cmd = modal.creds.curl_cmd.clone();
                        return Ok(self.copy_socks_secret_field(cmd, "ui.socks_secret_field_curl"));
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 2c. Transient MCP access & Bearer token copy modal
        if let Some(modal) = &self.mcp_access {
            use crate::mcp::config::{ClientFormat, TransportMode};
            let transport = modal.transport;
            let has_token = modal.access.token.is_some();
            let geom = McpAccessGeometry::compute(
                area,
                transport,
                has_token,
                &self.ctx.i18n,
                self.theme.active.flags.ascii,
                density,
            );
            let max_s = super::render::mcp_access_max_scroll(self, area);
            match event.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(m) = self.mcp_access.as_mut() {
                        m.scroll = m.scroll.min(max_s).saturating_sub(2);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(m) = self.mcp_access.as_mut() {
                        m.scroll = m.scroll.saturating_add(2).min(max_s);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let btn = hit_best(
                        [
                            (geom.btn_close, 0u8),
                            (geom.btn_mode, 1u8),
                            (geom.btn_json, 2u8),
                            (geom.btn_claude, 3u8),
                            (geom.btn_codex, 4u8),
                            (geom.btn_secret, 5u8),
                            (geom.btn_rotate, 6u8),
                        ],
                        point,
                        slop,
                    );
                    if !geom.modal.contains(point) || btn == Some(0) {
                        self.close_mcp_access();
                    } else if btn == Some(1) {
                        if let Some(m) = self.mcp_access.as_mut() {
                            m.transport = m.transport.next();
                            m.scroll = 0;
                        }
                    } else if btn == Some(2) {
                        let text = modal.access.copy_snippet(transport, ClientFormat::Json);
                        return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_json"));
                    } else if btn == Some(3) {
                        let text = modal.access.copy_snippet(transport, ClientFormat::Claude);
                        return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_claude"));
                    } else if btn == Some(4) {
                        let text = modal.access.copy_snippet(transport, ClientFormat::Codex);
                        return Ok(self.copy_mcp_field(text, "ui.mcp_access_field_codex"));
                    } else if btn == Some(5) {
                        if transport == TransportMode::Http
                            && let Some(tok) = modal.access.token.clone()
                        {
                            return Ok(self.copy_mcp_field(tok, "ui.mcp_access_field_token"));
                        }
                        let tunnel = modal.access.ssh_tunnel_cmd();
                        return Ok(self.copy_mcp_field(tunnel, "ui.mcp_access_field_tunnel"));
                    } else if btn == Some(6) {
                        self.rotate_mcp_access_token();
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 3. Picker
        if self.form.as_ref().is_some_and(|f| f.picker.is_some()) {
            let (geom, picked, count) = {
                let form = self.form.as_ref().unwrap();
                let picker = form.picker.as_ref().unwrap();
                let matches = picker.matches(form.choices(form.index));
                let geom = PickerGeometry::compute(area, matches.len(), picker.cursor, density);
                let picked = geom
                    .item_at(point, matches.len())
                    .map(|idx| (idx, matches[idx].value.clone()));
                (geom, picked, matches.len())
            };
            match event.kind {
                MouseEventKind::ScrollUp if count > 0 => {
                    let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                    picker.cursor = picker.cursor.saturating_sub(1);
                }
                MouseEventKind::ScrollDown if count > 0 => {
                    let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                    picker.cursor = (picker.cursor + 1).min(count - 1);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let page = (geom.list.height / geom.row_height.max(1)).max(1) as usize;
                    if !geom.modal.contains(point) {
                        self.close_picker();
                    } else if hit_opt(geom.prev_btn, point, slop) && count > 0 {
                        let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                        picker.cursor = picker.cursor.saturating_sub(page);
                    } else if hit_opt(geom.next_btn, point, slop) && count > 0 {
                        let picker = self.form.as_mut().unwrap().picker.as_mut().unwrap();
                        picker.cursor = (picker.cursor + page).min(count - 1);
                    } else if let Some((_, value)) = picked {
                        return self.pick_choice(value);
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 4. Form
        if let Some(form) = &self.form {
            let geom = FormGeometry::compute(area, form, self);
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if !geom.modal.contains(point) {
                    self.form = None;
                    return Ok(Effect::None);
                }
                match hit_best(
                    [(geom.buttons[0], 0u8), (geom.buttons[1], 1u8)],
                    point,
                    slop,
                ) {
                    Some(0) => return self.submit_form(),
                    Some(1) => {
                        self.form = None;
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
                if let Some((fi, ci)) = geom.chip_at(point, slop) {
                    let form = self.form.as_mut().unwrap();
                    form.index = fi;
                    form.values[fi].replace(form.spec.fields[fi].choices[ci].into());
                } else if let Some(fi) = geom.field_at(point) {
                    let form = self.form.as_mut().unwrap();
                    form.index = fi;
                    if form.listed(fi) {
                        form.open_picker(String::new(), false);
                    } else if !form.chip_field(fi)
                        && !form.preset_chips(fi)
                        && !form.spec.fields[fi].choices.is_empty()
                    {
                        self.cycle_field(true);
                    }
                }
            }
            return Ok(Effect::None);
        }

        // 4b. Interactive Terminal Prompt Preview Studio
        if let Some(preview_ref) = &self.prompt_preview {
            let geom = PromptPreviewGeometry::compute_for(
                area,
                &self.ctx.i18n,
                preview_ref.style_idx,
                preview_ref.color_idx,
            );
            match event.kind {
                MouseEventKind::ScrollUp => {
                    let preview = self.prompt_preview.as_mut().unwrap();
                    if preview.show_toml && geom.preview_box.contains(point) {
                        preview.toml_scroll = preview.toml_scroll.saturating_sub(3);
                    } else if geom.left_pane.contains(point) {
                        let count = crate::ops::terminal::STYLES.len();
                        preview.style_idx = (preview.style_idx + count - 1) % count;
                        preview.toml_scroll = 0;
                    }
                }
                MouseEventKind::ScrollDown => {
                    let max_toml = self.prompt_preview_max_scroll(area);
                    let preview = self.prompt_preview.as_mut().unwrap();
                    if preview.show_toml && geom.preview_box.contains(point) {
                        preview.toml_scroll = preview.toml_scroll.saturating_add(3).min(max_toml);
                    } else if geom.left_pane.contains(point) {
                        let count = crate::ops::terminal::STYLES.len();
                        preview.style_idx = (preview.style_idx + 1) % count;
                        preview.toml_scroll = 0;
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if !geom.modal.contains(point) || geom.close_btn.contains(point) {
                        self.prompt_preview = None;
                        return Ok(Effect::None);
                    }
                    if geom.apply_btn.contains(point) {
                        return self.apply_prompt_preview();
                    }
                    let preview = self.prompt_preview.as_mut().unwrap();
                    if let Some(s_idx) = geom.style_at(point) {
                        preview.style_idx = s_idx;
                        preview.toml_scroll = 0;
                    } else if let Some(c_idx) = geom.color_at(point) {
                        preview.color_idx = c_idx;
                        preview.toml_scroll = 0;
                    } else if geom.time_btn.contains(point) {
                        preview.time = !preview.time;
                    } else if geom.git_btn.contains(point) {
                        preview.git = !preview.git;
                    } else if geom.shell_btn.contains(point) {
                        preview.shell_idx =
                            (preview.shell_idx + 1) % crate::ops::terminal::SHELLS.len();
                    } else if geom.view_btn.contains(point) {
                        preview.show_toml = !preview.show_toml;
                        preview.toml_scroll = 0;
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 4c. Docker-specific modals
        if self.docker_has_modal() {
            return self.docker_modal_mouse(event, area);
        }
        // 4d. Mihomo-specific modals
        if self.clash_has_modal() {
            return self.clash_modal_mouse(event, area);
        }

        // 5. Notice & Help (dragging inside Notice selects text cleanly; clicking outside closes)
        if let Some(notice) = &self.notice {
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                let (modal, body) = layout::notice_modal_rects(area, notice);
                if body.contains(point) {
                    self.drag_select = Some(DragSelection::start(TextPane::Notice, body, point));
                } else if !modal.contains(point) {
                    self.notice = None;
                }
            }
            return Ok(Effect::None);
        }
        if self.help {
            let max_s = super::render::help_max_scroll(self, area);
            match event.kind {
                MouseEventKind::ScrollUp => {
                    self.help_scroll = self.help_scroll.min(max_s).saturating_sub(2);
                }
                MouseEventKind::ScrollDown => {
                    self.help_scroll = self.help_scroll.saturating_add(2).min(max_s);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.help = false;
                    self.help_scroll = 0;
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 6. Full logs modal
        if self.log_modal {
            let (modal, body) = layout::full_log_modal(area);
            let btns = layout::full_log_modal_buttons(modal, self);
            match event.kind {
                MouseEventKind::ScrollUp if body.contains(point) => self.scroll_logs(3),
                MouseEventKind::ScrollDown if body.contains(point) => self.scroll_logs(-3),
                MouseEventKind::Down(MouseButton::Left) if btns.copy.contains(point) => {
                    return Ok(self.copy_logs());
                }
                MouseEventKind::Down(MouseButton::Left) if btns.plain.contains(point) => {
                    return Ok(self.open_plain_view_logs());
                }
                MouseEventKind::Down(MouseButton::Left) if body.contains(point) => {
                    self.drag_select = Some(DragSelection::start(TextPane::LogModal, body, point));
                }
                MouseEventKind::Down(MouseButton::Left) if !modal.contains(point) => {
                    self.log_modal = false;
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 7. Command Palette
        if self.command_palette.is_some() {
            let specs = crate::actions::specs_slice();
            let results = {
                let pal = self.command_palette.as_ref().unwrap();
                palette::search(
                    &pal.query.text,
                    specs,
                    &self.ctx.i18n,
                    &self.category_order,
                    &self.records,
                )
            };
            let count = results.count();
            let geom = CommandPaletteGeometry::compute(
                area,
                &results,
                self.command_palette.as_ref().unwrap().cursor,
                density,
            );
            match event.kind {
                MouseEventKind::ScrollUp if count > 0 => {
                    let pal = self.command_palette.as_mut().unwrap();
                    pal.cursor = pal.cursor.saturating_sub(1);
                }
                MouseEventKind::ScrollDown if count > 0 => {
                    let pal = self.command_palette.as_mut().unwrap();
                    pal.cursor = (pal.cursor + 1).min(count - 1);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let page = (geom.list.height / geom.row_height.max(1)).max(1) as usize;
                    if !geom.modal.contains(point) {
                        self.command_palette = None;
                    } else if hit_opt(geom.prev_btn, point, slop) && count > 0 {
                        let pal = self.command_palette.as_mut().unwrap();
                        pal.cursor = pal.cursor.saturating_sub(page);
                    } else if hit_opt(geom.next_btn, point, slop) && count > 0 {
                        let pal = self.command_palette.as_mut().unwrap();
                        pal.cursor = (pal.cursor + page).min(count - 1);
                    } else if let Some(idx) = geom.cursor_at(point, &results) {
                        let item = results.item_at(idx).cloned();
                        self.command_palette = None;
                        return self.execute_palette_item(item, false);
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // Layout regions for main workspace, sidebar, theme drawer, log drawer, and bottom thumb bar
        let regions = LayoutRegions::compute(area, self);

        // 7b. Bottom clickable Touch Thumb Bar (always accessible for one-handed navigation/actions)
        if !regions.thumb_bar.is_empty()
            && event.kind == MouseEventKind::Down(MouseButton::Left)
            && regions.footer.contains(point)
            && let Some(action) = hit_best(
                regions.thumb_bar.iter().map(|b| (b.rect, b.action)),
                point,
                slop,
            )
        {
            return self.handle_thumb_action(action);
        }

        // 8. Compact sidebar overlay
        if self.sidebar_overlay {
            match event.kind {
                MouseEventKind::ScrollUp => {
                    self.sidebar_scroll = None;
                    let order = self.nav_order();
                    let cur = order.iter().position(|t| *t == self.nav);
                    self.nav = order[step(cur, order.len(), false)];
                    self.view_changed = self.now;
                }
                MouseEventKind::ScrollDown => {
                    self.sidebar_scroll = None;
                    let order = self.nav_order();
                    let cur = order.iter().position(|t| *t == self.nav);
                    self.nav = order[step(cur, order.len(), true)];
                    self.view_changed = self.now;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(side) = &regions.sidebar {
                        if side.can_prev && hit_opt(side.prev_btn, point, slop) {
                            let step_rows = side.visible_items.max(1);
                            self.sidebar_scroll = Some(side.scroll.saturating_sub(step_rows));
                        } else if side.can_next && hit_opt(side.next_btn, point, slop) {
                            let step_rows = side.visible_items.max(1);
                            let max_s = side.rows.len().saturating_sub(1);
                            self.sidebar_scroll =
                                Some(side.scroll.saturating_add(step_rows).min(max_s));
                        } else if let Some(target) = side.target_at(point, slop) {
                            self.sidebar_scroll = None;
                            self.goto_nav(target);
                        } else if hit_opt(side.theme_btn, point, slop) {
                            self.sidebar_overlay = false;
                            self.sidebar_scroll = None;
                            self.open_theme_drawer();
                        } else if hit_opt(side.collapse_btn, point, slop)
                            || !side.area.contains(point)
                        {
                            self.sidebar_overlay = false;
                            self.sidebar_scroll = None;
                        }
                    } else {
                        self.sidebar_overlay = false;
                        self.sidebar_scroll = None;
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 9. Theme Drawer clicks & scroll
        if let (Some(drawer_geom), Some(drawer)) = (&regions.theme_drawer, &self.theme_drawer) {
            if drawer_geom.area.contains(point) {
                let count = drawer.active_count();
                let cur = drawer.active_cursor();
                let page =
                    (drawer_geom.list_area.height / drawer_geom.row_height.max(1)).max(1) as usize;
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.preview_theme_cursor(cur.saturating_sub(1));
                    }
                    MouseEventKind::ScrollDown => {
                        self.preview_theme_cursor((cur + 1).min(count.saturating_sub(1)));
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if hit_rect(drawer_geom.tab_palette, point, slop) {
                            self.switch_theme_drawer_tab(super::theme_drawer::DrawerTab::Palette);
                        } else if hit_rect(drawer_geom.tab_style, point, slop) {
                            self.switch_theme_drawer_tab(super::theme_drawer::DrawerTab::Style);
                        } else if drawer_geom.can_prev && hit_opt(drawer_geom.prev_btn, point, slop)
                        {
                            self.preview_theme_cursor(cur.saturating_sub(page));
                        } else if drawer_geom.can_next && hit_opt(drawer_geom.next_btn, point, slop)
                        {
                            self.preview_theme_cursor((cur + page).min(count.saturating_sub(1)));
                        } else if hit_rect(drawer_geom.transparent_toggle, point, false) {
                            self.toggle_transparent_bg(false);
                        } else if hit_rect(drawer_geom.motion_toggle, point, false) {
                            self.toggle_reduced_motion(false);
                        } else if hit_rect(drawer_geom.density_toggle, point, false) {
                            self.cycle_ui_density(false);
                        } else if let Some(idx) = drawer_geom.entry_at(point, count) {
                            let same = idx == cur;
                            self.preview_theme_cursor(idx);
                            if same && (density.is_touch() || self.is_double_click(point)) {
                                self.save_theme_drawer();
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(Effect::None);
            } else if event.kind == MouseEventKind::Down(MouseButton::Left) {
                // Clicking outside theme drawer reverts cleanly without triggering background widgets
                self.cancel_theme_drawer();
                return Ok(Effect::None);
            }
        }

        // 10. TopBar interactive pills (with 1-row downward touch slop grace on small screens)
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && point.y <= regions.topbar.y.saturating_add(u16::from(slop))
        {
            let mut top_targets: Vec<(Rect, u8)> = Vec::with_capacity(3);
            if let Some(r) = regions.topbar_hits.nav_toggle {
                top_targets.push((r, 0));
            }
            if let Some(r) = regions.topbar_hits.running_pill {
                top_targets.push((r, 1));
            }
            if let Some(r) = regions.topbar_hits.lang_btn {
                top_targets.push((r, 2));
            }
            if let Some(hit) = hit_best(top_targets, point, slop) {
                match hit {
                    0 => {
                        self.toggle_sidebar();
                        return Ok(Effect::None);
                    }
                    1 => {
                        self.logs_open = true;
                        return Ok(Effect::None);
                    }
                    2 => {
                        self.cycle_language()?;
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
            }
        }

        // 11. Bottom log drawer scroll, header action pills, and border-free drag selection
        if let Some(logs_rect) = regions.logs
            && logs_rect.contains(point)
        {
            match event.kind {
                MouseEventKind::ScrollUp => self.scroll_logs(3),
                MouseEventKind::ScrollDown => self.scroll_logs(-3),
                MouseEventKind::Down(MouseButton::Left) => {
                    if hit_opt(regions.log_copy_btn, point, slop) {
                        return Ok(self.copy_logs());
                    }
                    if hit_opt(regions.log_plain_btn, point, slop) {
                        return Ok(self.open_plain_view_logs());
                    }
                    if hit_opt(regions.log_expand_btn, point, slop) {
                        self.log_modal = true;
                    } else if regions.log_body.contains(point) {
                        self.drag_select = Some(DragSelection::start(
                            TextPane::LogDrawer,
                            regions.log_body,
                            point,
                        ));
                    }
                }
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 12. Docked Sidebar clicks
        if let Some(side) = &regions.sidebar
            && side.area.contains(point)
        {
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if side.can_prev && hit_opt(side.prev_btn, point, slop) {
                    let step_rows = side.visible_items.max(1);
                    self.sidebar_scroll = Some(side.scroll.saturating_sub(step_rows));
                } else if side.can_next && hit_opt(side.next_btn, point, slop) {
                    let step_rows = side.visible_items.max(1);
                    let max_s = side.rows.len().saturating_sub(1);
                    self.sidebar_scroll = Some(side.scroll.saturating_add(step_rows).min(max_s));
                } else if let Some(target) = side.target_at(point, slop) {
                    self.goto_nav(target);
                } else if hit_opt(side.collapse_btn, point, slop) {
                    self.toggle_sidebar();
                } else if hit_opt(side.theme_btn, point, slop) {
                    self.open_theme_drawer();
                }
            }
            return Ok(Effect::None);
        }

        // 13. Overview Quick Cards clicks
        if let Some(ov) = &regions.overview
            && event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(idx) = ov.card_at(point, slop)
        {
            self.quick_cursor = idx;
            if self.worker.is_none() {
                return self.activate_quick_card(idx);
            }
            return Ok(Effect::None);
        }

        // 13b. Dedicated Docker Workspace
        if regions.docker.is_some() {
            return self.docker_mouse(event, &regions);
        }
        // 13c. Dedicated Mihomo Workspace
        if regions.clash.is_some() {
            return self.clash_mouse(event, &regions);
        }

        // 14. Workbench Action List & Inspector
        if let Some(wb) = &regions.workbench {
            if wb.inspector_area.contains(point) {
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.inspector_scroll = self.inspector_scroll.saturating_sub(2);
                    }
                    MouseEventKind::ScrollDown => {
                        let max_s = self.max_inspector_scroll.get();
                        self.inspector_scroll = self.inspector_scroll.saturating_add(2).min(max_s);
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if hit_opt(wb.inspector_copy_btn, point, slop) =>
                    {
                        return Ok(self.copy_current_command());
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if self.worker.is_none() && hit_opt(wb.inspector_cta, point, slop) =>
                    {
                        return self.activate();
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let cta_reserve = wb.inspector_cta.map_or(0, |r| r.height + 1);
                        let body_h = wb.inspector_inner.height.saturating_sub(cta_reserve);
                        let body = Rect::new(
                            wb.inspector_inner.x,
                            wb.inspector_inner.y,
                            wb.inspector_inner.width,
                            body_h,
                        );
                        if body.contains(point) {
                            self.drag_select =
                                Some(DragSelection::start(TextPane::Inspector, body, point));
                        }
                    }
                    _ => {}
                }
                return Ok(Effect::None);
            }
            if wb.list_area.contains(point) {
                match event.kind {
                    MouseEventKind::ScrollUp => self.move_row(false),
                    MouseEventKind::ScrollDown => self.move_row(true),
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(group) = self.active_group() {
                            if wb.can_prev && hit_opt(wb.prev_btn, point, slop) {
                                let step_n = wb.selectable_rects.len().max(1);
                                self.positions[group] =
                                    self.positions[group].saturating_sub(step_n);
                                self.inspector_scroll = 0;
                                return Ok(Effect::None);
                            }
                            if wb.can_next && hit_opt(wb.next_btn, point, slop) {
                                let step_n = wb.selectable_rects.len().max(1);
                                let max_p = self.selectable_count(group).saturating_sub(1);
                                self.positions[group] =
                                    self.positions[group].saturating_add(step_n).min(max_p);
                                self.inspector_scroll = 0;
                                return Ok(Effect::None);
                            }
                            if let Some(tab) = wb.section_pill_at(point, slop) {
                                self.set_section_tab(group, tab);
                                return Ok(Effect::None);
                            }
                            let rows = self.rows(group);
                            if let Some(sec_id) = wb.section_row_at(point, &rows)
                                && let Some(secs) = self.group_sections(group)
                                && let Some(idx) = secs.iter().position(|(id, _)| *id == sec_id)
                            {
                                self.set_section_tab(group, Some(idx));
                                return Ok(Effect::None);
                            }
                            if let Some(pos) = wb.selectable_at(point, &rows, slop) {
                                let was_selected = self.selected_in(group) == pos;
                                self.positions[group] = pos;
                                self.inspector_scroll = 0;
                                let double = self.is_double_click(point);
                                if self.worker.is_none() {
                                    // Clicking checkbox region in Optimization toggles immediately
                                    let check_w = if density.is_touch() { 8 } else { 6 };
                                    if group == OPTIMIZATION_GROUP
                                        && point.x <= wb.list_inner.x + check_w
                                        && matches!(self.selected_row(), Some(Row::Action(_)))
                                    {
                                        self.toggle_selected()?;
                                    } else if (was_selected && (density.is_touch() || double))
                                        || matches!(self.selected_row(), Some(Row::Execute))
                                    {
                                        return self.activate();
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(Effect::None)
    }

    fn handle_thumb_action(&mut self, action: ThumbAction) -> Result<Effect> {
        let idle = self.worker.is_none();
        match action {
            ThumbAction::NavToggle => {
                self.toggle_sidebar();
            }
            ThumbAction::OverviewOpt => {
                self.goto_nav(NavTarget::Optimization);
            }
            ThumbAction::OverviewPublicIp => {
                return Ok(Effect::PublicIp);
            }
            ThumbAction::Theme => {
                self.sidebar_overlay = false;
                self.open_theme_drawer();
            }
            ThumbAction::Help => {
                self.sidebar_overlay = false;
                self.help = true;
                self.help_scroll = 0;
            }
            ThumbAction::Search => {
                self.sidebar_overlay = false;
                self.command_palette = Some(palette::CommandPalette::default());
            }
            ThumbAction::Execute if idle => {
                self.sidebar_overlay = false;
                return self.activate();
            }
            ThumbAction::ToggleMark if idle => {
                self.toggle_selected()?;
            }
            ThumbAction::RunOptimization if idle => {
                return self.optimization_jobs();
            }
            ThumbAction::ClashNextTab
            | ThumbAction::ClashCycleMode
            | ThumbAction::ClashPrimary
            | ThumbAction::ClashSecondary => {
                self.sidebar_overlay = false;
                return self.clash_thumb_action(action);
            }
            ThumbAction::DockerNextTab
            | ThumbAction::DockerPrimary
            | ThumbAction::DockerLogs
            | ThumbAction::DockerRefresh => {
                self.sidebar_overlay = false;
                return self.docker_thumb_action(action);
            }
            ThumbAction::LogsToggle => {
                if self.viewport.height.saturating_sub(2) < 16 {
                    self.log_modal = !self.log_modal;
                } else {
                    self.logs_open = !self.logs_open;
                }
            }
            ThumbAction::CancelTask if !idle => {
                self.request_cancel(false);
            }
            ThumbAction::ThemeSave => {
                self.save_theme_drawer();
            }
            ThumbAction::ThemeCancel => {
                self.cancel_theme_drawer();
            }
            ThumbAction::ThemeDensity => {
                self.cycle_ui_density(false);
            }
            ThumbAction::ThemeSwitchTab => {
                if let Some(drawer) = &self.theme_drawer {
                    let next = drawer.tab.toggle();
                    self.switch_theme_drawer_tab(next);
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    pub(in crate::tui) fn is_double_click(&mut self, point: Position) -> bool {
        let now = Instant::now();
        let hit = self.last_click.is_some_and(|(at, prev)| {
            prev == point && now.saturating_duration_since(at) <= DOUBLE_CLICK_WINDOW
        });
        self.last_click = if hit { None } else { Some((now, point)) };
        hit
    }
}
