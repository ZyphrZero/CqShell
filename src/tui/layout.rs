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
    pub item_rects: Vec<(Rect, NavTarget)>,
    pub collapse_btn: Option<Rect>,
    pub theme_btn: Option<Rect>,
    pub scroll: usize,
}

impl SidebarGeometry {
    pub fn target_at(&self, point: Position) -> Option<NavTarget> {
        self.item_rects
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, target)| *target)
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
    pub fn card_at(&self, point: Position) -> Option<usize> {
        self.quick_cards
            .iter()
            .position(|rect| rect.contains(point))
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
    pub inspector_area: Rect,
    pub inspector_inner: Rect,
    pub inspector_cta: Option<Rect>,
    pub inspector_copy_btn: Option<Rect>,
    pub stacked: bool,
}

impl WorkbenchGeometry {
    /// Returns the section tab filter (`None` = All, `Some(idx)` = section) if `point` hits a pill.
    pub fn section_pill_at(&self, point: Position) -> Option<Option<usize>> {
        self.section_pills
            .iter()
            .find(|pill| pill.rect.contains(point))
            .map(|pill| pill.tab)
    }

    /// Returns the section ID if `point` clicks a section header row in the list.
    pub fn section_row_at(&self, point: Position, rows: &[Row]) -> Option<&'static str> {
        if !self.list_inner.contains(point) {
            return None;
        }
        let visual_row = (point.y - self.list_inner.y) as usize + self.list_scroll;
        match rows.get(visual_row)? {
            Row::Section(sec_id, _) => Some(*sec_id),
            _ => None,
        }
    }

    /// Returns the selectable row index under `point`, if any.
    pub fn selectable_at(&self, point: Position, rows: &[Row]) -> Option<usize> {
        if !self.list_inner.contains(point) {
            return None;
        }
        let visual_row = (point.y - self.list_inner.y) as usize + self.list_scroll;
        let row = rows.get(visual_row)?;
        if !row.selectable() {
            return None;
        }
        Some(rows[..visual_row].iter().filter(|r| r.selectable()).count())
    }
}

#[derive(Debug, Clone)]
pub(super) struct ThemeDrawerGeometry {
    pub area: Rect,
    pub tab_palette: Rect,
    pub tab_style: Rect,
    pub list_area: Rect,
    pub list_scroll: usize,
    pub transparent_toggle: Rect,
    pub motion_toggle: Rect,
    pub details_area: Rect,
}

impl ThemeDrawerGeometry {
    pub fn entry_at(&self, point: Position, count: usize) -> Option<usize> {
        if !self.list_area.contains(point) {
            return None;
        }
        let idx = (point.y - self.list_area.y) as usize + self.list_scroll;
        (idx < count).then_some(idx)
    }
}

#[derive(Debug, Clone)]
pub(super) struct LayoutRegions {
    pub topbar: Rect,
    pub topbar_hits: TopBarHits,
    pub sidebar: Option<SidebarGeometry>,
    pub workspace: Rect,
    pub overview: Option<OverviewGeometry>,
    pub workbench: Option<WorkbenchGeometry>,
    pub docker: Option<super::docker::layout::DockerGeometry>,
    pub theme_drawer: Option<ThemeDrawerGeometry>,
    pub logs: Option<Rect>,
    pub log_body: Rect,
    pub log_copy_btn: Option<Rect>,
    pub log_plain_btn: Option<Rect>,
    pub log_expand_btn: Option<Rect>,
    pub footer: Rect,
}

