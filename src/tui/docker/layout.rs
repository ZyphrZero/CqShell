//! Shared responsive geometry (`Wide`, `Medium`, `Compact`, `Focus`) and hit-testing rectangles
//! for the dedicated Docker management workspace and its local modals.

use super::state::{ComposeSubTab, ContainerSubTab, DockerModal, DockerState, ImageSubTab};
use crate::{
    i18n::I18n,
    ops::docker::model::{DockerAvailability, DockerTab},
    tui::{layout::list_offset, state::App},
};
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub(in crate::tui) struct DockerTabPill {
    pub tab: DockerTab,
    pub label: String,
    pub count: String,
    pub rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum QuickBtnKind {
    StartStop,
    Restart,
    Exec,
    Logs,
    Pause,
    Delete,
    Pull,
    Mirror,
    CreateNetwork,
    Prune,
    JumpProject,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct QuickBtn {
    pub key: &'static str,
    pub label: String,
    pub rect: Rect,
    pub kind: QuickBtnKind,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct DockerGeometry {
    pub area: Rect,
    pub tab_bar: Rect,
    pub tab_pills: Vec<DockerTabPill>,
    pub tab_overflow_left: bool,
    pub tab_overflow_right: bool,
    pub focus_btn: Option<Rect>,
    pub tab_rule_y: u16,
    pub list_area: Rect,
    pub filter_bar: Rect,
    pub scope_chip: Option<Rect>,
    pub sort_chip: Option<Rect>,
    pub project_chip: Option<Rect>,
    pub table_header: Rect,
    pub col_hits: Vec<(Rect, usize)>,
    pub list_inner: Rect,
    pub list_scroll: usize,
    pub batch_bar: Option<Rect>,
    pub inspector_area: Rect,
    pub sub_pills: Vec<(Rect, usize)>,
    pub inspector_body: Rect,
    pub quick_btns: Vec<QuickBtn>,
    pub fullscreen_view: Option<Rect>,
    pub empty_cta: Option<Rect>,
}

impl DockerGeometry {
    pub fn compute(area: Rect, app: &App, drawer_open: bool) -> Self {
        let dk = &app.docker;
        let i = &app.ctx.i18n;
        let theme = &app.theme.active;
        let framed = theme.flags.framed;

        let inner_x = area.x + 2;
        let inner_w = area.width.saturating_sub(4);
        let tab_y = area.y + crate::tui::render::tokens::card_top_inset(theme, area.height);

        // Top-right Focus toggle button when space allows
        let focus_text = if dk.fullscreen {
            format!(" [z] {} ", i.text("docker.key.unfocus"))
        } else {
            format!(" [z] {} ", i.text("docker.key.focus"))
        };
        let focus_w = focus_text.width() as u16;
        let focus_btn = if inner_w >= focus_w + 48 && !drawer_open {
            Some(Rect::new(
                (inner_x + inner_w).saturating_sub(focus_w),
                tab_y,
                focus_w,
                1,
            ))
        } else {
            None
        };

        let pills_w = focus_btn
            .map(|r| r.x.saturating_sub(inner_x + 1))
            .unwrap_or(inner_w);
        let tab_bar = Rect::new(inner_x, tab_y, pills_w, 1);
        let (tab_pills, tab_overflow_left, tab_overflow_right) = build_tab_pills(tab_bar, dk, i);
        let tab_rule_y = (tab_y + 1).min(area.bottom().saturating_sub(1));

        let body_y = (tab_rule_y + 1).min(area.bottom());
        let body_h = area.bottom().saturating_sub(body_y + u16::from(framed));
        let body = Rect::new(area.x + 1, body_y, area.width.saturating_sub(2), body_h);

        // Fullscreen log / inspect overlay inside the Docker workspace
        if dk.logs.fullscreen || dk.inspector_fullscreen {
            return Self {
                area,
                tab_bar,
                tab_pills,
                tab_overflow_left,
                tab_overflow_right,
                focus_btn,
                tab_rule_y,
                list_area: Rect::default(),
                filter_bar: Rect::default(),
                scope_chip: None,
                sort_chip: None,
                project_chip: None,
                table_header: Rect::default(),
                col_hits: Vec::new(),
                list_inner: Rect::default(),
                list_scroll: 0,
                batch_bar: None,
                inspector_area: Rect::default(),
                sub_pills: Vec::new(),
                inspector_body: body,
                quick_btns: Vec::new(),
                fullscreen_view: Some(body),
                empty_cta: None,
            };
        }

        // Empty / unavailable guidance CTA button — exact same card formula as `draw_unavailable_guidance`
        let empty_cta = if dk.tab != DockerTab::Daemon
            && matches!(
                dk.snapshot.availability,
                DockerAvailability::NotInstalled | DockerAvailability::DaemonStopped(_)
            )
            && body.height >= 6
            && body.width >= 20
        {
            let card_w = body
                .width
                .saturating_sub(if body.width < 48 { 2 } else { 6 })
                .min(68)
                .max(20.min(body.width));
            let card_h = 9u16
                .min(body.height.saturating_sub(2))
                .max(6.min(body.height));
            let card_x = body.x + (body.width.saturating_sub(card_w)) / 2;
            let card_y = body.y + (body.height.saturating_sub(card_h)) / 2;
            let card = Rect::new(card_x, card_y, card_w, card_h);
            let inner_x = card.x + 2;
            let inner_w = card.width.saturating_sub(4).min(28);
            Some(Rect::new(
                inner_x,
                card.bottom().saturating_sub(2).max(card.y + 2),
                inner_w,
                1,
            ))
        } else {
            None
        };

        let side_by_side = !drawer_open && body.width >= 76;
        let (list_area, inspector_area, stacked) = if side_by_side {
            let list_pct = if body.width >= 108 { 57 } else { 54 };
            let list_w = ((u32::from(body.width) * list_pct) / 100) as u16;
            let list_w = list_w.clamp(38, body.width.saturating_sub(32));
            let left = Rect::new(body.x, body.y, list_w, body.height);
            let right = Rect::new(
                body.x + list_w + 1,
                body.y,
                body.width.saturating_sub(list_w + 1),
                body.height,
            );
            (left, right, false)
        } else if drawer_open {
            (body, Rect::default(), false)
        } else {
            let insp_h = if body.height >= 18 {
                8
            } else if body.height >= 13 {
                6
            } else if body.height >= 10 {
                5
            } else {
                4.min(body.height / 2)
            };
            let list_h = body.height.saturating_sub(insp_h);
            let top = Rect::new(body.x, body.y, body.width, list_h);
            let bot = Rect::new(body.x, top.bottom(), body.width, insp_h);
            (top, bot, true)
        };

        // List sub-regions: filter bar + table header + rows + optional batch footer
        let list_x = list_area.x + 1;
        let list_w = list_area.width.saturating_sub(2);
        let pad_top = u16::from(framed);
        let pad_bot = u16::from(framed);
        let filter_y = list_area.y + pad_top;
        let has_filter_bar = dk.tab != DockerTab::Daemon
            && (list_area.height >= 6 || dk.filtering || !dk.filter.text.is_empty());
        let filter_bar = if has_filter_bar {
            Rect::new(list_x, filter_y, list_w, 1)
        } else {
            Rect::default()
        };

        let (scope_chip, sort_chip, project_chip) = if has_filter_bar {
            build_filter_chips(filter_bar, dk, i)
        } else {
            (None, None, None)
        };

        let header_y = if has_filter_bar {
            filter_y + 1
        } else {
            list_area.y + pad_top
        };
        let has_header = dk.tab != DockerTab::Daemon && list_area.height >= 5;
        let table_header = if has_header {
            Rect::new(list_x, header_y, list_w, 1)
        } else {
            Rect::default()
        };
        let col_hits = if has_header {
            build_col_hits(table_header, dk.tab)
        } else {
            Vec::new()
        };

        let show_batch = dk.selected_count() > 0 && list_area.height >= 8;
        let batch_h = u16::from(show_batch);
        let rows_y = if has_header {
            header_y + 1
        } else {
            list_area.y + pad_top
        };
        let rows_bottom = list_area.bottom().saturating_sub(pad_bot + batch_h);
        let list_inner = Rect::new(
            list_x,
            rows_y.min(rows_bottom),
            list_w,
            rows_bottom.saturating_sub(rows_y),
        );
        let batch_bar = show_batch.then(|| Rect::new(list_x, rows_bottom, list_w, 1));

        let visible_total = dk.visible_count();
        let list_scroll = list_offset(dk.cursor(), visible_total, list_inner.height as usize);

        // Inspector sub-regions: sub-tab bar + scrollable body + bottom quick action buttons
        let (sub_pills, inspector_body, quick_btns) =
            if inspector_area.width >= 16 && inspector_area.height >= 4 {
                let ix = inspector_area.x + 1;
                let iw = inspector_area.width.saturating_sub(2);
                let iy = inspector_area.y + u16::from(framed);
                let ibot = inspector_area.bottom().saturating_sub(u16::from(framed));

                let sub_pills = build_inspector_sub_pills(Rect::new(ix, iy, iw, 1), dk, i);
                let body_start = if sub_pills.is_empty() { iy } else { iy + 2 };

                let quick_h = if !stacked && ibot.saturating_sub(body_start) >= 6 {
                    2u16
                } else if stacked && ibot.saturating_sub(body_start) >= 3 {
                    1u16
                } else {
                    0u16
                };
                let quick_y = ibot.saturating_sub(quick_h);
                let quick_btns = if quick_h > 0 {
                    build_quick_buttons(Rect::new(ix, quick_y, iw, quick_h), dk, i)
                } else {
                    Vec::new()
                };
                let body_rect = Rect::new(
                    ix,
                    body_start.min(quick_y),
                    iw,
                    quick_y.saturating_sub(body_start),
                );
                (sub_pills, body_rect, quick_btns)
            } else {
                (Vec::new(), Rect::default(), Vec::new())
            };

        Self {
            area,
            tab_bar,
            tab_pills,
            tab_overflow_left,
            tab_overflow_right,
            focus_btn,
            tab_rule_y,
            list_area,
            filter_bar,
            scope_chip,
            sort_chip,
            project_chip,
            table_header,
            col_hits,
            list_inner,
            list_scroll,
            batch_bar,
            inspector_area,
            sub_pills,
            inspector_body,
            quick_btns,
            fullscreen_view: None,
            empty_cta,
        }
    }

    pub fn tab_at(&self, point: Position) -> Option<DockerTab> {
        self.tab_pills
            .iter()
            .find(|p| p.rect.contains(point))
            .map(|p| p.tab)
    }

    pub fn row_at(&self, point: Position, total: usize) -> Option<(usize, bool)> {
        if !self.list_inner.contains(point) {
            return None;
        }
        let idx = (point.y - self.list_inner.y) as usize + self.list_scroll;
        if idx >= total {
            return None;
        }
        let in_checkbox = point.x <= self.list_inner.x + 4;
        Some((idx, in_checkbox))
    }

    pub fn col_at(&self, point: Position) -> Option<usize> {
        self.col_hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, idx)| *idx)
    }

    pub fn sub_pill_at(&self, point: Position) -> Option<usize> {
        self.sub_pills
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, idx)| *idx)
    }

    pub fn quick_btn_at(&self, point: Position) -> Option<QuickBtnKind> {
        self.quick_btns
            .iter()
            .find(|b| b.rect.contains(point))
            .map(|b| b.kind)
    }
}

