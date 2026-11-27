//! Four-tier responsive layout geometry (`Wide`, `Medium`, `Compact`, `Mobile`) and modal
//! hit-testing rectangles. Every view renderer and mouse event handler reads from the same
//! geometry types so visual positions and click targets never drift.

use super::{
    palette,
    state::{App, Confirmation, Form, NavTarget, Row, default_category_order, row_index},
};
use crate::i18n::I18n;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use unicode_width::UnicodeWidthStr;

pub(super) const MIN_WIDTH: u16 = 40;
pub(super) const MIN_HEIGHT: u16 = 16;
const WIDE_MIN: u16 = 112;
const MEDIUM_MIN: u16 = 92;
const COMPACT_MIN: u16 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Breakpoint {
    Wide,
    Medium,
    Compact,
    Mobile,
}

impl Breakpoint {
    pub fn of(width: u16) -> Self {
        if width >= WIDE_MIN {
            Self::Wide
        } else if width >= MEDIUM_MIN {
            Self::Medium
        } else if width >= COMPACT_MIN {
            Self::Compact
        } else {
            Self::Mobile
        }
    }

    pub fn uses_overlay_sidebar(self) -> bool {
        matches!(self, Self::Compact | Self::Mobile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum UiDensityMode {
    #[default]
    Auto,
    Compact,
    Touch,
}

impl UiDensityMode {
    pub fn from_pref(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("compact") => Self::Compact,
            Some("touch") => Self::Touch,
            _ => Self::Auto,
        }
    }

    pub const fn as_pref(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Compact => Some("compact"),
            Self::Touch => Some("touch"),
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Auto => Self::Touch,
            Self::Touch => Self::Compact,
            Self::Compact => Self::Auto,
        }
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Auto => "ui.density_auto",
            Self::Touch => "ui.density_touch",
            Self::Compact => "ui.density_compact",
        }
    }
}

/// Resolved per-frame UI density derived purely from [`UiDensityMode`] and viewport size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ResolvedDensity {
    /// Traditional 1-row high-density layout (desktop wide/medium, 80x24 classic SSH).
    Compact,
    /// Narrow short screens (e.g. 59x24 mobile with virtual keyboard): keep 1-row visual heights
    /// so content is not crushed, while enabling touch slop, relaxed second-tap, and mis-tap guards.
    TouchSlim,
    /// Medium-height touch screens (30..42 rows, e.g. enlarged-font portrait phone 52x36 or manual
    /// Touch on 120x36): 2-row solid touch buttons, 2-row lists, 2-row clickable bottom thumb bar.
    TouchMedium,
    /// Tall portrait touch screens (>=42 rows, e.g. 82x54 phone SSH): 3-row symmetric primary
    /// touch buttons, 2-row lists/tabs, 3-row clickable bottom thumb bar.
    TouchTall,
}

impl ResolvedDensity {
    pub fn resolve(mode: UiDensityMode, area: Rect) -> Self {
        match mode {
            UiDensityMode::Compact => Self::Compact,
            UiDensityMode::Touch => {
                if area.height >= 42 {
                    Self::TouchTall
                } else if area.height >= 26 {
                    Self::TouchMedium
                } else {
                    Self::TouchSlim
                }
            }
            UiDensityMode::Auto => {
                let portrait_or_narrow_tall = area.width < 96
                    && area.height >= 30
                    && (area.width < 80 || area.height.saturating_mul(2) > area.width);
                if portrait_or_narrow_tall {
                    if area.height >= 42 {
                        Self::TouchTall
                    } else {
                        Self::TouchMedium
                    }
                } else if area.width < 80 {
                    Self::TouchSlim
                } else {
                    Self::Compact
                }
            }
        }
    }

    pub const fn is_touch(self) -> bool {
        !matches!(self, Self::Compact)
    }

    pub const fn is_expanded(self) -> bool {
        matches!(self, Self::TouchMedium | Self::TouchTall)
    }

    pub const fn slop_enabled(self) -> bool {
        self.is_touch()
    }

    pub const fn button_height(self) -> u16 {
        match self {
            Self::Compact | Self::TouchSlim => 1,
            Self::TouchMedium => 2,
            Self::TouchTall => 3,
        }
    }

    pub const fn compact_btn_height(self) -> u16 {
        match self {
            Self::Compact | Self::TouchSlim => 1,
            Self::TouchMedium | Self::TouchTall => 2,
        }
    }

    pub const fn list_row_height(self) -> u16 {
        if self.is_expanded() { 2 } else { 1 }
    }

    pub const fn thumb_bar_height(self) -> u16 {
        match self {
            Self::Compact | Self::TouchSlim => 1,
            Self::TouchMedium => 2,
            Self::TouchTall => 3,
        }
    }
}

/// Chebyshev distance (in terminal cells) from `point` to the nearest cell inside `rect`.
pub(in crate::tui) fn rect_distance(rect: Rect, point: Position) -> u16 {
    if rect.width == 0 || rect.height == 0 {
        return u16::MAX;
    }
    let dx = if point.x < rect.left() {
        rect.left() - point.x
    } else if point.x >= rect.right() {
        point.x - (rect.right() - 1)
    } else {
        0
    };
    let dy = if point.y < rect.top() {
        rect.top() - point.y
    } else if point.y >= rect.bottom() {
        point.y - (rect.bottom() - 1)
    } else {
        0
    };
    dx.max(dy)
}

pub(in crate::tui) fn hit_rect(rect: Rect, point: Position, slop: bool) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    rect.contains(point) || (slop && rect_distance(rect, point) <= 1)
}

pub(in crate::tui) fn hit_opt(rect: Option<Rect>, point: Position, slop: bool) -> bool {
    rect.is_some_and(|r| hit_rect(r, point, slop))
}

/// Resolve the closest matching target in `items`:
/// 1. Exact containment always wins immediately.
/// 2. When `slop` is enabled, picks the unique closest target within 1 cell (ties resolve to `None`
///    so tapping a separator gap directly between two buttons never triggers the wrong neighbor).
pub(in crate::tui) fn hit_best<T: Copy>(
    items: impl IntoIterator<Item = (Rect, T)>,
    point: Position,
    slop: bool,
) -> Option<T> {
    let mut best: Option<(u16, u16, T)> = None;
    let mut tie = false;
    for (rect, val) in items {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        if rect.contains(point) {
            return Some(val);
        }
        if !slop {
            continue;
        }
        let dx = if point.x < rect.left() {
            rect.left() - point.x
        } else if point.x >= rect.right() {
            point.x - (rect.right() - 1)
        } else {
            0
        };
        let dy = if point.y < rect.top() {
            rect.top() - point.y
        } else if point.y >= rect.bottom() {
            point.y - (rect.bottom() - 1)
        } else {
            0
        };
        let cheb = dx.max(dy);
        if cheb > 1 {
            continue;
        }
        let manh = dx + dy;
        match best {
            None => {
                best = Some((cheb, manh, val));
                tie = false;
            }
            Some((bc, bm, _)) if (cheb, manh) < (bc, bm) => {
                best = Some((cheb, manh, val));
                tie = false;
            }
            Some((bc, bm, _)) if (cheb, manh) == (bc, bm) => {
                tie = true;
            }
            _ => {}
        }
    }
    if tie { None } else { best.map(|(_, _, v)| v) }
}