impl LayoutRegions {
    pub fn compute(area: Rect, app: &App) -> Self {
        let breakpoint = Breakpoint::of(area.width);
        let topbar = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        let footer_y = area.bottom().saturating_sub(1);
        let footer = Rect::new(area.x, footer_y, area.width, 1.min(area.height));

        let mid_height = area.height.saturating_sub(2);
        let mid = Rect::new(area.x, area.y + 1, area.width, mid_height);

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

        // Sidebar docking vs overlay (Docker Focus mode hides docked sidebar for full width)
        let docker_focus = app.is_docker_view() && app.docker.fullscreen;
        let docked_sidebar = match breakpoint {
            Breakpoint::Compact | Breakpoint::Mobile => false,
            Breakpoint::Medium | Breakpoint::Wide => !app.sidebar_collapsed && !docker_focus,
        };
        let sidebar_width = match breakpoint {
            Breakpoint::Wide => 22,
            Breakpoint::Medium => 19,
            Breakpoint::Compact | Breakpoint::Mobile => 24,
        };

        let (sidebar, workspace) = if docked_sidebar && body.width > sidebar_width + 20 {
            let side_rect = Rect::new(body.x, body.y, sidebar_width, body.height);
            let ws_rect = Rect::new(
                body.x + sidebar_width + 1,
                body.y,
                body.width.saturating_sub(sidebar_width + 1),
                body.height,
            );
            (Some(build_sidebar(side_rect, false, app)), ws_rect)
        } else if app.sidebar_overlay {
            let overlay_rect =
                Rect::new(body.x, body.y, sidebar_width.min(body.width), body.height);
            (Some(build_sidebar(overlay_rect, true, app)), body)
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
            if workspace.width >= 78 {
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
                    )),
                )
            } else {
                (
                    workspace,
                    Some(build_theme_drawer(
                        workspace,
                        drawer.active_count(),
                        drawer.active_cursor(),
                    )),
                )
            }
        } else {
            (workspace, None)
        };

        let (overview, workbench, docker) = match app.nav {
            NavTarget::Overview => (Some(build_overview(content_area, app)), None, None),
            NavTarget::Optimization | NavTarget::Group(_) if app.is_docker_view() => (
                None,
                None,
                Some(super::docker::layout::DockerGeometry::compute(
                    content_area,
                    app,
                    theme_drawer.is_some(),
                )),
            ),
            NavTarget::Optimization | NavTarget::Group(_) => (
                None,
                Some(build_workbench(content_area, app, theme_drawer.is_some())),
                None,
            ),
        };

        // Topbar interactive hit targets (aligned with topbar renderer)
        let topbar_hits = compute_topbar_hits(topbar, app, docked_sidebar);
        let (log_copy_btn, log_plain_btn, log_expand_btn) = logs
            .map(|rect| compute_log_drawer_hits(rect, app))
            .unwrap_or((None, None, None));

        Self {
            topbar,
            topbar_hits,
            sidebar,
            workspace,
            overview,
            workbench,
            docker,
            theme_drawer,
            logs,
            log_body,
            log_copy_btn,
            log_plain_btn,
            log_expand_btn,
            footer,
        }
    }
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

fn compute_topbar_hits(bar: Rect, app: &App, docked_sidebar: bool) -> TopBarHits {
    let mut hits = TopBarHits::default();
    if bar.height == 0 || bar.width < 20 {
        return hits;
    }
    if !docked_sidebar {
        let nav_label = format!(" {} ", app.ctx.i18n.text("nav.toggle"));
        let w = (nav_label.width() as u16 + 2).min(bar.width);
        hits.nav_toggle = Some(Rect::new(bar.x + 1, bar.y, w, 1));
    }
    let lang_text = topbar_lang_label(app, bar.width);
    let lang_w = lang_text.width() as u16 + 2;
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
        if bar.width >= 80 {
            let x = bar.x + (bar.width.saturating_sub(pill_w.min(bar.width / 3))) / 2;
            hits.running_pill = Some(Rect::new(x, bar.y, pill_w.min(bar.width / 3), 1));
        } else {
            let right_after_pill = lang_w + 2;
            let x = bar.right().saturating_sub(right_after_pill + pill_w);
            if x > hits.nav_toggle.map_or(bar.x, |r| r.right()) {
                hits.running_pill = Some(Rect::new(x, bar.y, pill_w, 1));
            }
        }
    }
    hits
}