fn build_tab_pills(bar: Rect, dk: &DockerState, i: &I18n) -> (Vec<DockerTabPill>, bool, bool) {
    if bar.width < 12 || bar.height == 0 {
        return (Vec::new(), false, false);
    }
    let ready = dk.snapshot.availability.is_ready();
    let show_counts = bar.width >= 48 && ready;
    let items: Vec<(DockerTab, String, String, u16)> = DockerTab::ALL
        .iter()
        .map(|&tab| {
            let label = if dk.fullscreen {
                format!("{} {}", tab.index() + 1, i.text(tab.label_key()))
            } else {
                i.text(tab.label_key()).to_owned()
            };
            let count = if !show_counts {
                String::new()
            } else {
                match tab {
                    DockerTab::Containers => format!(
                        "{}/{}",
                        dk.snapshot.running_containers(),
                        dk.snapshot.containers.len()
                    ),
                    DockerTab::Compose => dk.snapshot.compose.len().to_string(),
                    DockerTab::Images => dk.snapshot.images.len().to_string(),
                    DockerTab::Volumes => dk.snapshot.volumes.len().to_string(),
                    DockerTab::Networks => dk.snapshot.networks.len().to_string(),
                    DockerTab::Daemon => String::new(),
                }
            };
            let w = (label.width()
                + if count.is_empty() {
                    0
                } else {
                    count.width() + 1
                }
                + 2)
            .min(bar.width as usize) as u16;
            (tab, label, count, w)
        })
        .collect();

    let total_w: u16 = items
        .iter()
        .enumerate()
        .map(|(idx, (_, _, _, w))| *w + u16::from(idx > 0))
        .sum();

    if total_w <= bar.width {
        let mut x = bar.x;
        let pills = items
            .into_iter()
            .map(|(tab, label, count, w)| {
                let rect = Rect::new(x, bar.y, w.min(bar.right().saturating_sub(x)), 1);
                x = x.saturating_add(w + 1);
                DockerTabPill {
                    tab,
                    label,
                    count,
                    rect,
                }
            })
            .collect();
        return (pills, false, false);
    }

    // Window around the active tab for narrow terminals (e.g. 80x24)
    let active = dk.tab.index();
    let inner_w = bar.width.saturating_sub(4).max(1);
    let mut lo = active;
    let mut hi = active;
    let mut used = items[active].3.min(inner_w);
    loop {
        let mut grew = false;
        if hi + 1 < items.len() && used + items[hi + 1].3 < inner_w {
            hi += 1;
            used += items[hi].3 + 1;
            grew = true;
        }
        if lo > 0 && used + items[lo - 1].3 < inner_w {
            lo -= 1;
            used += items[lo].3 + 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let over_l = lo > 0;
    let over_r = hi + 1 < items.len();
    let mut x = if over_l { bar.x + 2 } else { bar.x };
    let max_x = if over_r {
        bar.right().saturating_sub(2)
    } else {
        bar.right()
    };
    let mut pills = Vec::new();
    for (tab, label, count, w) in &items[lo..=hi] {
        let avail = max_x.saturating_sub(x);
        if avail == 0 {
            break;
        }
        let pw = (*w).min(avail);
        pills.push(DockerTabPill {
            tab: *tab,
            label: label.clone(),
            count: count.clone(),
            rect: Rect::new(x, bar.y, pw, 1),
        });
        x = x.saturating_add(pw + 1);
    }
    (pills, over_l, over_r)
}

fn build_filter_chips(
    bar: Rect,
    dk: &DockerState,
    i: &I18n,
) -> (Option<Rect>, Option<Rect>, Option<Rect>) {
    if bar.width < 20 || bar.height == 0 {
        return (None, None, None);
    }
    let mut right_edge = bar.right();
    let sort_chip = dk.sort_label_key().and_then(|key| {
        let txt = format!(" [o] {}^ ", i.text(key));
        let w = txt.width() as u16;
        if right_edge > bar.x + w + 8 {
            right_edge = right_edge.saturating_sub(w);
            let r = Rect::new(right_edge, bar.y, w, 1);
            right_edge = right_edge.saturating_sub(1);
            Some(r)
        } else {
            None
        }
    });
    let scope_chip = match dk.tab {
        DockerTab::Containers => {
            let lbl = if dk.only_running {
                i.text("docker.filter.running")
            } else {
                i.text("docker.filter.all")
            };
            let txt = format!(" [a] {lbl} ");
            let w = txt.width() as u16;
            if right_edge > bar.x + w + 8 {
                right_edge = right_edge.saturating_sub(w);
                let r = Rect::new(right_edge, bar.y, w, 1);
                right_edge = right_edge.saturating_sub(1);
                Some(r)
            } else {
                None
            }
        }
        DockerTab::Images => {
            let lbl = if dk.show_dangling {
                i.text("docker.filter.dangling_on")
            } else {
                i.text("docker.filter.dangling_off")
            };
            let txt = format!(" [a] {lbl} ");
            let w = txt.width() as u16;
            if right_edge > bar.x + w + 8 {
                right_edge = right_edge.saturating_sub(w);
                let r = Rect::new(right_edge, bar.y, w, 1);
                right_edge = right_edge.saturating_sub(1);
                Some(r)
            } else {
                None
            }
        }
        _ => None,
    };
    let project_chip = if dk.tab == DockerTab::Containers
        && let Some(proj) = &dk.project_filter
    {
        let txt = format!(
            " {} x ",
            i.format("docker.filter.project_badge", &[("name", proj)])
        );
        let w = (txt.width() as u16).min(right_edge.saturating_sub(bar.x + 8));
        if w >= 6 {
            right_edge = right_edge.saturating_sub(w);
            Some(Rect::new(right_edge, bar.y, w, 1))
        } else {
            None
        }
    } else {
        None
    };
    (scope_chip, sort_chip, project_chip)
}

fn build_col_hits(header: Rect, tab: DockerTab) -> Vec<(Rect, usize)> {
    if header.width < 20 || header.height == 0 {
        return Vec::new();
    }
    match tab {
        DockerTab::Containers => {
            let mut hits = vec![
                (Rect::new(header.x, header.y, 7, 1), 0), // State
                (
                    Rect::new(
                        header.x + 7,
                        header.y,
                        header.width.saturating_sub(21).max(8),
                        1,
                    ),
                    1,
                ), // Name
            ];
            if header.width >= 38 {
                let cpu_x = header.right().saturating_sub(13);
                hits.push((Rect::new(cpu_x, header.y, 6, 1), 2));
                hits.push((Rect::new(cpu_x + 6, header.y, 7, 1), 3));
            }
            hits
        }
        DockerTab::Images | DockerTab::Volumes | DockerTab::Networks => {
            let split = header.width.saturating_sub(16).max(10);
            vec![
                (Rect::new(header.x, header.y, split, 1), 0),
                (
                    Rect::new(
                        header.x + split,
                        header.y,
                        header.width.saturating_sub(split),
                        1,
                    ),
                    1,
                ),
            ]
        }
        _ => Vec::new(),
    }
}

fn build_inspector_sub_pills(bar: Rect, dk: &DockerState, i: &I18n) -> Vec<(Rect, usize)> {
    let keys: &[&str] = match dk.tab {
        DockerTab::Containers => &[
            ContainerSubTab::Overview.label_key(),
            ContainerSubTab::Logs.label_key(),
            ContainerSubTab::Stats.label_key(),
            ContainerSubTab::Env.label_key(),
        ],
        DockerTab::Compose => &[
            ComposeSubTab::Services.label_key(),
            ComposeSubTab::Logs.label_key(),
            ComposeSubTab::Yaml.label_key(),
        ],
        DockerTab::Images => &[
            ImageSubTab::Overview.label_key(),
            ImageSubTab::History.label_key(),
        ],
        _ => return Vec::new(),
    };
    let mut x = bar.x;
    let mut out = Vec::new();
    for (idx, key) in keys.iter().enumerate() {
        let lbl = i.text(key);
        let w = (lbl.width() + 2) as u16;
        if x + w > bar.right() {
            break;
        }
        out.push((Rect::new(x, bar.y, w, 1), idx));
        x = x.saturating_add(w + 1);
    }
    out
}

fn build_quick_buttons(area: Rect, dk: &DockerState, i: &I18n) -> Vec<QuickBtn> {
    if area.width < 16 || area.height == 0 || !dk.snapshot.availability.is_ready() {
        return Vec::new();
    }
    let defs: Vec<(&'static str, &str, QuickBtnKind)> = match dk.tab {
        DockerTab::Containers => {
            let Some(c) = dk.selected_container() else {
                return Vec::new();
            };
            let s_lbl = if c.state.is_running() {
                i.text("docker.op.stop")
            } else {
                i.text("docker.op.start")
            };
            vec![
                ("s", s_lbl, QuickBtnKind::StartStop),
                ("u", i.text("docker.key.restart"), QuickBtnKind::Restart),
                ("e", i.text("docker.key.exec"), QuickBtnKind::Exec),
                ("l", i.text("docker.key.logs"), QuickBtnKind::Logs),
                ("p", i.text("docker.op.pause"), QuickBtnKind::Pause),
                ("d", i.text("docker.key.delete"), QuickBtnKind::Delete),
            ]
        }
        DockerTab::Compose => {
            if dk.selected_compose().is_none() {
                return Vec::new();
            }
            vec![
                ("u", "Up", QuickBtnKind::Restart),
                (
                    "s",
                    i.text("docker.key.start_stop"),
                    QuickBtnKind::StartStop,
                ),
                ("l", i.text("docker.key.logs"), QuickBtnKind::Logs),
                (
                    "j",
                    i.text("docker.tab.containers"),
                    QuickBtnKind::JumpProject,
                ),
                ("d", "Down", QuickBtnKind::Delete),
            ]
        }
        DockerTab::Images => vec![
            ("p", i.text("docker.key.pull"), QuickBtnKind::Pull),
            ("g", i.text("docker.key.mirror"), QuickBtnKind::Mirror),
            ("d", i.text("docker.key.delete"), QuickBtnKind::Delete),
            ("P", i.text("docker.key.prune"), QuickBtnKind::Prune),
        ],
        DockerTab::Volumes => vec![
            ("d", i.text("docker.key.delete"), QuickBtnKind::Delete),
            ("P", i.text("docker.key.prune"), QuickBtnKind::Prune),
        ],
        DockerTab::Networks => vec![
            (
                "c",
                i.text("docker.key.new_net"),
                QuickBtnKind::CreateNetwork,
            ),
            ("d", i.text("docker.key.delete"), QuickBtnKind::Delete),
            ("P", i.text("docker.key.prune"), QuickBtnKind::Prune),
        ],
        DockerTab::Daemon => vec![
            ("g", i.text("docker.key.mirror"), QuickBtnKind::Mirror),
            ("P", i.text("docker.key.prune"), QuickBtnKind::Prune),
        ],
    };

    let mut out = Vec::new();
    let mut x = area.x;
    let mut y = area.y;
    for (key, lbl, kind) in defs {
        let short = lbl.split(' ').next().unwrap_or(lbl);
        let text = format!(" [{key}] {short} ");
        let w = (text.width() as u16).min(area.width);
        if x > area.x && x + w > area.right() {
            y += 1;
            x = area.x;
        }
        if y >= area.bottom() {
            break;
        }
        out.push(QuickBtn {
            key,
            label: short.to_owned(),
            rect: Rect::new(x, y, w, 1),
            kind,
        });
        x = x.saturating_add(w + 1);
    }
    out
}

/// Centered modal geometry for Docker-specific dialogs.
#[derive(Debug, Clone)]
pub(in crate::tui) struct DockerModalGeometry {
    pub modal: Rect,
    pub rows: Vec<Rect>,
    pub submit_btn: Option<Rect>,
    pub cancel_btn: Option<Rect>,
}

impl DockerModalGeometry {
    pub fn compute(area: Rect, modal: &DockerModal) -> Self {
        let margin_x = if area.width < 60 { 2 } else { 4 };
        let max_w = area.width.saturating_sub(margin_x).max(20.min(area.width));
        let max_h = area.height.saturating_sub(2).max(7.min(area.height));
        let clamp_w = |ideal: u16, min_pref: u16| ideal.min(max_w).max(min_pref.min(max_w));
        let clamp_h = |ideal: u16| ideal.min(max_h).max(7.min(max_h));

        let (w, h, row_count, has_buttons) = match modal {
            DockerModal::ActionMenu { items, .. } => {
                let w = clamp_w(48, 32);
                let h = clamp_h((items.len() as u16) + 5);
                (w, h, items.len(), false)
            }
            DockerModal::PullImage { .. } => (clamp_w(62, 36), clamp_h(11), 1, true),
            DockerModal::CreateNetwork { .. } => (clamp_w(64, 36), clamp_h(15), 3, true),
            DockerModal::ExecForm { .. } => (clamp_w(64, 36), clamp_h(15), 3, true),
            DockerModal::PruneWizard { .. } => (clamp_w(66, 36), clamp_h(16), 6, true),
        };

        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);

        let inner_x = rect.x + 2;
        let inner_w = rect.width.saturating_sub(4);
        let start_y = match modal {
            DockerModal::ActionMenu { .. } => rect.y + 2,
            DockerModal::PruneWizard { .. } => rect.y + if rect.height >= 14 { 4 } else { 3 },
            _ => rect.y + 3,
        };
        let step_y = match modal {
            DockerModal::ActionMenu { .. } | DockerModal::PruneWizard { .. } => 1u16,
            _ => {
                if rect.height >= 14 {
                    3u16
                } else {
                    2u16
                }
            }
        };
        let mut rows = Vec::with_capacity(row_count);
        for idx in 0..row_count {
            let ry = start_y + (idx as u16) * step_y;
            if ry < rect.bottom().saturating_sub(2) {
                rows.push(Rect::new(inner_x, ry, inner_w, 1));
            }
        }

        let (submit_btn, cancel_btn) = if has_buttons && rect.height >= 6 {
            let btn_y = rect.bottom().saturating_sub(2);
            let bw = 14u16.min(inner_w.saturating_sub(2) / 2).max(6.min(inner_w));
            let cancel_x = inner_x + bw + 2;
            let cancel_w = bw
                .min(12)
                .min(inner_x + inner_w - cancel_x.min(inner_x + inner_w));
            (
                Some(Rect::new(inner_x, btn_y, bw, 1)),
                (cancel_w >= 4).then_some(Rect::new(cancel_x, btn_y, cancel_w, 1)),
            )
        } else {
            (None, None)
        };

        Self {
            modal: rect,
            rows,
            submit_btn,
            cancel_btn,
        }
    }
}