/// Split a horizontal bar into evenly sized touch buttons separated by 1-cell gaps.
pub(in crate::tui) fn split_touch_buttons(bar: Rect, count: usize) -> Vec<Rect> {
    if count == 0 || bar.width == 0 || bar.height == 0 {
        return Vec::new();
    }
    let gap = u16::from(bar.width >= (count as u16) * 5 + count.saturating_sub(1) as u16);
    let total_gap = gap.saturating_mul(count.saturating_sub(1) as u16);
    let usable = bar.width.saturating_sub(total_gap);
    let base_w = usable / count as u16;
    let rem = usable % count as u16;
    let mut out = Vec::with_capacity(count);
    let mut x = bar.x;
    for idx in 0..count {
        let w = base_w + u16::from((idx as u16) < rem);
        if w == 0 || x >= bar.right() {
            out.push(Rect::default());
        } else {
            let clamped_w = w.min(bar.right().saturating_sub(x));
            out.push(Rect::new(x, bar.y, clamped_w, bar.height));
            x = x.saturating_add(clamped_w + gap);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThumbAction {
    NavToggle,
    OverviewOpt,
    OverviewPublicIp,
    Theme,
    Help,
    Search,
    Execute,
    ToggleMark,
    RunOptimization,
    ClashNextTab,
    ClashCycleMode,
    ClashPrimary,
    ClashSecondary,
    DockerNextTab,
    DockerPrimary,
    DockerLogs,
    DockerRefresh,
    LogsToggle,
    CancelTask,
    ThemeSave,
    ThemeCancel,
    ThemeDensity,
    ThemeSwitchTab,
}

#[derive(Debug, Clone)]
pub(super) struct ThumbButton {
    pub action: ThumbAction,
    pub label: String,
    pub emphasized: bool,
    pub danger: bool,
    pub rect: Rect,
}

pub(super) fn fits(area: Rect) -> bool {
    area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT
}

pub(super) fn list_offset(selected: usize, count: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    selected
        .saturating_sub(height.saturating_sub(1))
        .min(count.saturating_sub(height))
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TopBarHits {
    pub nav_toggle: Option<Rect>,
    pub running_pill: Option<Rect>,
    pub lang_btn: Option<Rect>,
}

#[derive(Debug, Clone)]
pub(super) enum SidebarRow {
    Spacer,
    Item(NavTarget),
    Rule,
    Section(&'static str),
}

#[derive(Debug, Clone)]
pub(super) struct SidebarGeometry {
    pub area: Rect,
    pub overlay: bool,
    pub rows: Vec<SidebarRow>,
    pub row_rects: Vec<(Rect, usize)>,
    pub item_rects: Vec<(Rect, NavTarget)>,
    pub collapse_btn: Option<Rect>,
    pub theme_btn: Option<Rect>,
    pub prev_btn: Option<Rect>,
    pub next_btn: Option<Rect>,
    pub can_prev: bool,
    pub can_next: bool,
    pub page_text: Option<String>,
    pub scroll: usize,
    pub visible_items: usize,
}

impl SidebarGeometry {
    pub fn target_at(&self, point: Position, slop: bool) -> Option<NavTarget> {
        hit_best(self.item_rects.iter().copied(), point, slop)
    }
}

#[derive(Debug, Clone)]
pub(super) struct OverviewGeometry {
    pub system_card: Rect,
    pub network_card: Rect,
    pub quick_header: Rect,
    /// When `Some`, quick actions render as a unified full-width list card (mobile / short screens).
    pub quick_list: Option<Rect>,
    pub quick_cards: [Rect; 4],
}

impl OverviewGeometry {
    pub fn card_at(&self, point: Position, slop: bool) -> Option<usize> {
        hit_best(
            self.quick_cards
                .iter()
                .copied()
                .enumerate()
                .map(|(i, r)| (r, i)),
            point,
            slop,
        )
    }

    pub fn is_list_mode(&self) -> bool {
        self.quick_list.is_some()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SectionPill {
    pub tab: Option<usize>,
    pub label: String,
    pub count: usize,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub(super) struct WorkbenchGeometry {
    pub list_area: Rect,
    pub section_bar: Rect,
    pub section_pills: Vec<SectionPill>,
    pub section_overflow_left: bool,
    pub section_overflow_right: bool,
    pub list_rule_y: Option<u16>,
    pub list_inner: Rect,
    pub list_scroll: usize,
    pub visual_rows: Vec<(Rect, usize)>,
    pub selectable_rects: Vec<(Rect, usize)>,
    pub section_row_rects: Vec<(Rect, &'static str)>,
    pub prev_btn: Option<Rect>,
    pub next_btn: Option<Rect>,
    pub can_prev: bool,
    pub can_next: bool,
    pub page_text: Option<String>,
    pub inspector_area: Rect,
    pub inspector_inner: Rect,
    pub inspector_cta: Option<Rect>,
    pub inspector_copy_btn: Option<Rect>,
    pub stacked: bool,
}

impl WorkbenchGeometry {
    /// Returns the section tab filter (`None` = All, `Some(idx)` = section) if `point` hits a pill.
    pub fn section_pill_at(&self, point: Position, slop: bool) -> Option<Option<usize>> {
        hit_best(
            self.section_pills.iter().map(|p| (p.rect, p.tab)),
            point,
            slop,
        )
    }

    /// Returns the section ID if `point` clicks a section header row in the list.
    pub fn section_row_at(&self, point: Position, _rows: &[Row]) -> Option<&'static str> {
        self.section_row_rects
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, id)| *id)
    }

    /// Returns the selectable row index under `point`, if any.
    pub fn selectable_at(&self, point: Position, _rows: &[Row], slop: bool) -> Option<usize> {
        if !self.list_inner.contains(point) {
            return None;
        }
        hit_best(self.selectable_rects.iter().copied(), point, slop)
    }
}

#[derive(Debug, Clone)]
pub(super) struct ThemeDrawerGeometry {
    pub area: Rect,
    pub tab_palette: Rect,
    pub tab_style: Rect,
    pub list_area: Rect,
    pub list_scroll: usize,
    pub row_height: u16,
    pub prev_btn: Option<Rect>,
    pub next_btn: Option<Rect>,
    pub can_prev: bool,
    pub can_next: bool,
    pub transparent_toggle: Rect,
    pub motion_toggle: Rect,
    pub density_toggle: Rect,
    pub details_area: Rect,
}

impl ThemeDrawerGeometry {
    pub fn entry_at(&self, point: Position, count: usize) -> Option<usize> {
        if !self.list_area.contains(point) || self.row_height == 0 {
            return None;
        }
        let idx = ((point.y - self.list_area.y) / self.row_height) as usize + self.list_scroll;
        (idx < count).then_some(idx)
    }
}

#[derive(Debug, Clone)]
pub(super) struct LayoutRegions {
    pub density: ResolvedDensity,
    pub topbar: Rect,
    pub topbar_hits: TopBarHits,
    pub sidebar: Option<SidebarGeometry>,
    pub workspace: Rect,
    pub overview: Option<OverviewGeometry>,
    pub workbench: Option<WorkbenchGeometry>,
    pub docker: Option<super::docker::layout::DockerGeometry>,
    pub clash: Option<super::clash::layout::ClashGeometry>,
    pub theme_drawer: Option<ThemeDrawerGeometry>,
    pub logs: Option<Rect>,
    pub log_body: Rect,
    pub log_copy_btn: Option<Rect>,
    pub log_plain_btn: Option<Rect>,
    pub log_expand_btn: Option<Rect>,
    pub footer: Rect,
    pub thumb_bar: Vec<ThumbButton>,
}

impl LayoutRegions {
    pub fn compute(area: Rect, app: &App) -> Self {
        let breakpoint = Breakpoint::of(area.width);
        let density = app.resolved_density(area);
        let topbar = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        let footer_h = if density.is_expanded() && area.height >= 20 {
            density
                .thumb_bar_height()
                .min(area.height.saturating_sub(2))
        } else {
            1.min(area.height)
        };
        let footer_y = area.bottom().saturating_sub(footer_h);
        let footer = Rect::new(area.x, footer_y, area.width, footer_h);

        let mid_height = area.height.saturating_sub(topbar.height + footer_h);
        let mid = Rect::new(area.x, area.y + topbar.height, area.width, mid_height);

        // Optional bottom log drawer (zero height when idle & closed)
        let show_logs = app.logs_open && (!app.logs.is_empty() || app.worker.is_some());
        let (body, logs, log_body) = if show_logs && mid.height >= 16 {
            let log_h = if mid.height >= 30 {
                9
            } else if mid.height >= 24 {
                8
            } else {
                6
            };
            let body_h = mid.height.saturating_sub(log_h + 1);
            let body = Rect::new(mid.x, mid.y, mid.width, body_h);
            let logs = Rect::new(mid.x, body.bottom() + 1, mid.width, log_h);
            let log_body = Rect::new(
                logs.x + 2,
                logs.y + 1,
                logs.width.saturating_sub(4),
                logs.height.saturating_sub(2),
            );
            (body, Some(logs), log_body)
        } else {
            (mid, None, Rect::default())
        };

        // Sidebar docking vs overlay (Docker / Mihomo Focus mode hides docked sidebar for full width)
        let workspace_focus = (app.is_docker_view() && app.docker.fullscreen)
            || (app.is_clash_view() && app.clash.fullscreen);
        let docked_sidebar = match breakpoint {
            Breakpoint::Compact | Breakpoint::Mobile => false,
            Breakpoint::Medium | Breakpoint::Wide => !app.sidebar_collapsed && !workspace_focus,
        };
        let sidebar_width = if !docked_sidebar && density.is_expanded() {
            ((u32::from(body.width) * 55) / 100)
                .clamp(28, 36)
                .min(u32::from(body.width)) as u16
        } else {
            match breakpoint {
                Breakpoint::Wide => 22,
                Breakpoint::Medium => 19,
                Breakpoint::Compact | Breakpoint::Mobile => 24,
            }
        };

        let (sidebar, workspace) = if docked_sidebar && body.width > sidebar_width + 20 {
            let side_rect = Rect::new(body.x, body.y, sidebar_width, body.height);
            let ws_rect = Rect::new(
                body.x + sidebar_width + 1,
                body.y,
                body.width.saturating_sub(sidebar_width + 1),
                body.height,
            );
            (Some(build_sidebar(side_rect, false, app, density)), ws_rect)
        } else if app.sidebar_overlay {
            let overlay_rect =
                Rect::new(body.x, body.y, sidebar_width.min(body.width), body.height);
            (Some(build_sidebar(overlay_rect, true, app, density)), body)
        } else {
            (None, body)
        };

        // Theme drawer docking on right of workspace when open
        let (content_area, theme_drawer) = if let Some(drawer) = &app.theme_drawer {
            let drawer_w = if workspace.width >= 96 {
                38
            } else if workspace.width >= 78 {
                34
            } else {
                workspace.width
            };
            if workspace.width >= 78 && !(density.is_expanded() && workspace.width < 92) {
                let left_w = workspace.width.saturating_sub(drawer_w + 1);
                let left = Rect::new(workspace.x, workspace.y, left_w, workspace.height);
                let right = Rect::new(
                    workspace.x + left_w + 1,
                    workspace.y,
                    drawer_w,
                    workspace.height,
                );
                (
                    left,
                    Some(build_theme_drawer(
                        right,
                        drawer.active_count(),
                        drawer.active_cursor(),
                        density,
                    )),
                )
            } else {
                (
                    workspace,
                    Some(build_theme_drawer(
                        workspace,
                        drawer.active_count(),
                        drawer.active_cursor(),
                        density,
                    )),
                )
            }
        } else {
            (workspace, None)
        };

        let (overview, workbench, docker, clash) = match app.nav {
            NavTarget::Overview => (
                Some(build_overview(content_area, app, density)),
                None,
                None,
                None,
            ),
            NavTarget::Optimization | NavTarget::Group(_) if app.is_docker_view() => (
                None,
                None,
                Some(super::docker::layout::DockerGeometry::compute(
                    content_area,
                    app,
                    theme_drawer.is_some(),
                    density,
                )),
                None,
            ),
            NavTarget::Optimization | NavTarget::Group(_) if app.is_clash_view() => (
                None,
                None,
                None,
                Some(super::clash::layout::ClashGeometry::compute(
                    content_area,
                    app,
                    theme_drawer.is_some(),
                    density,
                )),
            ),
            NavTarget::Optimization | NavTarget::Group(_) => (
                None,
                Some(build_workbench(
                    content_area,
                    app,
                    theme_drawer.is_some(),
                    density,
                )),
                None,
                None,
            ),
        };

        // Topbar interactive hit targets (aligned with topbar renderer)
        let topbar_hits = compute_topbar_hits(topbar, app, docked_sidebar, density);
        let (log_copy_btn, log_plain_btn, log_expand_btn) = logs
            .map(|rect| compute_log_drawer_hits(rect, app))
            .unwrap_or((None, None, None));
        let thumb_bar = if footer_h >= 2 {
            build_thumb_bar(footer, app, density)
        } else {
            Vec::new()
        };

        Self {
            density,
            topbar,
            topbar_hits,
            sidebar,
            workspace,
            overview,
            workbench,
            docker,
            clash,
            theme_drawer,
            logs,
            log_body,
            log_copy_btn,
            log_plain_btn,
            log_expand_btn,
            footer,
            thumb_bar,
        }
    }
}

fn build_thumb_bar(footer: Rect, app: &App, _density: ResolvedDensity) -> Vec<ThumbButton> {
    if footer.width < 20 || footer.height < 2 {
        return Vec::new();
    }
    let i = &app.ctx.i18n;
    let menu = super::render::tokens::glyph(&app.theme.active, super::render::tokens::Glyph::Menu);
    let nav_label = format!("{menu} {}", i.text("nav.toggle"));

    let specs: Vec<(ThumbAction, String, bool, bool)> = if app.worker.is_some() {
        vec![
            (
                ThumbAction::LogsToggle,
                i.text("touch.logs").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::CancelTask,
                i.text("touch.cancel").to_owned(),
                false,
                true,
            ),
        ]
    } else if app.theme_drawer.is_some() {
        vec![
            (
                ThumbAction::ThemeSwitchTab,
                i.text("touch.next_tab").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::ThemeDensity,
                i.text(app.ui_density.label_key()).to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::ThemeCancel,
                i.text("touch.cancel").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::ThemeSave,
                i.text("touch.save").to_owned(),
                true,
                false,
            ),
        ]
    } else if app.nav == NavTarget::Overview {
        vec![
            (ThumbAction::NavToggle, nav_label, false, false),
            (
                ThumbAction::OverviewOpt,
                i.text("group.optimization").to_owned(),
                true,
                false,
            ),
            (
                ThumbAction::OverviewPublicIp,
                i.text("key.public").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::Theme,
                i.text("nav.theme").to_owned(),
                false,
                false,
            ),
        ]
    } else if app.is_clash_view() {
        use crate::ops::clash::model::ClashTab;
        let (p_lbl, s_lbl) = match app.clash.tab {
            ClashTab::Overview => (
                i.text("touch.mode").to_owned(),
                i.text("touch.check").to_owned(),
            ),
            ClashTab::Proxies => (
                i.text("touch.test").to_owned(),
                i.text("clash.key.fastest").to_owned(),
            ),
            ClashTab::Profiles => (
                i.text("clash.key.update").to_owned(),
                i.text("clash.key.add").to_owned(),
            ),
            ClashTab::Connections => (
                i.text("touch.refresh").to_owned(),
                i.text("clash.key.close_all").to_owned(),
            ),
            ClashTab::Rules => (
                i.text("touch.refresh").to_owned(),
                i.text("touch.search").to_owned(),
            ),
            ClashTab::Tools => (
                i.text("touch.logs").to_owned(),
                i.text("touch.refresh").to_owned(),
            ),
        };
        let p_act = if app.clash.tab == ClashTab::Overview {
            ThumbAction::ClashCycleMode
        } else {
            ThumbAction::ClashPrimary
        };
        vec![
            (ThumbAction::NavToggle, nav_label, false, false),
            (
                ThumbAction::ClashNextTab,
                i.text("touch.next_tab").to_owned(),
                false,
                false,
            ),
            (p_act, p_lbl, true, false),
            (
                ThumbAction::ClashSecondary,
                s_lbl,
                false,
                app.clash.tab == ClashTab::Connections,
            ),
        ]
    } else if app.is_docker_view() {
        vec![
            (ThumbAction::NavToggle, nav_label, false, false),
            (
                ThumbAction::DockerNextTab,
                i.text("touch.next_tab").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::DockerPrimary,
                i.text("touch.start_stop").to_owned(),
                true,
                false,
            ),
            (
                ThumbAction::DockerLogs,
                i.text("touch.logs").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::DockerRefresh,
                i.text("touch.refresh").to_owned(),
                false,
                false,
            ),
        ]
    } else if app.optimization() {
        vec![
            (ThumbAction::NavToggle, nav_label, false, false),
            (
                ThumbAction::ToggleMark,
                i.text("key.select").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::RunOptimization,
                i.text("touch.run").to_owned(),
                true,
                false,
            ),
            (
                ThumbAction::Search,
                i.text("touch.search").to_owned(),
                false,
                false,
            ),
        ]
    } else {
        let exec_lbl = match app.selected_row() {
            Some(Row::Action(spec)) if !spec.fields.is_empty() => i.text("touch.open").to_owned(),
            _ => i.text("touch.run").to_owned(),
        };
        vec![
            (ThumbAction::NavToggle, nav_label, false, false),
            (ThumbAction::Execute, exec_lbl, true, false),
            (
                ThumbAction::Search,
                i.text("touch.search").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::Theme,
                i.text("nav.theme").to_owned(),
                false,
                false,
            ),
            (
                ThumbAction::Help,
                i.text("key.help").to_owned(),
                false,
                false,
            ),
        ]
    };

    // Trim to 4 buttons on narrow screens so every thumb button stays wide and legible
    let max_btns = if footer.width < 56 { 4 } else { specs.len() };
    let specs: Vec<_> = specs.into_iter().take(max_btns).collect();
    let pad_x = u16::from(footer.width >= 44);
    let inner = Rect::new(
        footer.x + pad_x,
        footer.y,
        footer.width.saturating_sub(pad_x * 2),
        footer.height,
    );
    let rects = split_touch_buttons(inner, specs.len());
    specs
        .into_iter()
        .zip(rects)
        .filter(|(_, r)| r.width >= 3 && r.height > 0)
        .map(|((action, label, emphasized, danger), rect)| ThumbButton {
            action,
            label,
            emphasized,
            danger,
            rect,
        })
        .collect()
}

pub(super) fn topbar_lang_label(app: &App, bar_width: u16) -> String {
    if bar_width < 52 {
        let locale = app.ctx.i18n.locale();
        if locale.starts_with("zh") {
            "中".to_owned()
        } else {
            locale
                .split(['-', '_'])
                .next()
                .unwrap_or("EN")
                .to_ascii_uppercase()
        }
    } else {
        app.ctx.i18n.language_name().to_owned()
    }
}

pub(super) fn topbar_running_pill_text(app: &App, bar_width: u16) -> String {
    let label = app.active_step_summary().unwrap_or_else(|| {
        let dur = app
            .single_elapsed()
            .map(super::state::format_mm_ss)
            .unwrap_or_default();
        format!("{} {dur}", app.task_title())
    });
    let max_pill = if bar_width >= 80 {
        34
    } else {
        (bar_width as usize / 3).clamp(12, 24)
    };
    super::render::clip_ascii_safe(
        &format!(" {} {} [G] ", app.spin(), label),
        max_pill,
        app.theme.active.flags.ascii,
    )
}

fn compute_topbar_hits(
    bar: Rect,
    app: &App,
    docked_sidebar: bool,
    density: ResolvedDensity,
) -> TopBarHits {
    let mut hits = TopBarHits::default();
    if bar.height == 0 || bar.width < 20 {
        return hits;
    }
    let extra_pad = u16::from(density.is_expanded() && bar.width >= 56) * 2;
    if !docked_sidebar {
        let nav_label = format!(" {} ", app.ctx.i18n.text("nav.toggle"));
        let w = (nav_label.width() as u16 + 2 + extra_pad).min(bar.width);
        hits.nav_toggle = Some(Rect::new(bar.x + 1, bar.y, w, 1));
    }
    let lang_text = topbar_lang_label(app, bar.width);
    let lang_w = lang_text.width() as u16 + 2 + extra_pad;
    if bar.width > lang_w + 4 {
        hits.lang_btn = Some(Rect::new(
            bar.right().saturating_sub(lang_w + 1),
            bar.y,
            lang_w,
            1,
        ));
    }
    if app.worker.is_some() && bar.width >= 40 {
        let pill_w = topbar_running_pill_text(app, bar.width).width() as u16;
        let mobile = bar.width < 80;
        let sep_w = format!(
            " {} ",
            super::render::tokens::glyph(
                &app.theme.active,
                super::render::tokens::Glyph::Separator
            )
        )
        .width() as u16;
        let mut after_pill = if mobile { 1u16 } else { sep_w };
        if bar.width >= 90
            && let crate::dashboard::Reading::Value(host) = &app.snapshot.local.host
            && !host.is_empty()
        {
            let clipped = super::render::clip_ascii_safe(host, 18, app.theme.active.flags.ascii);
            after_pill = after_pill.saturating_add(clipped.width() as u16 + sep_w);
        }
        if !mobile {
            let role_key = if app.platform_error.is_some() {
                "ui.mode_preview"
            } else if app.root {
                "ui.mode_root"
            } else {
                "ui.mode_user"
            };
            let role_w = format!(
                "{} {}",
                super::render::tokens::glyph(
                    &app.theme.active,
                    super::render::tokens::Glyph::DotOn
                ),
                app.ctx.i18n.text(role_key)
            )
            .width() as u16;
            after_pill = after_pill.saturating_add(role_w + 2);
        }
        after_pill = after_pill.saturating_add(lang_w + 1);
        let right_total = pill_w.saturating_add(after_pill);
        if bar.width > right_total {
            let x = bar.right().saturating_sub(right_total);
            if x >= hits.nav_toggle.map_or(bar.x, |r| r.right()) {
                hits.running_pill = Some(Rect::new(x, bar.y, pill_w, 1));
            }
        }
    }
    hits
}

fn build_sidebar(
    area: Rect,
    overlay: bool,
    app: &App,
    density: ResolvedDensity,
) -> SidebarGeometry {
    let expanded = density.is_expanded() && (overlay || area.height >= 30);
    let item_height = if expanded { 2u16 } else { 1u16 };
    let btn_h = if expanded {
        if area.height >= 34 {
            density.button_height()
        } else {
            density.compact_btn_height()
        }
    } else {
        1u16
    };
    let top_inset = if overlay {
        1
    } else {
        super::render::tokens::card_top_inset(&app.theme.active, area.height)
    };
    let inner_y = area.y + top_inset;
    let framed_bottom = u16::from(overlay || app.theme.active.flags.framed);
    let footer_h = if area.height >= 8 + btn_h {
        1 + btn_h + framed_bottom
    } else if area.height >= 8 {
        2 + framed_bottom
    } else {
        0
    };
    let raw_list_h = area.height.saturating_sub(top_inset + footer_h) as usize;

    let is_default_order = app.category_order == default_category_order();
    let use_sections = is_default_order && !expanded && raw_list_h >= 18;
    let use_spacers = use_sections && raw_list_h >= 21;

    let mut rows = Vec::with_capacity(24);
    rows.push(SidebarRow::Item(NavTarget::Overview));
    rows.push(SidebarRow::Item(NavTarget::Optimization));
    rows.push(SidebarRow::Rule);
    for (idx, &group) in app.category_order.iter().enumerate() {
        if use_sections {
            match idx {
                0 => rows.push(SidebarRow::Section("nav.section.infra")),
                5 => {
                    if use_spacers {
                        rows.push(SidebarRow::Spacer);
                    }
                    rows.push(SidebarRow::Section("nav.section.net"));
                }
                9 => {
                    if use_spacers {
                        rows.push(SidebarRow::Spacer);
                    }
                    rows.push(SidebarRow::Section("nav.section.more"));
                }
                _ => {}
            }
        }
        rows.push(SidebarRow::Item(NavTarget::Group(group)));
    }

    let row_h = |r: &SidebarRow| -> usize {
        match r {
            SidebarRow::Item(_) => item_height as usize,
            _ => 1,
        }
    };
    let total_visual_h: usize = rows.iter().map(row_h).sum();
    let need_pager = expanded && total_visual_h > raw_list_h && raw_list_h >= 8;
    let pager_h = if need_pager { 2usize } else { 0 };
    let list_h = raw_list_h.saturating_sub(pager_h);

    let selected_row = rows
        .iter()
        .position(|r| matches!(r, SidebarRow::Item(t) if *t == app.nav))
        .unwrap_or(0);

    // Compute starting row index so either manual touch scroll or active nav stays visible.
    let mut scroll = if let Some(manual) = app.sidebar_scroll.filter(|_| expanded) {
        manual.min(rows.len().saturating_sub(1))
    } else if !expanded {
        list_offset(selected_row, rows.len(), list_h)
    } else {
        let mut s = 0usize;
        while s < selected_row {
            let span: usize = rows[s..=selected_row].iter().map(row_h).sum();
            if span <= list_h {
                break;
            }
            s += 1;
        }
        s
    };

    // Pull scroll back when trailing rows leave unused space at bottom.
    while scroll > 0 {
        let span: usize = rows[scroll - 1..].iter().map(row_h).sum();
        if span <= list_h {
            scroll -= 1;
        } else {
            break;
        }
    }

    let mut row_rects = Vec::new();
    let mut item_rects = Vec::new();
    let mut y = inner_y;
    let max_y = inner_y + list_h as u16;
    let mut last_idx = scroll;
    for (idx, row) in rows.iter().enumerate().skip(scroll) {
        let h = row_h(row) as u16;
        if y + h > max_y {
            break;
        }
        let rect = Rect::new(area.x + 1, y, area.width.saturating_sub(2), h);
        row_rects.push((rect, idx));
        if let SidebarRow::Item(target) = *row {
            item_rects.push((rect, target));
        }
        last_idx = idx;
        y += h;
    }

    let visible_items = item_rects.len().max(1);
    let can_prev = scroll > 0;
    let can_next = !row_rects.is_empty() && last_idx + 1 < rows.len();
    let (prev_btn, next_btn, page_text) = if need_pager {
        let py = inner_y + list_h as u16;
        let inner_w = area.width.saturating_sub(4);
        let btn_w = inner_w.saturating_sub(1) / 2;
        let p = Rect::new(area.x + 2, py, btn_w, 2);
        let n = Rect::new(
            area.x + 2 + btn_w + 1,
            py,
            inner_w.saturating_sub(btn_w + 1),
            2,
        );
        let total_items = rows
            .iter()
            .filter(|r| matches!(r, SidebarRow::Item(_)))
            .count();
        let shown_before = rows[..scroll]
            .iter()
            .filter(|r| matches!(r, SidebarRow::Item(_)))
            .count();
        let page = (shown_before / visible_items) + 1;
        let pages = total_items.div_ceil(visible_items).max(1);
        (Some(p), Some(n), Some(format!("{page}/{pages}")))
    } else {
        (None, None, None)
    };

    let (collapse_btn, theme_btn) = if footer_h > 0 {
        let actual_btn_h = footer_h.saturating_sub(1 + framed_bottom).max(1);
        let btn_y = area.bottom().saturating_sub(actual_btn_h + framed_bottom);
        if expanded && area.width >= 14 {
            let btns = split_touch_buttons(
                Rect::new(
                    area.x + 2,
                    btn_y,
                    area.width.saturating_sub(4),
                    actual_btn_h,
                ),
                2,
            );
            (Some(btns[0]), Some(btns[1]))
        } else {
            let half = area.width.saturating_sub(2) / 2;
            (
                Some(Rect::new(area.x + 1, btn_y, half, 1)),
                Some(Rect::new(
                    area.x + 1 + half,
                    btn_y,
                    area.width.saturating_sub(2 + half),
                    1,
                )),
            )
        }
    } else {
        (None, None)
    };

    SidebarGeometry {
        area,
        overlay,
        rows,
        row_rects,
        item_rects,
        collapse_btn,
        theme_btn,
        prev_btn,
        next_btn,
        can_prev,
        can_next,
        page_text,
        scroll,
        visible_items,
    }
}

fn build_overview(area: Rect, app: &App, density: ResolvedDensity) -> OverviewGeometry {
    let stacked_metrics = area.width < 76;
    let use_unified_quick = stacked_metrics || area.height < 20;

    if use_unified_quick {
        let min_quick_h = if density.is_expanded() {
            14u16.min(area.height)
        } else {
            6u16.min(area.height)
        };
        let (system_card, network_card, metrics_h) = if stacked_metrics {
            let avail = area.height.saturating_sub(min_quick_h);
            let top_h = avail.div_ceil(2).min(9);
            let bot_h = avail.saturating_sub(top_h).min(8);
            (
                Rect::new(area.x, area.y, area.width, top_h),
                Rect::new(area.x, area.y + top_h, area.width, bot_h),
                top_h + bot_h,
            )
        } else {
            // Short wide screens (e.g. landscape phone with virtual keyboard): keep metrics side-by-side.
            let mh = area.height.saturating_sub(min_quick_h).min(9);
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Length(1),
                    Constraint::Min(20),
                ])
                .split(Rect::new(area.x, area.y, area.width, mh));
            (cols[0], cols[2], mh)
        };

        let remain = area.height.saturating_sub(metrics_h);
        let gap = u16::from((7..10).contains(&remain) || (density.is_expanded() && remain >= 12));
        let quick_y = area.y + metrics_h + gap;
        let quick_avail = area.bottom().saturating_sub(quick_y);
        let row_h = if density.is_expanded() && quick_avail >= 14 {
            3u16
        } else if quick_avail >= 10 {
            2u16
        } else {
            1u16
        };
        let quick_box_h = (2 + 4 * row_h).min(quick_avail);
        let quick_box = Rect::new(area.x, quick_y, area.width, quick_box_h);

        let top_inset = if quick_box_h >= 6 {
            super::render::tokens::card_top_inset(&app.theme.active, quick_box_h)
        } else {
            0
        };
        let bot_inset = if quick_box_h >= 6 {
            u16::from(app.theme.active.flags.framed)
        } else {
            0
        };
        let inner_y = quick_box.y + top_inset;
        let inner_bottom = quick_box.bottom().saturating_sub(bot_inset);
        let mut quick_cards = [Rect::default(); 4];
        for (i, slot) in quick_cards.iter_mut().enumerate() {
            let y = inner_y + (i as u16) * row_h;
            if y < inner_bottom {
                *slot = Rect::new(
                    quick_box.x + 1,
                    y,
                    quick_box.width.saturating_sub(2),
                    row_h.min(inner_bottom - y),
                );
            }
        }

        return OverviewGeometry {
            system_card,
            network_card,
            quick_header: Rect::default(),
            quick_list: Some(quick_box),
            quick_cards,
        };
    }

    let metrics_h = if area.height >= 26 {
        10
    } else if area.height >= 22 {
        9
    } else {
        8.min(area.height.saturating_sub(6).max(6))
    };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(1),
            Constraint::Min(20),
        ])
        .split(Rect::new(area.x, area.y, area.width, metrics_h));
    let (system_card, network_card) = (cols[0], cols[2]);

    let gap = u16::from(area.height > metrics_h + 7);
    let quick_y = area.y + metrics_h + gap;
    let quick_header = Rect::new(
        area.x,
        quick_y,
        area.width,
        u16::from(quick_y < area.bottom()),
    );
    let cards_y = quick_y + quick_header.height;
    let cards_h = area.bottom().saturating_sub(cards_y);

    let two_cols = area.width >= 68;
    let quick_cards = if two_cols && cards_h >= 7 {
        let max_row_h = if density.is_expanded() { 5 } else { 4 };
        let row_h = (cards_h.saturating_sub(1) / 2).clamp(3, max_row_h);
        let r0 = Rect::new(area.x, cards_y, area.width, row_h);
        let r1 = Rect::new(
            area.x,
            (cards_y + row_h + 1).min(area.bottom()),
            area.width,
            row_h.min(area.bottom().saturating_sub(cards_y + row_h + 1)),
        );
        let split_row = |row: Rect| {
            let parts = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Length(1),
                    Constraint::Min(10),
                ])
                .split(row);
            (parts[0], parts[2])
        };
        let (c0, c1) = split_row(r0);
        let (c2, c3) = split_row(r1);
        [c0, c1, c2, c3]
    } else {
        let each_h = (cards_h / 4).max(2);
        let mut cards = [Rect::default(); 4];
        for (i, slot) in cards.iter_mut().enumerate() {
            let y = cards_y + (i as u16) * each_h;
            if y < area.bottom() {
                *slot = Rect::new(area.x, y, area.width, each_h.min(area.bottom() - y));
            }
        }
        cards
    };

    OverviewGeometry {
        system_card,
        network_card,
        quick_header,
        quick_list: None,
        quick_cards,
    }
}