fn build_sidebar(area: Rect, overlay: bool, app: &App) -> SidebarGeometry {
    let top_inset = if overlay {
        1
    } else {
        super::render::tokens::card_top_inset(&app.theme.active, area.height)
    };
    let inner_y = area.y + top_inset;
    let framed_bottom = u16::from(overlay || app.theme.active.flags.framed);
    let footer_h = if area.height >= 8 {
        2 + framed_bottom
    } else {
        0
    };
    let list_h = area.height.saturating_sub(top_inset + footer_h) as usize;

    let is_default_order = app.category_order == default_category_order();
    let use_sections = is_default_order && list_h >= 18;
    let use_spacers = use_sections && list_h >= 21;

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

    let selected_row = rows
        .iter()
        .position(|r| matches!(r, SidebarRow::Item(t) if *t == app.nav))
        .unwrap_or(0);
    let scroll = list_offset(selected_row, rows.len(), list_h);

    let mut item_rects = Vec::new();
    for (offset, row) in rows.iter().skip(scroll).take(list_h).enumerate() {
        if let SidebarRow::Item(target) = *row {
            let rect = Rect::new(
                area.x + 1,
                inner_y + offset as u16,
                area.width.saturating_sub(2),
                1,
            );
            item_rects.push((rect, target));
        }
    }

    let (collapse_btn, theme_btn) = if footer_h > 0 {
        let btn_y = area.bottom().saturating_sub(1 + framed_bottom);
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
    } else {
        (None, None)
    };

    SidebarGeometry {
        area,
        overlay,
        rows,
        item_rects,
        collapse_btn,
        theme_btn,
        scroll,
    }
}

