//! Keyboard and mouse event routing following the Overlay Priority Stack:
//! Confirmation -> Loading -> Picker -> Form -> Notice -> Help -> Full Logs ->
//! Command Palette -> Sidebar Overlay -> Theme Drawer -> Main Workspace.

use super::{
    layout::{
        self, CommandPaletteGeometry, ConfirmationGeometry, FormGeometry, LayoutRegions,
        PickerGeometry, PromptPreviewGeometry,
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
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q' | '?') | KeyCode::F(1) => {
                    self.help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(16);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(6);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(6).min(16);
                }
                KeyCode::Home => self.help_scroll = 0,
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
                    self.inspector_scroll = self.inspector_scroll.saturating_add(2);
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
        match key.code {
            KeyCode::Esc | KeyCode::Char('[') | KeyCode::Enter => {
                self.sidebar_overlay = false;
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
        let specs = crate::actions::specs();
        let results = {
            let pal = self.command_palette.as_ref().expect("open palette");
            palette::search(
                &pal.query.text,
                &specs,
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
        );
        let page = geom.list.height.saturating_sub(1).max(1) as usize;

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
            let geom = PickerGeometry::compute(self.viewport, matches.len(), picker.cursor);
            (
                matches.get(picker.cursor).map(|c| c.value.clone()),
                matches.len(),
                geom.list.height.saturating_sub(1).max(1) as usize,
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

    fn form_key(&mut self, key: KeyEvent) -> Result<Effect> {
        if key.code == KeyCode::Esc {
            self.form = None;
            return Ok(Effect::None);
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.submit_form();
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
                    preview.toml_scroll = u16::MAX / 2;
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
                preview.toml_scroll = preview.toml_scroll.saturating_add(8);
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
        let confirm = self.confirmation.as_mut().expect("active confirm");
        let geom = ConfirmationGeometry::compute(self.viewport, confirm, &self.ctx.i18n);
        let page = geom.body.height.saturating_sub(1).max(1);
        let max_scroll = geom.max_scroll();

        let answer = if key.code == KeyCode::Esc {
            Some(false)
        } else if confirm.token == "YES" {
            match key.code {
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
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') => Some(false),
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
            }
        } else {
            match key.code {
                KeyCode::Enter if confirm.input.text == confirm.token => Some(true),
                KeyCode::Up => {
                    confirm.scroll = confirm.scroll.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    confirm.scroll = confirm.scroll.saturating_add(1).min(max_scroll);
                    None
                }
                KeyCode::Backspace => {
                    confirm.input.backspace();
                    None
                }
                KeyCode::Delete => {
                    confirm.input.delete();
                    None
                }
                KeyCode::Left => {
                    confirm.input.left();
                    None
                }
                KeyCode::Right => {
                    confirm.input.right();
                    None
                }
                KeyCode::Home => {
                    confirm.input.cursor = 0;
                    None
                }
                KeyCode::End => {
                    confirm.input.cursor = confirm.input.text.len();
                    None
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    confirm.input.clear();
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
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    confirm.input.insert(c);
                    None
                }
                _ => None,
            }
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

        // 1. Confirmation (never dismissed by clicking outside!)
        if let Some(confirm) = &mut self.confirmation {
            let geom = ConfirmationGeometry::compute(area, confirm, &self.ctx.i18n);
            match event.kind {
                MouseEventKind::ScrollUp => confirm.scroll = confirm.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => {
                    confirm.scroll = confirm.scroll.saturating_add(3).min(geom.max_scroll());
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if confirm.token == "YES" {
                        let answer = if geom.buttons[0].contains(point) {
                            Some(false)
                        } else if geom.buttons[1].contains(point) {
                            Some(true)
                        } else {
                            None
                        };
                        if let Some(answer) = answer {
                            let confirm = self.confirmation.take().expect("confirm");
                            let _ = confirm.reply.send(answer);
                            return Ok(Effect::None);
                        }
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

        // 3. Picker
        if self.form.as_ref().is_some_and(|f| f.picker.is_some()) {
            let (geom, picked, count) = {
                let form = self.form.as_ref().unwrap();
                let picker = form.picker.as_ref().unwrap();
                let matches = picker.matches(form.choices(form.index));
                let geom = PickerGeometry::compute(area, matches.len(), picker.cursor);
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
                    if !geom.modal.contains(point) {
                        self.close_picker();
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
                if geom.buttons[0].contains(point) {
                    return self.submit_form();
                }
                if geom.buttons[1].contains(point) {
                    self.form = None;
                    return Ok(Effect::None);
                }
                if let Some((fi, ci)) = geom.chip_at(point) {
                    let form = self.form.as_mut().unwrap();
                    form.index = fi;
                    form.values[fi].replace(form.spec.fields[fi].choices[ci].into());
                } else if let Some(fi) = geom.field_at(point) {
                    let form = self.form.as_mut().unwrap();
                    form.index = fi;
                    if form.listed(fi) {
                        form.open_picker(String::new(), false);
                    } else if !form.chip_field(fi) && !form.spec.fields[fi].choices.is_empty() {
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
                    let preview = self.prompt_preview.as_mut().unwrap();
                    if preview.show_toml && geom.preview_box.contains(point) {
                        preview.toml_scroll = preview.toml_scroll.saturating_add(3);
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
            match event.kind {
                MouseEventKind::ScrollUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(2);
                }
                MouseEventKind::ScrollDown => {
                    self.help_scroll = self.help_scroll.saturating_add(2).min(16);
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
            let specs = crate::actions::specs();
            let results = {
                let pal = self.command_palette.as_ref().unwrap();
                palette::search(
                    &pal.query.text,
                    &specs,
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
                    if !geom.modal.contains(point) {
                        self.command_palette = None;
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

        // Layout regions for main workspace, sidebar, theme drawer, and log drawer
        let regions = LayoutRegions::compute(area, self);

        // 8. Compact sidebar overlay
        if self.sidebar_overlay {
            match event.kind {
                MouseEventKind::ScrollUp => {
                    let order = self.nav_order();
                    let cur = order.iter().position(|t| *t == self.nav);
                    self.nav = order[step(cur, order.len(), false)];
                    self.view_changed = self.now;
                }
                MouseEventKind::ScrollDown => {
                    let order = self.nav_order();
                    let cur = order.iter().position(|t| *t == self.nav);
                    self.nav = order[step(cur, order.len(), true)];
                    self.view_changed = self.now;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(side) = &regions.sidebar {
                        if let Some(target) = side.target_at(point) {
                            self.goto_nav(target);
                        } else if side.theme_btn.is_some_and(|r| r.contains(point)) {
                            self.sidebar_overlay = false;
                            self.open_theme_drawer();
                        } else {
                            self.sidebar_overlay = false;
                        }
                    } else {
                        self.sidebar_overlay = false;
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
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.preview_theme_cursor(cur.saturating_sub(1));
                    }
                    MouseEventKind::ScrollDown => {
                        self.preview_theme_cursor((cur + 1).min(count.saturating_sub(1)));
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if drawer_geom.tab_palette.contains(point) {
                            self.switch_theme_drawer_tab(super::theme_drawer::DrawerTab::Palette);
                        } else if drawer_geom.tab_style.contains(point) {
                            self.switch_theme_drawer_tab(super::theme_drawer::DrawerTab::Style);
                        } else if drawer_geom.transparent_toggle.contains(point) {
                            self.toggle_transparent_bg(false);
                        } else if drawer_geom.motion_toggle.contains(point) {
                            self.toggle_reduced_motion(false);
                        } else if let Some(idx) = drawer_geom.entry_at(point, count) {
                            let same = idx == cur;
                            self.preview_theme_cursor(idx);
                            if same && self.is_double_click(point) {
                                self.save_theme_drawer();
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(Effect::None);
            } else if event.kind == MouseEventKind::Down(MouseButton::Left) {
                // Clicking outside theme drawer reverts cleanly
                self.cancel_theme_drawer();
            }
        }

        // 10. TopBar interactive pills
        if event.kind == MouseEventKind::Down(MouseButton::Left) && regions.topbar.contains(point) {
            if regions
                .topbar_hits
                .nav_toggle
                .is_some_and(|r| r.contains(point))
            {
                self.toggle_sidebar();
                return Ok(Effect::None);
            }
            if regions
                .topbar_hits
                .running_pill
                .is_some_and(|r| r.contains(point))
            {
                self.logs_open = true;
                return Ok(Effect::None);
            }
            if regions
                .topbar_hits
                .lang_btn
                .is_some_and(|r| r.contains(point))
            {
                self.cycle_language()?;
                return Ok(Effect::None);
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
                    if regions.log_copy_btn.is_some_and(|r| r.contains(point)) {
                        return Ok(self.copy_logs());
                    }
                    if regions.log_plain_btn.is_some_and(|r| r.contains(point)) {
                        return Ok(self.open_plain_view_logs());
                    }
                    if regions.log_expand_btn.is_some_and(|r| r.contains(point)) {
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
                if let Some(target) = side.target_at(point) {
                    self.goto_nav(target);
                } else if side.collapse_btn.is_some_and(|r| r.contains(point)) {
                    self.toggle_sidebar();
                } else if side.theme_btn.is_some_and(|r| r.contains(point)) {
                    self.open_theme_drawer();
                }
            }
            return Ok(Effect::None);
        }

        // 13. Overview Quick Cards clicks
        if let Some(ov) = &regions.overview
            && event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(idx) = ov.card_at(point)
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

        // 14. Workbench Action List & Inspector
        if let Some(wb) = &regions.workbench {
            if wb.inspector_area.contains(point) {
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.inspector_scroll = self.inspector_scroll.saturating_sub(2);
                    }
                    MouseEventKind::ScrollDown => {
                        self.inspector_scroll = self.inspector_scroll.saturating_add(2);
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if wb.inspector_copy_btn.is_some_and(|btn| btn.contains(point)) =>
                    {
                        return Ok(self.copy_current_command());
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if self.worker.is_none()
                            && wb.inspector_cta.is_some_and(|cta| cta.contains(point)) =>
                    {
                        return self.activate();
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let body_h = wb
                            .inspector_inner
                            .height
                            .saturating_sub(if wb.inspector_cta.is_some() { 2 } else { 0 });
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
                            if let Some(tab) = wb.section_pill_at(point) {
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
                            if let Some(pos) = wb.selectable_at(point, &rows) {
                                let was_selected = self.selected_in(group) == pos;
                                self.positions[group] = pos;
                                self.inspector_scroll = 0;
                                let double = self.is_double_click(point);
                                if self.worker.is_none() {
                                    // Clicking checkbox region in Optimization toggles immediately
                                    if group == OPTIMIZATION_GROUP
                                        && point.x <= wb.list_inner.x + 6
                                        && matches!(self.selected_row(), Some(Row::Action(_)))
                                    {
                                        self.toggle_selected()?;
                                    } else if (was_selected && double)
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

    pub(in crate::tui) fn is_double_click(&mut self, point: Position) -> bool {
        let now = Instant::now();
        let hit = self.last_click.is_some_and(|(at, prev)| {
            prev == point && now.saturating_duration_since(at) <= DOUBLE_CLICK_WINDOW
        });
        self.last_click = if hit { None } else { Some((now, point)) };
        hit
    }
}