fn build_workbench(
    area: Rect,
    app: &App,
    drawer_open: bool,
    density: ResolvedDensity,
) -> WorkbenchGeometry {
    let expanded = density.is_expanded();
    let row_height = density.list_row_height();
    let side_by_side = !drawer_open && area.width >= 82 && !(expanded && area.width < 92);
    let (list_area, inspector_area, stacked) = if side_by_side {
        let list_w = ((u32::from(area.width) * 56) / 100) as u16;
        let list_w = list_w.clamp(42, area.width.saturating_sub(34));
        let list = Rect::new(area.x, area.y, list_w, area.height);
        let insp = Rect::new(
            area.x + list_w + 1,
            area.y,
            area.width.saturating_sub(list_w + 1),
            area.height,
        );
        (list, insp, false)
    } else if drawer_open {
        // When Theme Drawer owns the right column, give the left column 100% to the list.
        (area, Rect::default(), false)
    } else {
        let gap = u16::from(area.width >= 76 && area.height >= 20);
        let insp_h = if expanded && area.height >= 24 {
            if area.height >= 36 { 9 } else { 8 }
        } else if area.height >= 20 {
            6
        } else if area.height >= 16 {
            5
        } else {
            4
        };
        let list_h = area.height.saturating_sub(insp_h + gap);
        let list = Rect::new(area.x, area.y, area.width, list_h);
        let insp = Rect::new(area.x, list.bottom() + gap, area.width, insp_h);
        (list, insp, true)
    };

    let (section_bar, section_pills, section_overflow_left, section_overflow_right, list_rule_y) =
        build_section_bar(list_area, app, density);

    let pad_top = match list_rule_y {
        Some(rule_y) => (rule_y + 1).saturating_sub(list_area.y),
        None => 2u16,
    };
    let pad_bottom = 1u16;
    let raw_list_inner = Rect::new(
        list_area.x + 1,
        list_area.y + pad_top.min(list_area.height),
        list_area.width.saturating_sub(2),
        list_area.height.saturating_sub(pad_top + pad_bottom),
    );
    let insp_top = super::render::tokens::card_top_inset(&app.theme.active, inspector_area.height);
    let insp_bot =
        super::render::tokens::card_bottom_inset(&app.theme.active, inspector_area.height);
    let insp_pad = insp_top + insp_bot;
    let inspector_inner = if inspector_area.width >= 4 && inspector_area.height > insp_pad {
        Rect::new(
            inspector_area.x + 2,
            inspector_area.y + insp_top,
            inspector_area.width.saturating_sub(4),
            inspector_area.height.saturating_sub(insp_pad),
        )
    } else {
        Rect::default()
    };

    let mut visual_rows = Vec::new();
    let mut selectable_rects = Vec::new();
    let mut section_row_rects = Vec::new();
    let mut list_scroll = 0usize;
    let mut list_inner = raw_list_inner;
    let mut prev_btn = None;
    let mut next_btn = None;
    let mut can_prev = false;
    let mut can_next = false;
    let mut page_text = None;

    if let Some(group) = app.active_group() {
        let rows = app.rows(group);
        let r_height = |r: &Row| -> usize {
            if expanded && r.selectable() {
                row_height as usize
            } else {
                1
            }
        };
        let total_visual_h: usize = rows.iter().map(r_height).sum();
        let need_pager = expanded
            && total_visual_h > raw_list_inner.height as usize
            && raw_list_inner.height >= 7;
        let pager_h = if need_pager { 2u16 } else { 0 };
        list_inner = Rect::new(
            raw_list_inner.x,
            raw_list_inner.y,
            raw_list_inner.width,
            raw_list_inner.height.saturating_sub(pager_h),
        );

        let sel_visual = if rows.iter().any(Row::selectable) {
            row_index(&rows, app.selected_in(group))
        } else {
            0
        };
        let avail_h = list_inner.height as usize;
        if !expanded {
            let mut scroll = list_offset(sel_visual, rows.len(), avail_h);
            if scroll > 0 && matches!(rows.get(scroll - 1), Some(Row::Section(..))) {
                scroll -= 1;
            }
            list_scroll = scroll;
        } else {
            let mut scroll = 0usize;
            while scroll < sel_visual {
                let span: usize = rows[scroll..=sel_visual].iter().map(r_height).sum();
                if span <= avail_h {
                    break;
                }
                scroll += 1;
            }
            if scroll > 0 && matches!(rows.get(scroll - 1), Some(Row::Section(..))) {
                let span_with_sec: usize = rows[scroll - 1..=sel_visual].iter().map(r_height).sum();
                if span_with_sec <= avail_h {
                    scroll -= 1;
                }
            }
            list_scroll = scroll;
        }

        let mut y = list_inner.y;
        let mut sel_idx = rows[..list_scroll]
            .iter()
            .filter(|r| r.selectable())
            .count();
        let mut last_idx = list_scroll;
        for (idx, row) in rows.iter().enumerate().skip(list_scroll) {
            let h = r_height(row) as u16;
            if y + h > list_inner.bottom() {
                break;
            }
            let rect = Rect::new(list_inner.x, y, list_inner.width, h);
            visual_rows.push((rect, idx));
            match row {
                Row::Section(sec_id, _) => section_row_rects.push((rect, *sec_id)),
                r if r.selectable() => {
                    selectable_rects.push((rect, sel_idx));
                    sel_idx += 1;
                }
                _ => {}
            }
            last_idx = idx;
            y += h;
        }

        if need_pager {
            can_prev = list_scroll > 0;
            can_next = !visual_rows.is_empty() && last_idx + 1 < rows.len();
            let py = list_inner.bottom();
            let p_inner = Rect::new(
                list_inner.x + 1,
                py,
                list_inner.width.saturating_sub(2),
                pager_h,
            );
            let btn_w = (p_inner.width.saturating_sub(10) / 2).clamp(10, 18);
            if p_inner.width >= btn_w * 2 + 2 {
                prev_btn = Some(Rect::new(p_inner.x, py, btn_w, pager_h));
                next_btn = Some(Rect::new(
                    p_inner.right().saturating_sub(btn_w),
                    py,
                    btn_w,
                    pager_h,
                ));
            } else {
                let btns = split_touch_buttons(p_inner, 2);
                prev_btn = Some(btns[0]);
                next_btn = Some(btns[1]);
            }
            let total_sel = rows.iter().filter(|r| r.selectable()).count().max(1);
            let cur_pos = app.selected_in(group) + 1;
            page_text = Some(format!("{cur_pos}/{total_sel}"));
        }
    }

    let show_opt_progress =
        app.active_group() == Some(super::state::OPTIMIZATION_GROUP) && app.progress.is_some();
    let empty_execute = matches!(app.selected_row(), Some(Row::Execute)) && app.marked.is_empty();
    let allow_cta = (!stacked && inspector_inner.height >= 6)
        || (stacked && expanded && inspector_inner.height >= 5);
    let cta_h = if expanded && inspector_inner.height >= 5 {
        density.compact_btn_height()
    } else {
        1u16
    };
    let (inspector_cta, inspector_copy_btn) =
        if allow_cta && !show_opt_progress && !empty_execute && inspector_inner.width >= 10 {
            let cta_y = inspector_inner.bottom().saturating_sub(cta_h);
            let i = &app.ctx.i18n;
            let ascii = app.theme.active.flags.ascii;
            if expanded && cta_h >= 2 {
                let btns = split_touch_buttons(
                    Rect::new(inspector_inner.x, cta_y, inspector_inner.width, cta_h),
                    2,
                );
                (Some(btns[0]), Some(btns[1]))
            } else {
                let enter_w = if ascii { 5usize } else { 1usize };
                let cta_label = match app.selected_row() {
                    Some(Row::Action(spec)) if !spec.fields.is_empty() => i.text("key.open"),
                    Some(Row::Action(_)) => i.text("key.accept"),
                    Some(Row::Execute) => i.text("key.run_checklist"),
                    _ => i.text("key.accept"),
                };
                let cta_w =
                    (enter_w + cta_label.width() + 5).min(inspector_inner.width as usize) as u16;
                let cta_rect = Rect::new(inspector_inner.x, cta_y, cta_w, 1);

                let copy_lbl = i.text("key.copy_cmd");
                let copy_w = if ascii {
                    format!("[c {copy_lbl}]").width() as u16
                } else {
                    format!(" c {copy_lbl} ").width() as u16
                };
                let copy_btn =
                    if cta_w.saturating_add(1).saturating_add(copy_w) <= inspector_inner.width {
                        Some(Rect::new(inspector_inner.x + cta_w + 1, cta_y, copy_w, 1))
                    } else {
                        None
                    };
                (Some(cta_rect), copy_btn)
            }
        } else {
            (None, None)
        };

    WorkbenchGeometry {
        list_area,
        section_bar,
        section_pills,
        section_overflow_left,
        section_overflow_right,
        list_rule_y,
        list_inner,
        list_scroll,
        visual_rows,
        selectable_rects,
        section_row_rects,
        prev_btn,
        next_btn,
        can_prev,
        can_next,
        page_text,
        inspector_area,
        inspector_inner,
        inspector_cta,
        inspector_copy_btn,
        stacked,
    }
}