fn build_overview(area: Rect, app: &App) -> OverviewGeometry {
    let stacked_metrics = area.width < 76;
    let use_unified_quick = stacked_metrics || area.height < 20;

    if use_unified_quick {
        let min_quick_h = 6u16.min(area.height);
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
        let gap = u16::from((7..10).contains(&remain));
        let quick_y = area.y + metrics_h + gap;
        let quick_avail = area.bottom().saturating_sub(quick_y);
        let row_h = if quick_avail >= 10 { 2u16 } else { 1u16 };
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
        let row_h = (cards_h.saturating_sub(1) / 2).clamp(3, 4);
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

fn build_workbench(area: Rect, app: &App, drawer_open: bool) -> WorkbenchGeometry {
    let side_by_side = !drawer_open && area.width >= 82;
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
        let insp_h = if area.height >= 20 {
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
        build_section_bar(list_area, app);

    let pad_top = match list_rule_y {
        Some(rule_y) => (rule_y + 1).saturating_sub(list_area.y),
        None => 2u16,
    };
    let pad_bottom = 1u16;
    let list_inner = Rect::new(
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

    let list_scroll = if let Some(group) = app.active_group() {
        let rows = app.rows(group);
        let sel_visual = if rows.iter().any(Row::selectable) {
            row_index(&rows, app.selected_in(group))
        } else {
            0
        };
        let mut scroll = list_offset(sel_visual, rows.len(), list_inner.height as usize);
        if scroll > 0 && matches!(rows.get(scroll - 1), Some(Row::Section(..))) {
            scroll -= 1;
        }
        scroll
    } else {
        0
    };

    let (inspector_cta, inspector_copy_btn) = if !stacked
        && inspector_inner.width >= 10
        && inspector_inner.height >= 6
    {
        let cta_y = inspector_inner.bottom().saturating_sub(1);
        let i = &app.ctx.i18n;
        let ascii = app.theme.active.flags.ascii;
        let enter_w = if ascii { 5usize } else { 1usize };
        let cta_label = match app.selected_row() {
            Some(Row::Action(spec)) if !spec.fields.is_empty() => i.text("key.open"),
            Some(Row::Action(_)) => i.text("key.accept"),
            Some(Row::Execute) => i.text("key.run_checklist"),
            _ => i.text("key.accept"),
        };
        let cta_w = (enter_w + cta_label.width() + 5).min(inspector_inner.width as usize) as u16;
        let cta_rect = Rect::new(inspector_inner.x, cta_y, cta_w, 1);

        let copy_lbl = i.text("key.copy_cmd");
        let copy_w = if ascii {
            format!("[c {copy_lbl}]").width() as u16
        } else {
            format!(" c {copy_lbl} ").width() as u16
        };
        let copy_btn = if cta_w.saturating_add(1).saturating_add(copy_w) <= inspector_inner.width {
            Some(Rect::new(inspector_inner.x + cta_w + 1, cta_y, copy_w, 1))
        } else {
            None
        };
        (Some(cta_rect), copy_btn)
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
    let bar_x = list_area.x + 2;
    let bar_w = list_area.width.saturating_sub(4);
    let bar_y =
        list_area.y + super::render::tokens::card_top_inset(&app.theme.active, list_area.height);
    let show_counts = bar_w >= 40;

    let total_actions: usize = secs.iter().map(|(_, ids)| ids.len()).sum();
    let mut items: Vec<(Option<usize>, String, usize, u16)> = Vec::with_capacity(secs.len() + 1);
    let pill_width = |label: &str, count: usize| -> u16 {
        let extra = if show_counts {
            1 + count.to_string().len()
        } else {
            0
        };
        (label.width() + extra + 2).min(bar_w as usize) as u16
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
    let max_rows: usize = if list_area.height >= 20 {
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
                        bar_y + r as u16,
                        w.min(bar_w.saturating_sub(x_off)),
                        1,
                    ),
                }
            })
            .collect();
        let bar = Rect::new(bar_x, bar_y, bar_w, total_rows as u16);
        let rule_y = bar_y + total_rows as u16;
        return (bar, pills, false, false, Some(rule_y));
    }

    // Compact fallback: 1-row horizontally windowed strip centered on the active pill.
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
            rect: Rect::new(cursor_x, bar_y, pw, 1),
        });
        cursor_x = cursor_x.saturating_add(pw).saturating_add(1);
    }
    let bar = Rect::new(bar_x, bar_y, bar_w, 1);
    (bar, pills, overflow_left, overflow_right, Some(bar_y + 1))
}

fn build_theme_drawer(area: Rect, count: usize, cursor: usize) -> ThemeDrawerGeometry {
    let inner_x = area.x + 2;
    let inner_w = area.width.saturating_sub(4);
    let tab_y = (area.y + 1).min(area.bottom().saturating_sub(1));
    let half = inner_w.saturating_sub(1) / 2;
    let tab_palette = Rect::new(inner_x, tab_y, half, 1);
    let tab_style = Rect::new(
        inner_x + half + 1,
        tab_y,
        inner_w.saturating_sub(half + 1),
        1,
    );

    let list_y = (tab_y + 2).min(area.bottom().saturating_sub(1));
    let footer_reserve = 9u16;
    let list_h = area
        .height
        .saturating_sub(3 + footer_reserve)
        .max(4)
        .min(count as u16);
    let list_area = Rect::new(inner_x, list_y, inner_w, list_h);
    let list_scroll = list_offset(cursor, count, list_h as usize);

    let toggles_y = (list_area.bottom() + 1).min(area.bottom().saturating_sub(2));
    let transparent_toggle = Rect::new(inner_x, toggles_y, inner_w, 1);
    let motion_toggle = Rect::new(
        inner_x,
        (toggles_y + 1).min(area.bottom().saturating_sub(1)),
        inner_w,
        1,
    );
    let details_y = (motion_toggle.bottom() + 1).min(area.bottom());
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
        transparent_toggle,
        motion_toggle,
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
    pub scroll: usize,
}

