//! Keyboard and mouse event routing for the dedicated Docker management workspace.

use super::{
    layout::{DockerModalGeometry, QuickBtnKind},
    state::{ComposeSubTab, ContainerSubTab, DaemonEntry, DockerModal, ImageSubTab, MenuAction},
};
use crate::{
    actions,
    error::Result,
    ops::docker::{
        actions::{DockerOp, ExecSpec},
        model::{ContainerState, DockerAvailability, DockerTab},
    },
    tui::{
        layout::LayoutRegions,
        palette,
        state::{App, Effect, NavTarget, Task, Tone},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

impl App {
    pub(in crate::tui) fn docker_has_modal(&self) -> bool {
        self.docker.modal.is_some()
    }

    pub(in crate::tui) fn docker_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let idle = self.worker.is_none();

        // 1. Local Docker modal (ActionMenu / Pull / CreateNetwork / ExecForm / PruneWizard)
        if self.docker.modal.is_some() {
            return self.docker_modal_key(key);
        }

        // 2. Global Ctrl+K Command Palette always remains reachable
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('k' | 'K'))
        {
            self.command_palette = Some(palette::CommandPalette::default());
            return Ok(Effect::None);
        }

        // 3. Log search or list inline filter input capture
        if self.docker.logs.searching {
            return self.docker_log_search_key(key);
        }
        if self.docker.filtering {
            return self.docker_filter_key(key);
        }

        // 4. Fullscreen log viewport mode
        if self.docker.logs.fullscreen {
            return self.docker_fullscreen_log_key(key);
        }

        // 5. Fullscreen inspect overlay (Compact mode)
        if self.docker.inspector_fullscreen {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.docker.inspector_fullscreen = false;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.docker.inspector_scroll = self.docker.inspector_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.docker.inspector_scroll = self.docker.inspector_scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    self.docker.inspector_scroll = self.docker.inspector_scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    self.docker.inspector_scroll = self.docker.inspector_scroll.saturating_add(10);
                }
                KeyCode::Char('J') => self.docker.show_raw_json = !self.docker.show_raw_json,
                KeyCode::Char('S') => self.docker.reveal_secrets = !self.docker.reveal_secrets,
                KeyCode::Char('y') => return Ok(self.docker_copy_current()),
                KeyCode::Char('V' | 'M') => return Ok(self.docker_plain_view()),
                _ => {}
            }
            return Ok(Effect::None);
        }

        // 6. Alt+1..6 switches Docker tabs even in embedded mode
        if key.modifiers.contains(KeyModifiers::ALT)
            && let KeyCode::Char(d @ '1'..='6') = key.code
        {
            let idx = (d as u8 - b'1') as usize;
            self.docker.set_tab(DockerTab::from_index(idx));
            return Ok(Effect::None);
        }

        // 7. Ctrl+U / Ctrl+D scroll right inspector without moving focus
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u' | 'U') => {
                    self.docker_scroll_right(true, 8);
                    return Ok(Effect::None);
                }
                KeyCode::Char('d' | 'D') => {
                    self.docker_scroll_right(false, 8);
                    return Ok(Effect::None);
                }
                _ => {}
            }
        }

        match key.code {
            // Progressive Esc unwind
            KeyCode::Esc => {
                if !idle {
                    self.request_cancel(false);
                } else if !self.docker.filter.text.is_empty()
                    || self.docker.project_filter.is_some()
                {
                    self.docker.filter.clear();
                    self.docker.project_filter = None;
                    self.docker.clamp_cursors();
                    self.toast_key("docker.toast.cleared_filter", Tone::Info);
                } else if self.docker.selected_count() > 0 {
                    self.docker.clear_selection();
                } else if self.docker.fullscreen && !self.docker.launched_standalone {
                    self.docker.fullscreen = false;
                }
            }

            // Global reserved shell controls (Shift-uppercase to avoid Docker lowercase collisions)
            KeyCode::Char('[') => self.toggle_sidebar(),
            KeyCode::Char('T') => self.open_theme_drawer(),
            KeyCode::Char('L') => self.cycle_language()?,
            KeyCode::Char('G') => {
                if self.viewport.height.saturating_sub(2) < 16 {
                    self.log_modal = !self.log_modal;
                } else {
                    self.logs_open = !self.logs_open;
                }
            }
            KeyCode::F(2) => self.log_modal = !self.log_modal,
            KeyCode::Char('V' | 'M') => return Ok(self.docker_plain_view()),
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('q' | 'Q') if idle => return Ok(Effect::Quit),

            // Focus / Zoom toggle
            KeyCode::Char('z' | 'F') => {
                self.docker.fullscreen = !self.docker.fullscreen;
                self.docker.focus_inspector = false;
            }

            // Refresh Docker snapshot + host metrics
            KeyCode::Char('r' | 'R') => {
                return Ok(Effect::DockerRefresh);
            }

            // Tab switching (Left/Right always switch 6 Docker tabs)
            KeyCode::Left => self.docker.cycle_tab(false),
            KeyCode::Right => self.docker.cycle_tab(true),

            // 1..6: direct Docker tabs in Focus mode; 1..9: CqShell categories in embedded mode
            KeyCode::Char(d @ '1'..='6') if self.docker.fullscreen => {
                let idx = (d as u8 - b'1') as usize;
                self.docker.set_tab(DockerTab::from_index(idx));
            }
            KeyCode::Char(d @ '1'..='9') if !self.docker.fullscreen => {
                let n = (d as u8 - b'0') as usize;
                if let Some(&group) = self.category_order.get(n - 1) {
                    self.goto_nav(NavTarget::Group(group));
                }
            }
            KeyCode::Char('H') if !self.docker.fullscreen => self.goto_nav(NavTarget::Overview),
            KeyCode::Char('O' | '0') if !self.docker.fullscreen => {
                self.goto_nav(NavTarget::Optimization);
            }

            // Tab / BackTab: switch CqShell category in embedded mode, or list<->inspector in Focus
            KeyCode::Tab => {
                if self.docker.fullscreen {
                    self.docker.focus_inspector = !self.docker.focus_inspector;
                } else {
                    self.cycle_nav(true);
                }
            }
            KeyCode::BackTab => {
                if self.docker.fullscreen {
                    self.docker.focus_inspector = !self.docker.focus_inspector;
                } else {
                    self.cycle_nav(false);
                }
            }

            // Vertical navigation
            KeyCode::Up | KeyCode::Char('k') => {
                if self.docker.focus_inspector {
                    self.docker_scroll_right(true, 1);
                } else {
                    self.docker.move_cursor(false);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.docker.focus_inspector {
                    self.docker_scroll_right(false, 1);
                } else {
                    self.docker.move_cursor(true);
                }
            }
            KeyCode::Home => {
                if self.docker.focus_inspector {
                    self.docker.inspector_scroll = 0;
                } else {
                    self.docker.set_cursor(0);
                }
            }
            KeyCode::End => {
                if !self.docker.focus_inspector {
                    let count = self.docker.visible_count();
                    self.docker.set_cursor(count.saturating_sub(1));
                }
            }
            KeyCode::PageUp => self.docker_scroll_right(true, 10),
            KeyCode::PageDown => self.docker_scroll_right(false, 10),

            // Inline list filter
            KeyCode::Char('/') if self.docker.tab != DockerTab::Daemon => {
                self.docker.filtering = true;
            }

            // Multi-select
            KeyCode::Char(' ') => self.docker.toggle_multi_select(),

            // Scope toggle (`a`) & Sort cycle (`o`)
            KeyCode::Char('a') => match self.docker.tab {
                DockerTab::Containers => {
                    self.docker.only_running = !self.docker.only_running;
                    self.docker.clamp_cursors();
                }
                DockerTab::Images => {
                    self.docker.show_dangling = !self.docker.show_dangling;
                    self.docker.clamp_cursors();
                }
                _ => {}
            },
            KeyCode::Char('o') => self.docker.cycle_sort(),

            // Inspector sub-tab hotkeys
            KeyCode::Char('i') => match self.docker.tab {
                DockerTab::Containers => {
                    if self.docker.container_sub == ContainerSubTab::Overview
                        && self.viewport.width < 92
                    {
                        self.docker.inspector_fullscreen = true;
                    } else {
                        self.docker.container_sub = ContainerSubTab::Overview;
                        self.docker.inspector_scroll = 0;
                    }
                }
                DockerTab::Compose => {
                    if self.docker.compose_sub == ComposeSubTab::Services
                        && self.viewport.width < 80
                    {
                        self.docker.inspector_fullscreen = true;
                    } else {
                        self.docker.compose_sub = ComposeSubTab::Services;
                        self.docker.inspector_scroll = 0;
                    }
                }
                DockerTab::Images => {
                    if self.docker.image_sub == ImageSubTab::Overview && self.viewport.width < 80 {
                        self.docker.inspector_fullscreen = true;
                    } else {
                        self.docker.image_sub = ImageSubTab::Overview;
                        self.docker.inspector_scroll = 0;
                    }
                }
                _ if self.viewport.width < 80 => {
                    self.docker.inspector_fullscreen = true;
                }
                _ => {}
            },
            KeyCode::Char('l') => match self.docker.tab {
                DockerTab::Containers => {
                    if self.docker.container_sub == ContainerSubTab::Logs {
                        self.docker.logs.fullscreen = true;
                    } else {
                        self.docker.container_sub = ContainerSubTab::Logs;
                    }
                }
                DockerTab::Compose => {
                    if self.docker.compose_sub == ComposeSubTab::Logs {
                        self.docker.logs.fullscreen = true;
                    } else {
                        self.docker.compose_sub = ComposeSubTab::Logs;
                    }
                }
                _ => {}
            },
            KeyCode::Char('m') => match self.docker.tab {
                DockerTab::Containers => {
                    self.docker.container_sub = ContainerSubTab::Stats;
                    self.docker.inspector_scroll = 0;
                }
                DockerTab::Images => {
                    self.docker.image_sub = ImageSubTab::History;
                    self.docker.inspector_scroll = 0;
                }
                DockerTab::Compose => {
                    self.docker.compose_sub = ComposeSubTab::Yaml;
                    self.docker.inspector_scroll = 0;
                }
                _ => {}
            },
            KeyCode::Char('v') => match self.docker.tab {
                DockerTab::Containers => {
                    self.docker.container_sub = ContainerSubTab::Env;
                    self.docker.inspector_scroll = 0;
                }
                DockerTab::Compose => {
                    self.docker.compose_sub = ComposeSubTab::Yaml;
                    self.docker.inspector_scroll = 0;
                }
                _ => {}
            },
            KeyCode::Char('J') => {
                self.docker.show_raw_json = !self.docker.show_raw_json;
                self.docker.inspector_scroll = 0;
            }
            KeyCode::Char('S') => {
                self.docker.reveal_secrets = !self.docker.reveal_secrets;
            }

            // Log quick toggles when embedded logs subtab is visible
            KeyCode::Char('f') if self.docker_in_embedded_logs() => {
                self.docker.logs.follow = !self.docker.logs.follow;
                if self.docker.logs.follow {
                    self.docker.logs.scroll = 0;
                }
            }
            KeyCode::Char('t') if self.docker_in_embedded_logs() => {
                self.docker.logs.timestamps = !self.docker.logs.timestamps;
            }
            KeyCode::Char('w') if self.docker_in_embedded_logs() => {
                self.docker.logs.wrap = !self.docker.logs.wrap;
            }

            // Domain actions
            KeyCode::Enter if idle => return self.docker_activate_enter(),
            KeyCode::Char('s') if idle => return self.docker_quick_start_stop(),
            KeyCode::Char('u') if idle => return self.docker_quick_restart_or_up(),
            KeyCode::Char('p') if idle => return self.docker_quick_pause_or_pull(),
            KeyCode::Char('e') if idle => return self.docker_quick_exec(false),
            KeyCode::Char('E') if idle => return self.docker_quick_exec(true),
            KeyCode::Char('d') | KeyCode::Delete if idle => return self.docker_quick_delete(false),
            KeyCode::Char('D') if idle => return self.docker_quick_delete(true),
            KeyCode::Char('c') if idle && self.docker.tab == DockerTab::Networks => {
                self.docker.modal = Some(DockerModal::network_default());
            }
            KeyCode::Char('c') if self.docker_in_embedded_logs() => {
                self.docker.logs.lines.clear();
                self.docker.logs.scroll = 0;
                self.toast_key("docker.toast.logs_cleared", Tone::Info);
            }
            KeyCode::Char('P') if idle => {
                self.docker.modal = Some(DockerModal::prune_default());
            }
            KeyCode::Char('y') => return Ok(self.docker_copy_current()),
            _ => {}
        }

        Ok(Effect::None)
    }

    fn docker_in_embedded_logs(&self) -> bool {
        (self.docker.tab == DockerTab::Containers
            && self.docker.container_sub == ContainerSubTab::Logs)
            || (self.docker.tab == DockerTab::Compose
                && self.docker.compose_sub == ComposeSubTab::Logs)
    }

    fn docker_scroll_right(&mut self, up: bool, amount: usize) {
        if self.docker_in_embedded_logs() {
            let total = self.docker.logs.filtered_lines().len();
            if up {
                self.docker.logs.follow = false;
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_add(amount).min(total);
            } else {
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_sub(amount);
                if self.docker.logs.scroll == 0 {
                    self.docker.logs.follow = true;
                }
            }
        } else if up {
            self.docker.inspector_scroll =
                self.docker.inspector_scroll.saturating_sub(amount as u16);
        } else {
            self.docker.inspector_scroll =
                self.docker.inspector_scroll.saturating_add(amount as u16);
        }
    }

    fn docker_filter_key(&mut self, key: KeyEvent) -> Result<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.docker.filter.clear();
                self.docker.filtering = false;
                self.docker.clamp_cursors();
            }
            KeyCode::Enter => {
                self.docker.filtering = false;
            }
            KeyCode::Backspace => {
                self.docker.filter.backspace();
                self.docker.clamp_cursors();
            }
            KeyCode::Delete => {
                self.docker.filter.delete();
                self.docker.clamp_cursors();
            }
            KeyCode::Left => self.docker.filter.left(),
            KeyCode::Right => self.docker.filter.right(),
            KeyCode::Home => self.docker.filter.cursor = 0,
            KeyCode::End => self.docker.filter.cursor = self.docker.filter.text.len(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.docker.filter.clear();
                self.docker.clamp_cursors();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.docker.filter.insert(c);
                self.docker.clamp_cursors();
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn docker_log_search_key(&mut self, key: KeyEvent) -> Result<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.docker.logs.search.clear();
                self.docker.logs.searching = false;
            }
            KeyCode::Enter => {
                self.docker.logs.searching = false;
            }
            KeyCode::Backspace => self.docker.logs.search.backspace(),
            KeyCode::Delete => self.docker.logs.search.delete(),
            KeyCode::Left => self.docker.logs.search.left(),
            KeyCode::Right => self.docker.logs.search.right(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.docker.logs.search.clear();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.docker.logs.search.insert(c);
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn docker_fullscreen_log_key(&mut self, key: KeyEvent) -> Result<Effect> {
        let total = self.docker.logs.filtered_lines().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('l' | 'q') => {
                self.docker.logs.fullscreen = false;
            }
            KeyCode::Char('f') => {
                self.docker.logs.follow = !self.docker.logs.follow;
                if self.docker.logs.follow {
                    self.docker.logs.scroll = 0;
                }
            }
            KeyCode::Char('t') => {
                self.docker.logs.timestamps = !self.docker.logs.timestamps;
            }
            KeyCode::Char('w') => {
                self.docker.logs.wrap = !self.docker.logs.wrap;
            }
            KeyCode::Char('c') => {
                self.docker.logs.lines.clear();
                self.docker.logs.scroll = 0;
                self.toast_key("docker.toast.logs_cleared", Tone::Info);
            }
            KeyCode::Char('/') => {
                self.docker.logs.searching = true;
            }
            KeyCode::Char('y') => {
                let text = self.docker.logs.plain_export();
                if text.is_empty() {
                    self.toast_key("toast.no_logs_to_copy", Tone::Warning);
                    return Ok(Effect::None);
                }
                let count = text.lines().count();
                self.toast(
                    self.ctx
                        .i18n
                        .format("toast.copied_logs", &[("count", &count.to_string())]),
                    Tone::Success,
                );
                return Ok(Effect::Copy(text));
            }
            KeyCode::Char('V' | 'M') => return Ok(self.docker_plain_view()),
            KeyCode::Up | KeyCode::Char('k') => {
                self.docker.logs.follow = false;
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_add(1).min(total);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_sub(1);
                if self.docker.logs.scroll == 0 {
                    self.docker.logs.follow = true;
                }
            }
            KeyCode::PageUp => {
                self.docker.logs.follow = false;
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_add(14).min(total);
            }
            KeyCode::PageDown => {
                self.docker.logs.scroll = self.docker.logs.scroll.saturating_sub(14);
                if self.docker.logs.scroll == 0 {
                    self.docker.logs.follow = true;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.docker.logs.follow = false;
                self.docker.logs.scroll = total;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.docker.logs.scroll = 0;
                self.docker.logs.follow = true;
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn docker_activate_enter(&mut self) -> Result<Effect> {
        // Empty-state primary CTAs when Docker isn't installed or daemon is stopped
        if self.docker.tab != DockerTab::Daemon {
            match &self.docker.snapshot.availability {
                DockerAvailability::NotInstalled => {
                    return self.trigger_cqs_action_by_id("docker.install");
                }
                DockerAvailability::DaemonStopped(_) => {
                    return self.run_docker_op(DockerOp::ServiceStart);
                }
                DockerAvailability::Ready => {}
                _ => return Ok(Effect::None),
            }
        }

        if self.docker.tab == DockerTab::Daemon {
            return match self.docker.selected_daemon_entry() {
                DaemonEntry::ServiceStart => self.run_docker_op(DockerOp::ServiceStart),
                DaemonEntry::ServiceStop => self.run_docker_op(DockerOp::ServiceStop),
                DaemonEntry::ServiceRestart => self.run_docker_op(DockerOp::ServiceRestart),
                DaemonEntry::PruneWizard => {
                    self.docker.modal = Some(DockerModal::prune_default());
                    Ok(Effect::None)
                }
                DaemonEntry::CqsAction(id) => self.trigger_cqs_action_by_id(id),
            };
        }

        if let Some(menu) = self.docker.build_action_menu() {
            self.docker.modal = Some(menu);
        } else {
            self.toast_key("docker.toast.no_selection", Tone::Warning);
        }
        Ok(Effect::None)
    }

    fn docker_quick_start_stop(&mut self) -> Result<Effect> {
        if !self.docker.snapshot.availability.is_ready() {
            return Ok(Effect::None);
        }
        match self.docker.tab {
            DockerTab::Containers => {
                let Some(c) = self.docker.selected_container() else {
                    return Ok(Effect::None);
                };
                let running = c.state.is_running();
                let Some((ids, names)) = self.docker.container_targets() else {
                    return Ok(Effect::None);
                };
                let op = if running {
                    DockerOp::ContainerStop { ids, names }
                } else {
                    DockerOp::ContainerStart { ids, names }
                };
                self.run_docker_op(op)
            }
            DockerTab::Compose => {
                let Some(p) = self.docker.selected_compose().cloned() else {
                    return Ok(Effect::None);
                };
                let op = if p.running_count > 0 {
                    DockerOp::ComposeStop {
                        project: p.name,
                        config_files: p.config_files,
                    }
                } else {
                    DockerOp::ComposeStart {
                        project: p.name,
                        config_files: p.config_files,
                    }
                };
                self.run_docker_op(op)
            }
            _ => Ok(Effect::None),
        }
    }

    fn docker_quick_restart_or_up(&mut self) -> Result<Effect> {
        if !self.docker.snapshot.availability.is_ready() {
            return Ok(Effect::None);
        }
        match self.docker.tab {
            DockerTab::Containers => {
                let Some((ids, names)) = self.docker.container_targets() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ContainerRestart { ids, names })
            }
            DockerTab::Compose => {
                let Some(p) = self.docker.selected_compose().cloned() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ComposeUp {
                    project: p.name,
                    config_files: p.config_files,
                })
            }
            _ => Ok(Effect::None),
        }
    }

    fn docker_quick_pause_or_pull(&mut self) -> Result<Effect> {
        if !self.docker.snapshot.availability.is_ready() {
            return Ok(Effect::None);
        }
        match self.docker.tab {
            DockerTab::Containers => {
                let Some(c) = self.docker.selected_container() else {
                    return Ok(Effect::None);
                };
                let state = c.state;
                let Some((ids, names)) = self.docker.container_targets() else {
                    return Ok(Effect::None);
                };
                if state == ContainerState::Paused {
                    self.run_docker_op(DockerOp::ContainerUnpause { ids, names })
                } else if state.is_running() {
                    self.run_docker_op(DockerOp::ContainerPause { ids, names })
                } else {
                    self.toast_key("docker.toast.not_running", Tone::Warning);
                    Ok(Effect::None)
                }
            }
            DockerTab::Images => {
                self.docker.modal = Some(DockerModal::pull_default());
                Ok(Effect::None)
            }
            DockerTab::Compose => {
                let Some(p) = self.docker.selected_compose().cloned() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ComposePull {
                    project: p.name,
                    config_files: p.config_files,
                })
            }
            _ => Ok(Effect::None),
        }
    }

    fn docker_quick_exec(&mut self, custom: bool) -> Result<Effect> {
        if self.docker.tab != DockerTab::Containers || !self.docker.snapshot.availability.is_ready()
        {
            return Ok(Effect::None);
        }
        let Some(c) = self.docker.selected_container().cloned() else {
            return Ok(Effect::None);
        };
        if !c.state.is_running() {
            self.toast_key("docker.toast.not_running", Tone::Warning);
            return Ok(Effect::None);
        }
        if custom {
            self.docker.modal = Some(DockerModal::exec_default(c.id, c.name));
            Ok(Effect::None)
        } else if let Some(spec) = self.docker.quick_exec_spec() {
            Ok(Effect::DockerExec(spec))
        } else {
            Ok(Effect::None)
        }
    }

    fn docker_quick_delete(&mut self, force_or_volumes: bool) -> Result<Effect> {
        if !self.docker.snapshot.availability.is_ready() {
            return Ok(Effect::None);
        }
        match self.docker.tab {
            DockerTab::Containers => {
                let Some(c) = self.docker.selected_container() else {
                    return Ok(Effect::None);
                };
                let running = c.state.is_running();
                let Some((ids, names)) = self.docker.container_targets() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ContainerRemove {
                    ids,
                    names,
                    force: force_or_volumes || running,
                })
            }
            DockerTab::Compose => {
                let Some(p) = self.docker.selected_compose().cloned() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ComposeDown {
                    project: p.name,
                    config_files: p.config_files,
                    volumes: force_or_volumes,
                })
            }
            DockerTab::Images => {
                let Some((ids, labels)) = self.docker.image_targets() else {
                    return Ok(Effect::None);
                };
                self.run_docker_op(DockerOp::ImageRemove {
                    ids,
                    labels,
                    force: force_or_volumes,
                })
            }
            DockerTab::Volumes => {
                let Some(vol) = self.docker.selected_volume() else {
                    return Ok(Effect::None);
                };
                let Some(names) = self.docker.volume_targets() else {
                    return Ok(Effect::None);
                };
                if names.len() == 1 && !vol.used_by.is_empty() {
                    self.toast_key("docker.toast.volume_in_use", Tone::Warning);
                    return Ok(Effect::None);
                }
                self.run_docker_op(DockerOp::VolumeRemove { names })
            }
            DockerTab::Networks => {
                let Some(net) = self.docker.selected_network() else {
                    return Ok(Effect::None);
                };
                let Some((ids, names)) = self.docker.network_targets() else {
                    return Ok(Effect::None);
                };
                if ids.len() == 1 && net.builtin {
                    self.toast_key("docker.toast.builtin_protected", Tone::Warning);
                    return Ok(Effect::None);
                }
                self.run_docker_op(DockerOp::NetworkRemove { ids, names })
            }
            DockerTab::Daemon => Ok(Effect::None),
        }
    }

    fn docker_copy_current(&mut self) -> Effect {
        if self.docker_in_embedded_logs() {
            let text = self.docker.logs.plain_export();
            if !text.is_empty() {
                let count = text.lines().count();
                self.toast(
                    self.ctx
                        .i18n
                        .format("toast.copied_logs", &[("count", &count.to_string())]),
                    Tone::Success,
                );
                return Effect::Copy(text);
            }
        }
        if let Some(cmd) = self.docker.current_cli_hint() {
            self.toast(
                self.ctx
                    .i18n
                    .format("toast.copied_cmd", &[("detail", &cmd)]),
                Tone::Success,
            );
            Effect::Copy(cmd)
        } else {
            self.toast_key("docker.toast.no_selection", Tone::Warning);
            Effect::None
        }
    }

    fn run_docker_op(&mut self, op: DockerOp) -> Result<Effect> {
        op.validate()?;
        if let Some(err) = &self.platform_error {
            return Err(err.clone());
        }
        if op.requires_root() && !self.root {
            return Err(crate::error::Error::new("error.root", "sudo cqs"));
        }
        Ok(Effect::Run(Task::DockerOp(op)))
    }

    fn trigger_cqs_action_by_id(&mut self, id: &str) -> Result<Effect> {
        let spec = actions::find(id)?;
        self.activate_action(spec)
    }

    fn docker_plain_view(&mut self) -> Effect {
        let title = match self.docker.tab {
            DockerTab::Containers => self
                .docker
                .selected_container()
                .map(|c| format!("Docker: {}", c.name))
                .unwrap_or_else(|| "Docker".into()),
            DockerTab::Compose => self
                .docker
                .selected_compose()
                .map(|p| format!("Compose: {}", p.name))
                .unwrap_or_else(|| "Compose".into()),
            _ => "Docker".into(),
        };
        let content = if self.docker.logs.fullscreen || self.docker_in_embedded_logs() {
            self.docker.logs.plain_export()
        } else if let Some(c) = self.docker.selected_container()
            && let Some(insp) = self.docker.container_inspects.get(&c.id)
        {
            insp.raw_json.clone()
        } else {
            self.docker.current_cli_hint().unwrap_or_default()
        };
        if content.is_empty() {
            self.toast_key("toast.no_logs_to_copy", Tone::Warning);
            Effect::None
        } else {
            Effect::PlainView { title, content }
        }
    }

    fn docker_modal_key(&mut self, key: KeyEvent) -> Result<Effect> {
        if key.code == KeyCode::Esc {
            self.docker.modal = None;
            return Ok(Effect::None);
        }

        match self.docker.modal.as_mut().expect("active docker modal") {
            DockerModal::ActionMenu { items, cursor, .. } => {
                let len = items.len();
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') if len > 0 => {
                        *cursor = (*cursor + len - 1) % len;
                    }
                    KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                        *cursor = (*cursor + 1) % len;
                    }
                    KeyCode::Enter if len > 0 => {
                        let chosen = items[*cursor].clone();
                        self.docker.modal = None;
                        return self.execute_menu_item(chosen);
                    }
                    KeyCode::Char(c) => {
                        let key_str = c.to_string();
                        if let Some(item) = items.iter().find(|it| it.shortcut == key_str).cloned()
                        {
                            self.docker.modal = None;
                            return self.execute_menu_item(item);
                        }
                    }
                    _ => {}
                }
            }
            DockerModal::PullImage { reference, error } => {
                *error = None;
                match key.code {
                    KeyCode::Enter => {
                        let op = DockerOp::ImagePull {
                            reference: reference.text.trim().to_owned(),
                        };
                        match op.validate() {
                            Ok(()) => {
                                self.docker.modal = None;
                                return self.run_docker_op(op);
                            }
                            Err(e) => *error = Some(self.ctx.i18n.error(&e)),
                        }
                    }
                    _ => edit_buffer(reference, key),
                }
            }
            DockerModal::CreateNetwork {
                name,
                subnet,
                ipv6,
                field,
                error,
            } => {
                *error = None;
                match key.code {
                    KeyCode::Tab | KeyCode::Down => *field = (*field + 1) % 3,
                    KeyCode::BackTab | KeyCode::Up => *field = (*field + 2) % 3,
                    KeyCode::Char(' ') if *field == 2 => *ipv6 = !*ipv6,
                    KeyCode::Enter if *field < 2 => *field += 1,
                    KeyCode::Enter => {
                        let op = DockerOp::NetworkCreate {
                            name: name.text.trim().to_owned(),
                            subnet: subnet.text.trim().to_owned(),
                            ipv6: *ipv6,
                        };
                        match op.validate() {
                            Ok(()) => {
                                self.docker.modal = None;
                                return self.run_docker_op(op);
                            }
                            Err(e) => *error = Some(self.ctx.i18n.error(&e)),
                        }
                    }
                    _ if *field == 0 => edit_buffer(name, key),
                    _ if *field == 1 => edit_buffer(subnet, key),
                    _ => {}
                }
            }
            DockerModal::ExecForm {
                container_id,
                container_name,
                shell,
                user,
                workdir,
                field,
                error,
            } => {
                *error = None;
                match key.code {
                    KeyCode::Tab | KeyCode::Down => *field = (*field + 1) % 3,
                    KeyCode::BackTab | KeyCode::Up => *field = (*field + 2) % 3,
                    KeyCode::Enter if *field < 2 => *field += 1,
                    KeyCode::Enter => {
                        let spec = ExecSpec {
                            container_id: container_id.clone(),
                            container_name: container_name.clone(),
                            shell: shell.text.trim().to_owned(),
                            user: user.text.trim().to_owned(),
                            workdir: workdir.text.trim().to_owned(),
                        };
                        let probe = if spec.shell.is_empty() || spec.shell == "auto" {
                            "/bin/sh"
                        } else {
                            spec.shell.as_str()
                        };
                        match crate::ops::docker::actions::build_exec_args(&spec, probe) {
                            Ok(_) => {
                                self.docker.modal = None;
                                return Ok(Effect::DockerExec(spec));
                            }
                            Err(e) => *error = Some(self.ctx.i18n.error(&e)),
                        }
                    }
                    _ if *field == 0 => edit_buffer(shell, key),
                    _ if *field == 1 => edit_buffer(user, key),
                    _ => edit_buffer(workdir, key),
                }
            }
            DockerModal::PruneWizard {
                containers,
                images,
                all_unused_images,
                volumes,
                networks,
                build_cache,
                cursor,
            } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *cursor = (*cursor + 5) % 6,
                KeyCode::Down | KeyCode::Char('j') => *cursor = (*cursor + 1) % 6,
                KeyCode::Char(' ') => match *cursor {
                    0 => *containers = !*containers,
                    1 => *images = !*images,
                    2 => {
                        *all_unused_images = !*all_unused_images;
                        if *all_unused_images {
                            *images = true;
                        }
                    }
                    3 => *volumes = !*volumes,
                    4 => *networks = !*networks,
                    _ => *build_cache = !*build_cache,
                },
                KeyCode::Enter => {
                    let op = DockerOp::Prune {
                        containers: *containers,
                        images: *images,
                        all_unused_images: *all_unused_images,
                        volumes: *volumes,
                        networks: *networks,
                        build_cache: *build_cache,
                    };
                    if op.validate().is_ok() {
                        self.docker.modal = None;
                        return self.run_docker_op(op);
                    }
                }
                _ => {}
            },
        }
        Ok(Effect::None)
    }

    fn execute_menu_item(&mut self, item: super::state::ActionMenuItem) -> Result<Effect> {
        if let Some(toast_key) = item.disabled_toast {
            self.toast_key(toast_key, Tone::Warning);
            return Ok(Effect::None);
        }
        match item.action {
            MenuAction::Op(op) => self.run_docker_op(op),
            MenuAction::ExecQuick {
                container_id,
                container_name,
            } => Ok(Effect::DockerExec(ExecSpec {
                container_id,
                container_name,
                shell: "auto".into(),
                user: String::new(),
                workdir: String::new(),
            })),
            MenuAction::ExecCustom {
                container_id,
                container_name,
            } => {
                self.docker.modal = Some(DockerModal::exec_default(container_id, container_name));
                Ok(Effect::None)
            }
            MenuAction::OpenLogs => {
                match self.docker.tab {
                    DockerTab::Containers => self.docker.container_sub = ContainerSubTab::Logs,
                    DockerTab::Compose => self.docker.compose_sub = ComposeSubTab::Logs,
                    _ => {}
                }
                Ok(Effect::None)
            }
            MenuAction::JumpProject(project) => {
                self.docker.project_filter = Some(project.clone());
                self.docker.set_tab(DockerTab::Containers);
                self.toast(
                    self.ctx
                        .i18n
                        .format("docker.toast.filtered_project", &[("name", &project)]),
                    Tone::Info,
                );
                Ok(Effect::None)
            }
            MenuAction::Copy(text) => {
                self.toast(
                    self.ctx
                        .i18n
                        .format("toast.copied_cmd", &[("detail", &text)]),
                    Tone::Success,
                );
                Ok(Effect::Copy(text))
            }
            MenuAction::OpenPrune => {
                self.docker.modal = Some(DockerModal::prune_default());
                Ok(Effect::None)
            }
            MenuAction::OpenPull => {
                self.docker.modal = Some(DockerModal::pull_default());
                Ok(Effect::None)
            }
            MenuAction::OpenCreateNetwork => {
                self.docker.modal = Some(DockerModal::network_default());
                Ok(Effect::None)
            }
        }
    }

    pub(in crate::tui) fn docker_modal_mouse(
        &mut self,
        event: MouseEvent,
        area: Rect,
    ) -> Result<Effect> {
        let point: Position = (event.column, event.row).into();
        let geom = {
            let Some(modal) = &self.docker.modal else {
                return Ok(Effect::None);
            };
            DockerModalGeometry::compute(area, modal)
        };
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(Effect::None);
        }
        if !geom.modal.contains(point) {
            self.docker.modal = None;
            return Ok(Effect::None);
        }
        if geom.cancel_btn.is_some_and(|r| r.contains(point)) {
            self.docker.modal = None;
            return Ok(Effect::None);
        }
        if geom.submit_btn.is_some_and(|r| r.contains(point)) {
            return self.docker_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        }
        if let Some(idx) = geom.rows.iter().position(|r| r.contains(point)) {
            match self.docker.modal.as_mut() {
                Some(DockerModal::ActionMenu { items, cursor, .. }) => {
                    if let Some(chosen) = items.get(idx).cloned() {
                        *cursor = idx;
                        self.docker.modal = None;
                        return self.execute_menu_item(chosen);
                    }
                }
                Some(DockerModal::PruneWizard { cursor, .. }) => {
                    *cursor = idx;
                    return self
                        .docker_modal_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
                }
                Some(DockerModal::CreateNetwork { field, ipv6, .. }) => {
                    *field = idx;
                    if idx == 2 {
                        *ipv6 = !*ipv6;
                    }
                }
                Some(DockerModal::ExecForm { field, .. }) => {
                    *field = idx;
                }
                _ => {}
            }
        }
        Ok(Effect::None)
    }

    pub(in crate::tui) fn docker_mouse(
        &mut self,
        event: MouseEvent,
        regions: &LayoutRegions,
    ) -> Result<Effect> {
        let Some(dk_geom) = &regions.docker else {
            return Ok(Effect::None);
        };
        let point: Position = (event.column, event.row).into();
        let idle = self.worker.is_none();

        match event.kind {
            MouseEventKind::ScrollUp => {
                if dk_geom.fullscreen_view.is_some() || dk_geom.inspector_area.contains(point) {
                    self.docker_scroll_right(true, 3);
                } else if dk_geom.list_area.contains(point) {
                    self.docker.move_cursor(false);
                }
            }
            MouseEventKind::ScrollDown => {
                if dk_geom.fullscreen_view.is_some() || dk_geom.inspector_area.contains(point) {
                    self.docker_scroll_right(false, 3);
                } else if dk_geom.list_area.contains(point) {
                    self.docker.move_cursor(true);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if dk_geom.focus_btn.is_some_and(|r| r.contains(point)) {
                    self.docker.fullscreen = !self.docker.fullscreen;
                    return Ok(Effect::None);
                }
                if let Some(tab) = dk_geom.tab_at(point) {
                    self.docker.set_tab(tab);
                    return Ok(Effect::None);
                }
                if idle && dk_geom.empty_cta.is_some_and(|r| r.contains(point)) {
                    return self.docker_activate_enter();
                }
                if dk_geom.scope_chip.is_some_and(|r| r.contains(point)) {
                    return self.docker_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
                }
                if dk_geom.sort_chip.is_some_and(|r| r.contains(point)) {
                    self.docker.cycle_sort();
                    return Ok(Effect::None);
                }
                if dk_geom.project_chip.is_some_and(|r| r.contains(point)) {
                    self.docker.project_filter = None;
                    self.docker.clamp_cursors();
                    return Ok(Effect::None);
                }
                if let Some(col_idx) = dk_geom.col_at(point) {
                    self.docker.sort_by_column(col_idx);
                    return Ok(Effect::None);
                }
                if let Some(sub_idx) = dk_geom.sub_pill_at(point) {
                    match self.docker.tab {
                        DockerTab::Containers => {
                            self.docker.container_sub = ContainerSubTab::from_index(sub_idx);
                        }
                        DockerTab::Compose => {
                            self.docker.compose_sub = ComposeSubTab::from_index(sub_idx);
                        }
                        DockerTab::Images => {
                            self.docker.image_sub = ImageSubTab::from_index(sub_idx);
                        }
                        _ => {}
                    }
                    self.docker.inspector_scroll = 0;
                    return Ok(Effect::None);
                }
                if idle && let Some(kind) = dk_geom.quick_btn_at(point) {
                    return match kind {
                        QuickBtnKind::StartStop => self.docker_quick_start_stop(),
                        QuickBtnKind::Restart => self.docker_quick_restart_or_up(),
                        QuickBtnKind::Exec => self.docker_quick_exec(false),
                        QuickBtnKind::Logs => {
                            self.docker_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
                        }
                        QuickBtnKind::Pause => self.docker_quick_pause_or_pull(),
                        QuickBtnKind::Delete => self.docker_quick_delete(false),
                        QuickBtnKind::Pull => {
                            self.docker.modal = Some(DockerModal::pull_default());
                            Ok(Effect::None)
                        }
                        QuickBtnKind::CreateNetwork => {
                            self.docker.modal = Some(DockerModal::network_default());
                            Ok(Effect::None)
                        }
                        QuickBtnKind::Prune => {
                            self.docker.modal = Some(DockerModal::prune_default());
                            Ok(Effect::None)
                        }
                        QuickBtnKind::JumpProject => {
                            if let Some(proj) =
                                self.docker.selected_compose().map(|p| p.name.clone())
                            {
                                self.docker.project_filter = Some(proj.clone());
                                self.docker.set_tab(DockerTab::Containers);
                                self.toast(
                                    self.ctx.i18n.format(
                                        "docker.toast.filtered_project",
                                        &[("name", &proj)],
                                    ),
                                    Tone::Info,
                                );
                            }
                            Ok(Effect::None)
                        }
                    };
                }
                if self
                    .docker
                    .last_cmd_rect
                    .get()
                    .is_some_and(|r| r.contains(point))
                {
                    return Ok(self.docker_copy_current());
                }
                let total = self.docker.visible_count();
                if let Some((idx, in_checkbox)) = dk_geom.row_at(point, total) {
                    let is_double = self.is_double_click(point);
                    self.docker.set_cursor(idx);
                    if in_checkbox
                        && !matches!(self.docker.tab, DockerTab::Compose | DockerTab::Daemon)
                    {
                        self.docker.toggle_multi_select();
                    } else if is_double && idle {
                        return self.docker_activate_enter();
                    }
                    return Ok(Effect::None);
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }

    fn toast_key(&mut self, key: &str, tone: Tone) {
        let text = self.ctx.i18n.text(key).to_owned();
        self.toast(text, tone);
    }
}

fn edit_buffer(buf: &mut crate::tui::input::InputBuffer, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => buf.backspace(),
        KeyCode::Delete => buf.delete(),
        KeyCode::Left => buf.left(),
        KeyCode::Right => buf.right(),
        KeyCode::Home => buf.cursor = 0,
        KeyCode::End => buf.cursor = buf.text.len(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buf.clear(),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            buf.insert(c);
        }
        _ => {}
    }
}