fn build_section_bar(
    list_area: Rect,
    app: &App,
    density: ResolvedDensity,
) -> (Rect, Vec<SectionPill>, bool, bool, Option<u16>) {
    let Some(group) = app.active_group() else {
        return (Rect::default(), Vec::new(), false, false, None);
    };
    let Some(secs) = app.group_sections(group) else {
        return (Rect::default(), Vec::new(), false, false, None);
    };
    if list_area.width < 26 || list_area.height < 7 {
        return (Rect::default(), Vec::new(), false, false, None);
    }

    let i = &app.ctx.i18n;
    let pill_h = if density.is_expanded() && list_area.height >= 12 {
        2u16
    } else {
        1u16
    };
    let bar_x = list_area.x + 2;
    let bar_w = list_area.width.saturating_sub(4);
    let bar_y =
        list_area.y + super::render::tokens::card_top_inset(&app.theme.active, list_area.height);
    let show_counts = bar_w >= 40;

    let total_actions: usize = secs.iter().map(|(_, ids)| ids.len()).sum();
    let mut items: Vec<(Option<usize>, String, usize, u16)> = Vec::with_capacity(secs.len() + 1);
    let pill_width = |label: &str, count: usize| -> u16 {
        let extra = if show_counts && pill_h == 1 {
            1 + count.to_string().len()
        } else {
            0
        };
        let min_touch_w = if pill_h >= 2 { 8usize } else { 0 };
        (label.width() + extra + 2)
            .max(min_touch_w)
            .min(bar_w as usize) as u16
    };
    let all_label = i.text("ui.all_sections").to_owned();
    let all_w = pill_width(&all_label, total_actions);
    items.push((None, all_label, total_actions, all_w));
    for (idx, (sec_id, ids)) in secs.iter().enumerate() {
        let label = i
            .text(&format!(
                "section.{}.{}",
                super::state::GROUPS[group],
                sec_id
            ))
            .to_owned();
        let w = pill_width(&label, ids.len());
        items.push((Some(idx), label, ids.len(), w));
    }

    // Try multi-row wrapping first so wide/medium viewports display all section pills at once.
    let max_rows: usize = if pill_h >= 2 {
        if list_area.height >= 24 { 2 } else { 1 }
    } else if list_area.height >= 20 {
        3
    } else if list_area.height >= 15 {
        2
    } else {
        1
    };
    let mut wrapped: Vec<(usize, u16, usize)> = Vec::with_capacity(items.len());
    let mut row = 0usize;
    let mut col = 0u16;
    for (idx, (_, _, _, w)) in items.iter().enumerate() {
        if col > 0 && col.saturating_add(*w) > bar_w {
            row += 1;
            col = 0;
        }
        wrapped.push((row, col, idx));
        col = col.saturating_add(*w).saturating_add(1);
    }
    let total_rows = row + 1;

    if total_rows <= max_rows {
        let row_gap = u16::from(pill_h >= 2 && total_rows > 1);
        let row_stride = pill_h + row_gap;
        let pills = wrapped
            .into_iter()
            .map(|(r, x_off, idx)| {
                let (tab, ref label, count, w) = items[idx];
                SectionPill {
                    tab,
                    label: label.clone(),
                    count,
                    rect: Rect::new(
                        bar_x + x_off,
                        bar_y + (r as u16) * row_stride,
                        w.min(bar_w.saturating_sub(x_off)),
                        pill_h,
                    ),
                }
            })
            .collect();
        let total_h =
            (total_rows as u16) * pill_h + (total_rows.saturating_sub(1) as u16) * row_gap;
        let bar = Rect::new(bar_x, bar_y, bar_w, total_h);
        let rule_y = bar_y + total_h;
        return (bar, pills, false, false, Some(rule_y));
    }

    // Compact fallback: 1-strip horizontally windowed pills centered on the active pill.
    let active_idx = match app.section_tab(group) {
        None => 0,
        Some(sec) => (sec + 1).min(items.len().saturating_sub(1)),
    };
    let inner_x = bar_x + 2;
    let inner_w = bar_w.saturating_sub(4).max(1);
    let mut lo = active_idx;
    let mut hi = active_idx;
    let mut used = items[active_idx].3.min(inner_w);
    loop {
        let mut grew = false;
        if hi + 1 < items.len() {
            let need = items[hi + 1].3 + 1;
            if used + need <= inner_w {
                hi += 1;
                used += need;
                grew = true;
            }
        }
        if lo > 0 {
            let need = items[lo - 1].3 + 1;
            if used + need <= inner_w {
                lo -= 1;
                used += need;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let overflow_left = lo > 0;
    let overflow_right = hi + 1 < items.len();
    let start_x = if overflow_left { inner_x } else { bar_x };
    let max_right = if overflow_right {
        bar_x + bar_w.saturating_sub(2)
    } else {
        bar_x + bar_w
    };
    let mut cursor_x = start_x;
    let mut pills = Vec::with_capacity(hi - lo + 1);
    for (tab, label, count, w) in &items[lo..=hi] {
        let avail = max_right.saturating_sub(cursor_x);
        if avail == 0 {
            break;
        }
        let pw = (*w).min(avail);
        pills.push(SectionPill {
            tab: *tab,
            label: label.clone(),
            count: *count,
            rect: Rect::new(cursor_x, bar_y, pw, pill_h),
        });
        cursor_x = cursor_x.saturating_add(pw).saturating_add(1);
    }
    let bar = Rect::new(bar_x, bar_y, bar_w, pill_h);
    (
        bar,
        pills,
        overflow_left,
        overflow_right,
        Some(bar_y + pill_h),
    )
}

fn build_theme_drawer(
    area: Rect,
    count: usize,
    cursor: usize,
    density: ResolvedDensity,
) -> ThemeDrawerGeometry {
    let expanded = density.is_expanded() && area.height >= 24;
    let tab_h = if expanded { 2u16 } else { 1u16 };
    let row_height = if expanded { 2u16 } else { 1u16 };
    let toggle_h = if expanded && area.height >= 30 {
        2u16
    } else {
        1u16
    };

    let inner_x = area.x + 2;
    let inner_w = area.width.saturating_sub(4);
    let tab_y = (area.y + 1).min(area.bottom().saturating_sub(1));
    let half = inner_w.saturating_sub(1) / 2;
    let tab_palette = Rect::new(inner_x, tab_y, half, tab_h);
    let tab_style = Rect::new(
        inner_x + half + 1,
        tab_y,
        inner_w.saturating_sub(half + 1),
        tab_h,
    );

    let list_y = (tab_y + tab_h + 1).min(area.bottom().saturating_sub(1));
    let toggles_total = toggle_h * 3;
    let details_reserve = if expanded { 5u16 } else { 7u16 };
    let footer_reserve = 1 + toggles_total + details_reserve;
    let max_list_rows_h = area
        .height
        .saturating_sub((list_y - area.y) + footer_reserve)
        .max(row_height * 3);
    let full_need_h = (count as u16).saturating_mul(row_height);
    let need_pager = expanded && full_need_h > max_list_rows_h && max_list_rows_h >= 6;
    let pager_h = if need_pager { 2u16 } else { 0 };
    let list_h_cells = full_need_h.min(max_list_rows_h.saturating_sub(pager_h));
    let visible_entries = (list_h_cells / row_height).max(1) as usize;
    let list_area = Rect::new(
        inner_x,
        list_y,
        inner_w,
        visible_entries as u16 * row_height,
    );
    let list_scroll = list_offset(cursor, count, visible_entries);

    let (prev_btn, next_btn, can_prev, can_next) = if need_pager {
        let py = list_area.bottom();
        let btns = split_touch_buttons(Rect::new(inner_x, py, inner_w, pager_h), 2);
        (
            Some(btns[0]),
            Some(btns[1]),
            list_scroll > 0,
            list_scroll + visible_entries < count,
        )
    } else {
        (None, None, false, false)
    };

    let toggles_y = (list_area.bottom() + pager_h + 1).min(area.bottom().saturating_sub(2));
    let transparent_toggle = Rect::new(
        inner_x,
        toggles_y,
        inner_w,
        toggle_h.min(area.bottom().saturating_sub(toggles_y + 1).max(1)),
    );
    let motion_y = (transparent_toggle.bottom()).min(area.bottom().saturating_sub(1));
    let motion_toggle = Rect::new(
        inner_x,
        motion_y,
        inner_w,
        toggle_h.min(area.bottom().saturating_sub(motion_y + 1).max(1)),
    );
    let density_y = (motion_toggle.bottom()).min(area.bottom().saturating_sub(1));
    let density_toggle = Rect::new(
        inner_x,
        density_y,
        inner_w,
        toggle_h.min(area.bottom().saturating_sub(density_y + 1).max(1)),
    );
    let details_y = (density_toggle.bottom() + 1).min(area.bottom());
    let details_area = Rect::new(
        inner_x,
        details_y,
        inner_w,
        area.bottom().saturating_sub(details_y + 1),
    );

    ThemeDrawerGeometry {
        area,
        tab_palette,
        tab_style,
        list_area,
        list_scroll,
        row_height,
        prev_btn,
        next_btn,
        can_prev,
        can_next,
        transparent_toggle,
        motion_toggle,
        density_toggle,
        details_area,
    }
}

pub(super) fn centered(area: Rect, max_w: u16, height: u16) -> Rect {
    let margin_x = if area.width < 60 { 2 } else { 4 };
    let w = max_w
        .min(area.width.saturating_sub(margin_x))
        .max(36.min(area.width));
    let h = height
        .min(area.height.saturating_sub(2))
        .max(7.min(area.height));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

#[derive(Debug, Clone)]
pub(super) struct CommandPaletteGeometry {
    pub modal: Rect,
    pub input: Rect,
    pub list: Rect,
    pub row_height: u16,
    pub row_rects: Vec<(Rect, usize)>,
    pub prev_btn: Option<Rect>,
    pub next_btn: Option<Rect>,
    pub can_prev: bool,
    pub can_next: bool,
}

impl CommandPaletteGeometry {
    pub fn compute(
        area: Rect,
        results: &palette::Results,
        cursor: usize,
        density: ResolvedDensity,
    ) -> Self {
        let expanded = density.is_expanded();
        let row_height = if expanded { 2u16 } else { 1u16 };
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 78.min(area.width.saturating_sub(margin_x));
        let modal_h = if expanded {
            (area.height * 82 / 100)
                .clamp(16, 36)
                .min(area.height.saturating_sub(2))
        } else {
            (area.height * 72 / 100)
                .clamp(14, 24)
                .min(area.height.saturating_sub(2))
        };
        let modal = centered(area, modal_w, modal_h);
        let input = Rect::new(modal.x + 2, modal.y + 2, modal.width.saturating_sub(4), 1);
        let list_y = modal.y + 4;
        let raw_list_h = modal.bottom().saturating_sub(list_y + 2);
        let r_h = |row: &palette::PaletteRow| -> usize {
            if expanded && matches!(row, palette::PaletteRow::Item(_)) {
                row_height as usize
            } else {
                1
            }
        };
        let total_visual_h: usize = results.rows.iter().map(r_h).sum();
        let need_pager = expanded && total_visual_h > raw_list_h as usize && raw_list_h >= 6;
        let pager_h = if need_pager { 2u16 } else { 0 };
        let list_h = raw_list_h.saturating_sub(pager_h);
        let list = Rect::new(modal.x + 2, list_y, modal.width.saturating_sub(4), list_h);
        let sel_row = results.row_of(cursor);
        let scroll = if !expanded {
            list_offset(sel_row, results.rows.len(), list_h as usize)
        } else {
            let mut s = 0usize;
            while s < sel_row {
                let span: usize = results.rows[s..=sel_row].iter().map(r_h).sum();
                if span <= list_h as usize {
                    break;
                }
                s += 1;
            }
            s
        };
        let mut row_rects = Vec::new();
        let mut y = list.y;
        let mut last_idx = scroll;
        for (idx, row) in results.rows.iter().enumerate().skip(scroll) {
            let h = r_h(row) as u16;
            if y + h > list.bottom() {
                break;
            }
            row_rects.push((Rect::new(list.x, y, list.width, h), idx));
            last_idx = idx;
            y += h;
        }
        let (prev_btn, next_btn, can_prev, can_next) = if need_pager {
            let btns =
                split_touch_buttons(Rect::new(list.x, list.bottom(), list.width, pager_h), 2);
            (
                Some(btns[0]),
                Some(btns[1]),
                scroll > 0,
                !row_rects.is_empty() && last_idx + 1 < results.rows.len(),
            )
        } else {
            (None, None, false, false)
        };
        Self {
            modal,
            input,
            list,
            row_height,
            row_rects,
            prev_btn,
            next_btn,
            can_prev,
            can_next,
        }
    }

    pub fn cursor_at(&self, point: Position, results: &palette::Results) -> Option<usize> {
        if !self.list.contains(point) {
            return None;
        }
        let row_idx = self
            .row_rects
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, idx)| *idx)?;
        results.item_indices.iter().position(|idx| *idx == row_idx)
    }
}

#[derive(Debug, Clone)]
pub(super) struct FormGeometry {
    pub modal: Rect,
    pub fields: Vec<Rect>,
    pub chips: Vec<(usize, usize, Rect)>,
    pub buttons: [Rect; 2],
    /// Live simulated terminal preview area for `terminal.apply`.
    pub terminal_preview: Option<Rect>,
    /// Live connection & security preview card for `clash.socks.configure`.
    pub socks_preview: Option<Rect>,
}

/// Lay out wrapping choice chips for one field inside `[start_x, max_right)` starting at `top_y`.
/// Returns `(chip_rects, rows_used)`.
fn layout_chips(
    choices: &[&str],
    start_x: u16,
    max_right: u16,
    top_y: u16,
    max_bottom: u16,
    chip_h: u16,
) -> (Vec<(usize, Rect)>, u16) {
    let mut placed = Vec::with_capacity(choices.len());
    let mut cx = start_x;
    let mut cy = top_y;
    let row_gap = u16::from(chip_h >= 2);
    let mut rows = chip_h;
    for (c_idx, choice) in choices.iter().enumerate() {
        let cw = (choice.width() as u16 + 2).max(if chip_h >= 2 { 6 } else { 0 });
        if cx > start_x && cx + cw > max_right {
            cx = start_x;
            cy += chip_h + row_gap;
            rows += chip_h + row_gap;
        }
        if cy + chip_h <= max_bottom && cx + cw <= max_right {
            placed.push((c_idx, Rect::new(cx, cy, cw, chip_h)));
            cx += cw + 1;
        }
    }
    (placed, rows)
}

impl FormGeometry {
    pub fn compute(area: Rect, form: &Form, app: &App) -> Self {
        let density = app.resolved_density(area);
        let expanded = density.is_expanded();
        let chip_h = if expanded && area.height >= 28 {
            2u16
        } else {
            1u16
        };
        let btn_h = if expanded && area.height >= 24 {
            density.button_height()
        } else {
            1u16
        };
        let is_terminal = form.spec.id == "terminal.apply";
        let is_socks = form.spec.id == "clash.socks.configure";
        let has_preview = is_terminal || is_socks;
        let split_preview = has_preview && area.width >= 96;
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = if split_preview {
            let target = if is_socks { 106 } else { 104 };
            target.min(area.width.saturating_sub(4))
        } else {
            76.min(area.width.saturating_sub(margin_x))
        };
        let inner_w = modal_w.saturating_sub(4);
        let field_col_w = if split_preview {
            let target = if is_socks { 46 } else { 44 };
            target.min(inner_w.saturating_sub(36))
        } else {
            inner_w
        };

        // Measure chip row counts first so the modal height fits all wrapped chips cleanly.
        let chip_budget = field_col_w.saturating_sub(2);
        let mut field_heights = Vec::with_capacity(form.spec.fields.len());
        for (idx, field) in form.spec.fields.iter().enumerate() {
            let rows = if form.chip_field(idx) {
                layout_chips(field.choices, 0, chip_budget, 0, u16::MAX, chip_h).1
            } else if form.preset_chips(idx) {
                1 + layout_chips(field.choices, 0, chip_budget, 0, u16::MAX, chip_h).1
            } else {
                1
            };
            field_heights.push(1 + rows); // label + value/chip rows
        }

        let max_modal_h = area.height.saturating_sub(2);
        let footer_extra = btn_h.saturating_sub(1);
        let loose_fields_total: u16 = field_heights.iter().map(|&h| h + 1).sum::<u16>();
        let compact_vertical = (has_preview && !split_preview && area.height <= 26)
            || (4 + loose_fields_total + 5 + footer_extra > max_modal_h);
        let field_gap = if compact_vertical { 0u16 } else { 1u16 };
        let fields_total: u16 = field_heights.iter().map(|&h| h + field_gap).sum::<u16>();

        let header_rows = if compact_vertical { 2u16 } else { 4u16 };
        let stacked_preview_h = if is_terminal && !split_preview {
            4u16
        } else {
            0u16
        };
        let mut content_rows = header_rows + fields_total + stacked_preview_h;
        if form.error.is_some() {
            content_rows += 2;
        }
        content_rows += 5 + footer_extra; // command preview + buttons + hint
        if split_preview {
            content_rows = content_rows.max(25);
        }
        let modal = centered(area, modal_w, content_rows.clamp(12, max_modal_h));

        let btn_y = modal.bottom().saturating_sub(2 + btn_h);
        let cmd_y = btn_y.saturating_sub(2);
        let fields_top = modal.y + header_rows;
        let preview_limit = if is_terminal && !split_preview {
            let avail_before_cmd = cmd_y.saturating_sub(fields_top);
            if fields_total + 4 <= avail_before_cmd {
                cmd_y.saturating_sub(4)
            } else if fields_total + 3 <= avail_before_cmd {
                cmd_y.saturating_sub(3)
            } else {
                cmd_y
            }
        } else {
            cmd_y
        };
        let avail_fields_h = preview_limit.saturating_sub(fields_top);

        // Scroll fields vertically around the active field on very short mobile screens.
        let mut start_idx = 0usize;
        if fields_total > avail_fields_h && !field_heights.is_empty() {
            let active_field = form.index.min(field_heights.len().saturating_sub(1));
            let mut needed = 0u16;
            for (idx, &fh) in field_heights.iter().enumerate().take(active_field + 1) {
                needed = needed
                    .saturating_add(fh)
                    .saturating_add(if idx < active_field { field_gap } else { 0 });
            }
            while start_idx < active_field && needed > avail_fields_h {
                needed = needed.saturating_sub(field_heights[start_idx] + field_gap);
                start_idx += 1;
            }
        }

        let mut y = fields_top;
        let mut fields = Vec::with_capacity(form.spec.fields.len());
        let mut chips = Vec::new();
        let field_right = modal.x + 2 + field_col_w;

        for (idx, field) in form.spec.fields.iter().enumerate() {
            if idx < start_idx || y >= preview_limit {
                fields.push(Rect::default());
                continue;
            }
            let fh = field_heights[idx];
            let avail_h = preview_limit.saturating_sub(y);
            let rect = Rect::new(modal.x + 2, y, field_col_w, fh.min(avail_h));
            fields.push(rect);
            if form.chip_field(idx) && y + 1 < preview_limit {
                let (placed, _) = layout_chips(
                    field.choices,
                    modal.x + 4,
                    field_right,
                    y + 1,
                    preview_limit,
                    chip_h,
                );
                for (c_idx, chip_rect) in placed {
                    chips.push((idx, c_idx, chip_rect));
                }
            } else if form.preset_chips(idx) && y + 2 < preview_limit {
                let (placed, _) = layout_chips(
                    field.choices,
                    modal.x + 4,
                    field_right,
                    y + 2,
                    preview_limit,
                    chip_h,
                );
                for (c_idx, chip_rect) in placed {
                    chips.push((idx, c_idx, chip_rect));
                }
            }
            y = (y + fh + field_gap).min(preview_limit);
        }

        let right_preview = if split_preview {
            let px = modal.x + 2 + field_col_w + 3;
            let pw = modal.right().saturating_sub(px + 2);
            let py = modal.y + header_rows;
            let ph = btn_y.saturating_sub(py + 2);
            (pw >= 28 && ph >= 8).then_some(Rect::new(px, py, pw, ph))
        } else {
            None
        };
        let terminal_preview = if is_terminal {
            right_preview.or_else(|| {
                let py = y.min(preview_limit);
                let ph = cmd_y.saturating_sub(py);
                (ph >= 3 && inner_w >= 20).then_some(Rect::new(modal.x + 2, py, inner_w, ph))
            })
        } else {
            None
        };
        let socks_preview = if is_socks { right_preview } else { None };

        let buttons = if expanded && btn_h >= 2 {
            let btns = split_touch_buttons(Rect::new(modal.x + 2, btn_y, inner_w, btn_h), 2);
            [btns[0], btns[1]]
        } else {
            let primary_key = if form.mode == super::state::FormMode::Optimization {
                "ui.save_selection"
            } else {
                "ui.continue"
            };
            let max_btn_right = modal.right().saturating_sub(2);
            let b0_x = modal.x + 2;
            let b0_w = (app.ctx.i18n.text(primary_key).width() as u16 + 4)
                .min(max_btn_right.saturating_sub(b0_x));
            let b1_x = (b0_x + b0_w + 2).min(max_btn_right);
            let b1_w = (app.ctx.i18n.text("ui.cancel").width() as u16 + 4)
                .min(max_btn_right.saturating_sub(b1_x));
            [
                Rect::new(b0_x, btn_y, b0_w, 1),
                Rect::new(b1_x, btn_y, b1_w, 1),
            ]
        };

        Self {
            modal,
            fields,
            chips,
            buttons,
            terminal_preview,
            socks_preview,
        }
    }

    pub fn field_at(&self, point: Position) -> Option<usize> {
        self.fields.iter().position(|r| r.contains(point))
    }

    pub fn chip_at(&self, point: Position, slop: bool) -> Option<(usize, usize)> {
        hit_best(
            self.chips.iter().map(|&(fi, ci, r)| (r, (fi, ci))),
            point,
            slop,
        )
    }
}

/// Responsive geometry for the Interactive Terminal Prompt Preview Studio (`terminal.preview`).
#[derive(Debug, Clone)]
pub(super) struct PromptPreviewGeometry {
    pub modal: Rect,
    pub stacked: bool,
    pub left_pane: Rect,
    pub style_header_y: u16,
    pub style_rects: Vec<Rect>,
    pub color_header_x: u16,
    pub color_header_y: u16,
    pub color_header_w: u16,
    pub color_rects: Vec<Rect>,
    pub compact_colors: bool,
    pub time_btn: Rect,
    pub git_btn: Rect,
    pub shell_btn: Rect,
    pub view_btn: Rect,
    pub preview_box: Rect,
    pub meta_area: Rect,
    pub cmd_y: u16,
    pub apply_btn: Rect,
    pub close_btn: Rect,
}

impl PromptPreviewGeometry {
    #[cfg(test)]
    pub fn compute(area: Rect, i18n: &I18n) -> Self {
        Self::compute_for(area, i18n, 0, 0)
    }

    pub fn compute_for(area: Rect, i18n: &I18n, style_idx: usize, color_idx: usize) -> Self {
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 108.min(area.width.saturating_sub(margin_x));
        let modal_h = 30.min(area.height.saturating_sub(2));
        let modal = centered(area, modal_w, modal_h);

        let compact_h = modal.height < 26;
        let body_top = modal.y + if compact_h { 2 } else { 3 };
        let btn_y = modal.bottom().saturating_sub(3);
        let cmd_y = btn_y.saturating_sub(1);
        let body_bottom = cmd_y.saturating_sub(1).max(body_top + 4);
        let body_h = body_bottom.saturating_sub(body_top);

        let style_count = crate::ops::terminal::STYLES.len();
        let color_count = crate::ops::terminal::COLORS.len();

        let max_btn_right = modal.right().saturating_sub(2);
        let apply_x = modal.x + 2;
        let apply_w = (i18n.text("ui.prompt_apply_btn").width() as u16 + 4)
            .min(max_btn_right.saturating_sub(apply_x));
        let close_x = (apply_x + apply_w + 2).min(max_btn_right);
        let close_w =
            (i18n.text("key.close").width() as u16 + 4).min(max_btn_right.saturating_sub(close_x));
        let apply_btn = Rect::new(apply_x, btn_y, apply_w, 1);
        let close_btn = Rect::new(close_x, btn_y, close_w, 1);

        // Mobile / narrow stacked layout: full-width toggles + preview on top, 2-col scrolled selectors below.
        if modal.width < 72 {
            let ix = modal.x + 2;
            let iw = modal.width.saturating_sub(4);

            // Top toggle row spanning full inner width
            let toggles_y = body_top;
            let mut tx = ix;
            let mut next_btn = |ideal: u16, min_w: u16| -> Rect {
                let avail = (ix + iw).saturating_sub(tx);
                let w = if iw < 48 { min_w } else { ideal }.min(avail);
                let r = Rect::new(tx, toggles_y, w, 1);
                tx = (tx + w + 1).min(ix + iw);
                r
            };
            let time_btn = next_btn(14, 7);
            let git_btn = next_btn(12, 7);
            let shell_btn = next_btn(10, 8);
            let view_btn = next_btn(14, 9);

            let has_meta = body_h >= 13;
            let meta_h = u16::from(has_meta);
            let box_y = (toggles_y + 1).min(body_bottom);
            let box_h = if body_h >= 14 {
                7u16
            } else if body_h >= 11 {
                5u16
            } else {
                4u16.min(body_bottom.saturating_sub(box_y))
            };
            let preview_box =
                Rect::new(ix, box_y, iw, box_h.min(body_bottom.saturating_sub(box_y)));
            let meta_y = preview_box.bottom().min(body_bottom);
            let meta_area = Rect::new(
                ix,
                meta_y,
                iw,
                meta_h.min(body_bottom.saturating_sub(meta_y)),
            );

            let sel_top = meta_area.bottom().min(body_bottom);
            let sel_h = body_bottom.saturating_sub(sel_top);
            let left_pane = Rect::new(ix, sel_top, iw, sel_h);

            let half_w = iw.saturating_sub(3) / 2;
            let color_x = ix + half_w + 3;
            let color_w = (ix + iw).saturating_sub(color_x);
            let style_header_y = sel_top;
            let color_header_y = sel_top;
            let list_rows = sel_h.saturating_sub(1) as usize;

            let s_scroll = list_offset(style_idx, style_count, list_rows);
            let mut style_rects = vec![Rect::default(); style_count];
            for (offset, idx) in (s_scroll..style_count).take(list_rows).enumerate() {
                let y = style_header_y + 1 + offset as u16;
                if y < body_bottom {
                    style_rects[idx] = Rect::new(ix, y, half_w, 1);
                }
            }

            let c_scroll = list_offset(color_idx, color_count, list_rows);
            let mut color_rects = vec![Rect::default(); color_count];
            for (offset, idx) in (c_scroll..color_count).take(list_rows).enumerate() {
                let y = color_header_y + 1 + offset as u16;
                if y < body_bottom {
                    color_rects[idx] = Rect::new(color_x, y, color_w, 1);
                }
            }

            return Self {
                modal,
                stacked: true,
                left_pane,
                style_header_y,
                style_rects,
                color_header_x: color_x,
                color_header_y,
                color_header_w: color_w,
                color_rects,
                compact_colors: true,
                time_btn,
                git_btn,
                shell_btn,
                view_btn,
                preview_box,
                meta_area,
                cmd_y,
                apply_btn,
                close_btn,
            };
        }

        let left_w = if modal.width >= 100 {
            30
        } else if modal.width >= 88 {
            26
        } else {
            23
        };
        let left_pane = Rect::new(modal.x + 2, body_top, left_w, body_h);
        let right_x = left_pane.right() + 3;
        let right_w = modal.right().saturating_sub(right_x + 2);
        let right_pane = Rect::new(right_x, body_top, right_w, body_h);

        let style_header_y = left_pane.y;
        let mut style_rects = Vec::with_capacity(style_count);
        for idx in 0..style_count {
            let y = style_header_y + 1 + idx as u16;
            if y < left_pane.bottom() {
                style_rects.push(Rect::new(left_pane.x, y, left_pane.width, 1));
            }
        }

        // Fit all 9 color palettes: full rows with swatches when tall, or 2-column grid when compact.
        let compact_colors = body_h < (style_count + color_count + 4) as u16;
        let color_header_y = (style_header_y + 1 + style_count as u16 + u16::from(!compact_colors))
            .min(left_pane.bottom().saturating_sub(1));
        let mut color_rects = Vec::with_capacity(color_count);
        if compact_colors {
            let col_w = (left_pane.width.saturating_sub(1)) / 2;
            for idx in 0..color_count {
                let row = (idx / 2) as u16;
                let col = (idx % 2) as u16;
                let y = color_header_y + 1 + row;
                if y < left_pane.bottom() {
                    let x = left_pane.x + col * (col_w + 1);
                    let w = if col == 1 {
                        left_pane.right().saturating_sub(x)
                    } else {
                        col_w
                    };
                    color_rects.push(Rect::new(x, y, w, 1));
                }
            }
        } else {
            for idx in 0..color_count {
                let y = color_header_y + 1 + idx as u16;
                if y < left_pane.bottom() {
                    color_rects.push(Rect::new(left_pane.x, y, left_pane.width, 1));
                }
            }
        }

        // Right pane top toggle strip: [T] Time  [G] Git  [S] Shell  [V] Preview/TOML
        let toggles_y = right_pane.y;
        let mut tx = right_pane.x;
        let mut next_btn = |w: u16| -> Rect {
            let avail = right_pane.right().saturating_sub(tx);
            let actual = w.min(avail);
            let r = Rect::new(tx, toggles_y, actual, 1);
            tx = (tx + actual + 1).min(right_pane.right());
            r
        };
        let time_btn = next_btn(13);
        let git_btn = next_btn(12);
        let shell_btn = next_btn(11);
        let view_btn = next_btn(18);

        let meta_h = if right_pane.height >= 16 { 3u16 } else { 2u16 };
        let box_y = toggles_y + 2;
        let box_h = right_pane
            .bottom()
            .saturating_sub(box_y + meta_h + 1)
            .max(5);
        let preview_box = Rect::new(
            right_pane.x,
            box_y.min(right_pane.bottom()),
            right_pane.width,
            box_h.min(right_pane.bottom().saturating_sub(box_y)),
        );
        let meta_y = (preview_box.bottom() + 1).min(right_pane.bottom());
        let meta_area = Rect::new(
            right_pane.x,
            meta_y,
            right_pane.width,
            right_pane.bottom().saturating_sub(meta_y),
        );

        Self {
            modal,
            stacked: false,
            left_pane,
            style_header_y,
            style_rects,
            color_header_x: left_pane.x,
            color_header_y,
            color_header_w: left_pane.width,
            color_rects,
            compact_colors,
            time_btn,
            git_btn,
            shell_btn,
            view_btn,
            preview_box,
            meta_area,
            cmd_y,
            apply_btn,
            close_btn,
        }
    }

    pub fn style_at(&self, point: Position) -> Option<usize> {
        self.style_rects.iter().position(|r| r.contains(point))
    }

    pub fn color_at(&self, point: Position) -> Option<usize> {
        self.color_rects.iter().position(|r| r.contains(point))
    }
}

#[derive(Debug, Clone)]
pub(super) struct PickerGeometry {
    pub modal: Rect,
    pub list: Rect,
    pub scroll: usize,
    pub row_height: u16,
    pub prev_btn: Option<Rect>,
    pub next_btn: Option<Rect>,
    pub can_prev: bool,
    pub can_next: bool,
}

impl PickerGeometry {
    pub fn compute(
        area: Rect,
        total_matches: usize,
        cursor: usize,
        density: ResolvedDensity,
    ) -> Self {
        let expanded = density.is_expanded();
        let row_height = if expanded { 2u16 } else { 1u16 };
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 68.min(area.width.saturating_sub(margin_x));
        let target_h = (total_matches as u16).saturating_mul(row_height) + 7;
        let modal_h = if expanded {
            target_h.clamp(13, 30).min(area.height.saturating_sub(2))
        } else {
            target_h.clamp(11, 20).min(area.height.saturating_sub(2))
        };
        let modal = centered(area, modal_w, modal_h);
        let list_y = modal.y + 4;
        let raw_list_h = modal.bottom().saturating_sub(list_y + 2);
        let need_pager = expanded
            && (total_matches as u16).saturating_mul(row_height) > raw_list_h
            && raw_list_h >= 6;
        let pager_h = if need_pager { 2u16 } else { 0 };
        let list_h = raw_list_h.saturating_sub(pager_h);
        let visible_items = (list_h / row_height).max(1) as usize;
        let list = Rect::new(
            modal.x + 2,
            list_y,
            modal.width.saturating_sub(4),
            visible_items as u16 * row_height,
        );
        let scroll = list_offset(cursor, total_matches, visible_items);
        let (prev_btn, next_btn, can_prev, can_next) = if need_pager {
            let btns =
                split_touch_buttons(Rect::new(list.x, list.bottom(), list.width, pager_h), 2);
            (
                Some(btns[0]),
                Some(btns[1]),
                scroll > 0,
                scroll + visible_items < total_matches,
            )
        } else {
            (None, None, false, false)
        };
        Self {
            modal,
            list,
            scroll,
            row_height,
            prev_btn,
            next_btn,
            can_prev,
            can_next,
        }
    }

    pub fn item_at(&self, point: Position, count: usize) -> Option<usize> {
        if !self.list.contains(point) || self.row_height == 0 {
            return None;
        }
        let idx = ((point.y - self.list.y) / self.row_height) as usize + self.scroll;
        (idx < count).then_some(idx)
    }
}

#[derive(Debug, Clone)]
pub(super) struct ConfirmationGeometry {
    pub modal: Rect,
    pub body: Rect,
    pub buttons: [Rect; 2],
    pub total_lines: usize,
}

impl ConfirmationGeometry {
    pub fn compute(
        area: Rect,
        confirm: &Confirmation,
        i18n: &I18n,
        density: ResolvedDensity,
    ) -> Self {
        let expanded = density.is_expanded();
        let btn_h = if expanded && area.height >= 20 {
            density.button_height()
        } else {
            1u16
        };
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 72.min(area.width.saturating_sub(margin_x));
        let text_w = modal_w.saturating_sub(4).max(1) as usize;
        let wrapped = super::render::wrap_words(&confirm.text, text_w);
        let total_lines = wrapped.len();
        let footer_rows = 4 + btn_h;
        let modal_h = (total_lines as u16 + footer_rows + 3)
            .clamp(10 + btn_h.saturating_sub(1), area.height.saturating_sub(2));
        let modal = centered(area, modal_w, modal_h);
        let body = Rect::new(
            modal.x + 2,
            modal.y + 2,
            modal.width.saturating_sub(4),
            modal.height.saturating_sub(footer_rows + 2),
        );
        let btn_y = modal.bottom().saturating_sub(2 + btn_h);
        let buttons = if expanded && btn_h >= 2 {
            let btns = split_touch_buttons(
                Rect::new(modal.x + 2, btn_y, modal.width.saturating_sub(4), btn_h),
                2,
            );
            [btns[0], btns[1]]
        } else {
            let max_btn_right = modal.right().saturating_sub(2);
            let b0_x = modal.x + 2;
            let b0_w =
                (i18n.text("ui.cancel").width() as u16 + 4).min(max_btn_right.saturating_sub(b0_x));
            let b1_x = (b0_x + b0_w + 2).min(max_btn_right);
            let b1_w = (i18n.text("ui.confirm_execute").width() as u16 + 4)
                .min(max_btn_right.saturating_sub(b1_x));
            [
                Rect::new(b0_x, btn_y, b0_w, 1),
                Rect::new(b1_x, btn_y, b1_w, 1),
            ]
        };
        Self {
            modal,
            body,
            buttons,
            total_lines,
        }
    }

    pub fn max_scroll(&self) -> u16 {
        self.total_lines
            .saturating_sub(self.body.height as usize)
            .min(u16::MAX as usize) as u16
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LogModalButtons {
    pub copy: Rect,
    pub plain: Rect,
}

fn compute_log_drawer_hits(area: Rect, app: &App) -> (Option<Rect>, Option<Rect>, Option<Rect>) {
    if area.height == 0 || area.width < 34 {
        return (None, None, None);
    }
    let i = &app.ctx.i18n;
    let ascii = app.theme.active.flags.ascii;
    let count_str = format!(" {} ", app.logs.len());
    let expand_str = if ascii { "[F2]" } else { " F2 " };
    let plain_lbl = i.text("key.plain_view");
    let plain_str = if ascii {
        format!("[V {plain_lbl}]")
    } else {
        format!(" V {plain_lbl} ")
    };
    let copy_str = if ascii {
        format!("[y {}]", i.text("key.copy"))
    } else {
        format!(" y {} ", i.text("key.copy"))
    };

    let right_edge = area.right().saturating_sub(2);
    let mut cursor = right_edge.saturating_sub(count_str.width() as u16);

    let expand_w = expand_str.width() as u16;
    let expand = (cursor >= area.x + 18 + expand_w).then(|| {
        cursor = cursor.saturating_sub(expand_w);
        let r = Rect::new(cursor, area.y, expand_w, 1);
        cursor = cursor.saturating_sub(1);
        r
    });
    let plain_w = plain_str.width() as u16;
    let plain = (cursor >= area.x + 18 + plain_w).then(|| {
        cursor = cursor.saturating_sub(plain_w);
        let r = Rect::new(cursor, area.y, plain_w, 1);
        cursor = cursor.saturating_sub(1);
        r
    });
    let copy_w = copy_str.width() as u16;
    let copy = (cursor >= area.x + 18 + copy_w).then(|| {
        cursor = cursor.saturating_sub(copy_w);
        Rect::new(cursor, area.y, copy_w, 1)
    });
    (copy, plain, expand)
}

pub(super) fn full_log_modal_buttons(modal: Rect, app: &App) -> LogModalButtons {
    let i = &app.ctx.i18n;
    let ascii = app.theme.active.flags.ascii;
    let y = modal.bottom().saturating_sub(2);
    let copy_lbl = i.text("key.copy_logs");
    let plain_lbl = i.text("key.plain_view");
    let copy_w = if ascii {
        format!("[c {copy_lbl}]").width() as u16
    } else {
        format!(" c {copy_lbl} ").width() as u16
    };
    let plain_w = if ascii {
        format!("[V {plain_lbl}]").width() as u16
    } else {
        format!(" V {plain_lbl} ").width() as u16
    };
    let right_edge = modal.right().saturating_sub(2);
    let plain_x = right_edge.saturating_sub(plain_w);
    let copy_x = plain_x.saturating_sub(copy_w + 1);
    LogModalButtons {
        copy: Rect::new(copy_x, y, copy_w, 1),
        plain: Rect::new(plain_x, y, plain_w, 1),
    }
}

pub(super) fn notice_modal_rects(area: Rect, text: &str) -> (Rect, Rect) {
    let margin_x = if area.width < 60 { 2 } else { 4 };
    let w = 64.min(area.width.saturating_sub(margin_x));
    let wrapped = super::render::wrap_words(text, w.saturating_sub(4).max(1) as usize);
    let h = (wrapped.len() as u16 + 6).clamp(8, area.height.saturating_sub(2));
    let modal = centered(area, w, h);
    let body = Rect::new(
        modal.x + 2,
        modal.y + 2,
        modal.width.saturating_sub(4),
        modal.height.saturating_sub(4),
    );
    (modal, body)
}

#[derive(Debug, Clone)]
pub(super) struct SocksSecretGeometry {
    pub modal: Rect,
    pub body: Rect,
    pub rule_y: Option<u16>,
    pub btn_socks5h: Rect,
    pub btn_socks5: Rect,
    pub btn_password: Rect,
    pub btn_curl: Rect,
    pub btn_close: Rect,
}

impl SocksSecretGeometry {
    pub fn compute(
        area: Rect,
        creds: &crate::ops::clash::socks::RevealedCredentials,
        i18n: &I18n,
        ascii: bool,
        density: ResolvedDensity,
    ) -> Self {
        let expanded = density.is_expanded() && area.height >= 24;
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 88.min(area.width.saturating_sub(margin_x));
        let inner_w = modal_w.saturating_sub(4).max(1) as usize;

        let uri_h = creds.masked_uri("socks5h");
        let uri_5 = creds.masked_uri("socks5");
        let curl = creds.masked_curl_cmd();
        let file_str = creds.auth_file.display().to_string();

        // Estimate natural body line count so the modal wraps snugly without clipping masked URIs.
        let mut lines = 0usize;
        lines += super::render::wrap_words(i18n.text("ui.socks_secret_notice"), inner_w).len();
        lines += 2; // blank + listen/mode summary
        lines += if creds.has_auth { 3 } else { 2 }; // auth rows + blank
        lines += 1 + super::render::wrap_cells(&uri_h, inner_w).len() + 1;
        lines += 1 + super::render::wrap_cells(&uri_5, inner_w).len() + 1;
        lines += 1 + super::render::wrap_cells(&curl, inner_w).len() + 1;
        if creds.has_auth {
            lines += 1 + super::render::wrap_cells(&file_str, inner_w).len();
        }

        let btn_rows_h = if expanded { 5u16 } else { 1u16 };
        let max_h = area.height.saturating_sub(2).max(1);
        let chrome = if max_h >= 12 {
            4u16 + btn_rows_h
        } else {
            3u16 + btn_rows_h
        };
        let modal_h = (lines as u16)
            .saturating_add(chrome)
            .clamp((11 + btn_rows_h).min(max_h), max_h);
        let modal = centered(area, modal_w, modal_h);
        let rule_y =
            (modal.height >= 9 + btn_rows_h).then(|| modal.bottom().saturating_sub(2 + btn_rows_h));
        let body_h = modal
            .height
            .saturating_sub(if rule_y.is_some() { 4 } else { 3 } + btn_rows_h);
        let body = Rect::new(
            modal.x + 2,
            modal.y + 2,
            modal.width.saturating_sub(4),
            body_h,
        );

        let (btn_socks5h, btn_socks5, btn_password, btn_curl, btn_close) = if expanded {
            let row1_y = modal.bottom().saturating_sub(6);
            let row2_y = row1_y + 3;
            let row1 = split_touch_buttons(
                Rect::new(modal.x + 2, row1_y, modal.width.saturating_sub(4), 2),
                if creds.has_auth { 3 } else { 2 },
            );
            let row2 = split_touch_buttons(
                Rect::new(modal.x + 2, row2_y, modal.width.saturating_sub(4), 2),
                2,
            );
            if creds.has_auth {
                (row1[0], row1[1], row1[2], row2[0], row2[1])
            } else {
                (row1[0], row1[1], Rect::default(), row2[0], row2[1])
            }
        } else {
            let btn_y = modal.bottom().saturating_sub(2);
            let max_right = modal.right().saturating_sub(2);
            let mut x = modal.x + 2;
            let fmt_btn = |key: &str, label: &str| -> u16 {
                if ascii {
                    format!("[{key} {label}]").width() as u16
                } else {
                    format!(" {key} {label} ").width() as u16
                }
            };
            let mut place = |w: u16| -> Rect {
                if x + w > max_right || w == 0 {
                    return Rect::default();
                }
                let r = Rect::new(x, btn_y, w, 1);
                x = x.saturating_add(w + 1);
                r
            };

            let b1 = place(fmt_btn("1", "SOCKS5h"));
            let b2 = place(fmt_btn("2", "SOCKS5"));
            let b3 = if creds.has_auth {
                place(fmt_btn("3", i18n.text("ui.socks_secret_btn_pass")))
            } else {
                Rect::default()
            };
            let b4 = place(fmt_btn("4", "curl"));
            let b5 = place(fmt_btn("Esc", i18n.text("key.close")));
            (b1, b2, b3, b4, b5)
        };

        Self {
            modal,
            body,
            rule_y,
            btn_socks5h,
            btn_socks5,
            btn_password,
            btn_curl,
            btn_close,
        }
    }
}

pub(super) fn full_log_modal(area: Rect) -> (Rect, Rect) {
    let margin_x = if area.width < 60 { 2 } else { 4 };
    let modal = centered(
        area,
        area.width.saturating_sub(margin_x),
        area.height.saturating_sub(2),
    );
    let body = Rect::new(
        modal.x + 2,
        modal.y + 2,
        modal.width.saturating_sub(4),
        modal.height.saturating_sub(4),
    );
    (modal, body)
}

pub(super) fn log_body(app: &App, area: Rect) -> Rect {
    if !fits(area) {
        return Rect::default();
    }
    if app.log_modal {
        return full_log_modal(area).1;
    }
    LayoutRegions::compute(area, app).log_body
}