impl CommandPaletteGeometry {
    pub fn compute(area: Rect, results: &palette::Results, cursor: usize) -> Self {
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 78.min(area.width.saturating_sub(margin_x));
        let modal_h = (area.height * 72 / 100)
            .clamp(14, 24)
            .min(area.height.saturating_sub(2));
        let modal = centered(area, modal_w, modal_h);
        let input = Rect::new(modal.x + 2, modal.y + 2, modal.width.saturating_sub(4), 1);
        let list_y = modal.y + 4;
        let list_h = modal.bottom().saturating_sub(list_y + 2);
        let list = Rect::new(modal.x + 2, list_y, modal.width.saturating_sub(4), list_h);
        let sel_row = results.row_of(cursor);
        let scroll = list_offset(sel_row, results.rows.len(), list_h as usize);
        Self {
            modal,
            input,
            list,
            scroll,
        }
    }

    pub fn cursor_at(&self, point: Position, results: &palette::Results) -> Option<usize> {
        if !self.list.contains(point) {
            return None;
        }
        let row_idx = (point.y - self.list.y) as usize + self.scroll;
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
}

/// Lay out wrapping choice chips for one field inside `[start_x, max_right)` starting at `top_y`.
/// Returns `(chip_rects, rows_used)`.
fn layout_chips(
    choices: &[&str],
    start_x: u16,
    max_right: u16,
    top_y: u16,
    max_bottom: u16,
) -> (Vec<(usize, Rect)>, u16) {
    let mut placed = Vec::with_capacity(choices.len());
    let mut cx = start_x;
    let mut cy = top_y;
    let mut rows = 1u16;
    for (c_idx, choice) in choices.iter().enumerate() {
        let cw = choice.width() as u16 + 2;
        if cx > start_x && cx + cw > max_right {
            cx = start_x;
            cy += 1;
            rows += 1;
        }
        if cy < max_bottom && cx + cw <= max_right {
            placed.push((c_idx, Rect::new(cx, cy, cw, 1)));
            cx += cw + 1;
        }
    }
    (placed, rows)
}

impl FormGeometry {
    pub fn compute(area: Rect, form: &Form, app: &App) -> Self {
        let is_terminal = form.spec.id == "terminal.apply";
        let split_preview = is_terminal && area.width >= 96;
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = if split_preview {
            104.min(area.width.saturating_sub(4))
        } else {
            76.min(area.width.saturating_sub(margin_x))
        };
        let inner_w = modal_w.saturating_sub(4);
        let field_col_w = if split_preview {
            44.min(inner_w.saturating_sub(36))
        } else {
            inner_w
        };

        // Measure chip row counts first so the modal height fits all wrapped chips cleanly.
        let chip_budget = field_col_w.saturating_sub(2);
        let mut field_heights = Vec::with_capacity(form.spec.fields.len());
        for (idx, field) in form.spec.fields.iter().enumerate() {
            let rows = if form.chip_field(idx) {
                layout_chips(field.choices, 0, chip_budget, 0, u16::MAX).1
            } else if form.preset_chips(idx) {
                1 + layout_chips(field.choices, 0, chip_budget, 0, u16::MAX).1
            } else {
                1
            };
            field_heights.push(1 + rows); // label + value/chip rows
        }

        let max_modal_h = area.height.saturating_sub(2);
        let loose_fields_total: u16 = field_heights.iter().map(|&h| h + 1).sum::<u16>();
        let compact_vertical = (is_terminal && !split_preview && area.height <= 26)
            || (!split_preview && (4 + loose_fields_total + 5 > max_modal_h));
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
        content_rows += 5; // command preview + buttons + hint
        if split_preview {
            content_rows = content_rows.max(25);
        }
        let modal = centered(area, modal_w, content_rows.clamp(12, max_modal_h));

        let btn_y = modal.bottom().saturating_sub(3);
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
                );
                for (c_idx, chip_rect) in placed {
                    chips.push((idx, c_idx, chip_rect));
                }
            }
            y = (y + fh + field_gap).min(preview_limit);
        }

        let terminal_preview = if split_preview {
            let px = modal.x + 2 + field_col_w + 3;
            let pw = modal.right().saturating_sub(px + 2);
            let py = modal.y + header_rows;
            let ph = btn_y.saturating_sub(py + 2);
            (pw >= 28 && ph >= 8).then_some(Rect::new(px, py, pw, ph))
        } else if is_terminal {
            let py = y.min(preview_limit);
            let ph = cmd_y.saturating_sub(py);
            (ph >= 3 && inner_w >= 20).then_some(Rect::new(modal.x + 2, py, inner_w, ph))
        } else {
            None
        };

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
        let buttons = [
            Rect::new(b0_x, btn_y, b0_w, 1),
            Rect::new(b1_x, btn_y, b1_w, 1),
        ];

        Self {
            modal,
            fields,
            chips,
            buttons,
            terminal_preview,
        }
    }

    pub fn field_at(&self, point: Position) -> Option<usize> {
        self.fields.iter().position(|r| r.contains(point))
    }

    pub fn chip_at(&self, point: Position) -> Option<(usize, usize)> {
        self.chips
            .iter()
            .find(|(_, _, r)| r.contains(point))
            .map(|(fi, ci, _)| (*fi, *ci))
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
}

impl PickerGeometry {
    pub fn compute(area: Rect, total_matches: usize, cursor: usize) -> Self {
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 68.min(area.width.saturating_sub(margin_x));
        let modal_h = (total_matches as u16 + 7)
            .clamp(11, 20)
            .min(area.height.saturating_sub(2));
        let modal = centered(area, modal_w, modal_h);
        let list_y = modal.y + 4;
        let list_h = modal.bottom().saturating_sub(list_y + 2);
        let list = Rect::new(modal.x + 2, list_y, modal.width.saturating_sub(4), list_h);
        let scroll = list_offset(cursor, total_matches, list_h as usize);
        Self {
            modal,
            list,
            scroll,
        }
    }

    pub fn item_at(&self, point: Position, count: usize) -> Option<usize> {
        if !self.list.contains(point) {
            return None;
        }
        let idx = (point.y - self.list.y) as usize + self.scroll;
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
    pub fn compute(area: Rect, confirm: &Confirmation, i18n: &I18n) -> Self {
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let modal_w = 72.min(area.width.saturating_sub(margin_x));
        let text_w = modal_w.saturating_sub(4).max(1) as usize;
        let wrapped = super::render::wrap_words(&confirm.text, text_w);
        let total_lines = wrapped.len();
        let footer_rows = if confirm.token == "YES" { 5 } else { 6 };
        let modal_h =
            (total_lines as u16 + footer_rows + 3).clamp(10, area.height.saturating_sub(2));
        let modal = centered(area, modal_w, modal_h);
        let body = Rect::new(
            modal.x + 2,
            modal.y + 2,
            modal.width.saturating_sub(4),
            modal.height.saturating_sub(footer_rows + 2),
        );
        let btn_y = modal.bottom().saturating_sub(3);
        let max_btn_right = modal.right().saturating_sub(2);
        let b0_x = modal.x + 2;
        let b0_w =
            (i18n.text("ui.cancel").width() as u16 + 4).min(max_btn_right.saturating_sub(b0_x));
        let b1_x = (b0_x + b0_w + 2).min(max_btn_right);
        let b1_w = (i18n.text("ui.confirm_execute").width() as u16 + 4)
            .min(max_btn_right.saturating_sub(b1_x));
        let buttons = [
            Rect::new(b0_x, btn_y, b0_w, 1),
            Rect::new(b1_x, btn_y, b1_w, 1),
        ];
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
