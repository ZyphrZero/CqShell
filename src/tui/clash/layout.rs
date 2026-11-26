//! Shared responsive geometry (`Wide`, `Medium`, `Compact`, `Mobile`, `Focus`) and hit-testing
//! rectangles for the dedicated Mihomo visual management workspace and its local modals.

use super::state::{ClashModal, ClashState};
use crate::{
    i18n::I18n,
    ops::clash::model::{ClashAvailability, ClashTab, IpQualityLevel, IpQualityOutcome},
    theme::Role,
    tui::{layout::list_offset, state::App},
};
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub(in crate::tui) struct ClashTabPill {
    pub tab: ClashTab,
    pub label: String,
    pub count: String,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct GroupPillHit {
    pub group_idx: usize,
    pub label: String,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct OverviewGeom {
    pub status_card: Rect,
    pub outbound_badge_hit: Option<Rect>,
    pub outbound_ip_hit: Option<Rect>,
    pub outbound_detail_btn: Option<Rect>,
    pub start_stop_btn: Option<Rect>,
    pub reload_btn: Option<Rect>,
    pub traffic_card: Rect,
    pub traffic_plot: Rect,
    pub traffic_side: Rect,
    pub traffic_mode_chip: Option<Rect>,
    pub traffic_window_chip: Option<Rect>,
    pub traffic_scale_chip: Option<Rect>,
    pub traffic_pause_chip: Option<Rect>,
    pub traffic_expand_btn: Option<Rect>,
    pub mode_bar: Rect,
    pub mode_pills: Vec<(Rect, &'static str)>,
    pub profile_card: Rect,
    pub profile_switch_btn: Option<Rect>,
    pub profile_update_btn: Option<Rect>,
    pub services_card: Rect,
    pub exit_probe_btn: Option<Rect>,
    pub socks_secret_btn: Option<Rect>,
    pub webui_btn: Option<Rect>,
    pub connections_card: Rect,
    pub groups_card: Rect,
    pub group_card_hits: Vec<(Rect, usize)>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct TrafficFullscreenGeom {
    pub area: Rect,
    pub toolbar: Rect,
    pub mode_chip: Option<Rect>,
    pub window_chip: Option<Rect>,
    pub scale_chip: Option<Rect>,
    pub pause_chip: Option<Rect>,
    pub breakdown_chip: Option<Rect>,
    pub close_btn: Option<Rect>,
    pub waveform_card: Rect,
    pub panel_one: Rect,
    pub panel_two: Rect,
    pub panel_three: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ProxiesGeom {
    pub group_bar: Rect,
    pub group_pills: Vec<GroupPillHit>,
    pub group_overflow_left: bool,
    pub group_overflow_right: bool,
    pub group_picker_btn: Option<Rect>,
    pub toolbar: Rect,
    pub filter_bar: Rect,
    pub test_btn: Option<Rect>,
    pub sort_chip: Option<Rect>,
    pub hide_chip: Option<Rect>,
    pub providers_btn: Option<Rect>,
    pub chain_bar: Rect,
    pub grid_area: Rect,
    pub node_hits: Vec<(Rect, usize)>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ProfilesGeom {
    pub toolbar: Rect,
    pub filter_bar: Rect,
    pub update_all_btn: Option<Rect>,
    pub add_btn: Option<Rect>,
    pub import_btn: Option<Rect>,
    pub providers_btn: Option<Rect>,
    pub list_area: Rect,
    pub list_inner: Rect,
    pub item_hits: Vec<(Rect, usize)>,
    pub inspector_area: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ConnectionsGeom {
    pub toolbar: Rect,
    pub filter_bar: Rect,
    pub scope_chip: Option<Rect>,
    pub sort_chip: Option<Rect>,
    pub close_all_btn: Option<Rect>,
    pub facet_bar: Rect,
    pub facet_chips: Vec<(Rect, Option<String>)>,
    pub list_area: Rect,
    pub table_header: Rect,
    pub list_inner: Rect,
    pub list_scroll: usize,
    pub inspector_area: Rect,
    pub inspector_body: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct RulesGeom {
    pub toolbar: Rect,
    pub filter_bar: Rect,
    pub providers_btn: Option<Rect>,
    pub facet_bar: Rect,
    pub facet_chips: Vec<(Rect, Option<String>)>,
    pub list_area: Rect,
    pub table_header: Rect,
    pub list_inner: Rect,
    pub list_scroll: usize,
    pub providers_area: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ToolsGeom {
    pub toolbar: Rect,
    pub filter_bar: Rect,
    pub logs_btn: Option<Rect>,
    pub list_area: Rect,
    pub list_inner: Rect,
    pub list_scroll: usize,
    pub inspector_area: Rect,
    pub inspector_body: Rect,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct UnavailableGeom {
    pub body: Rect,
    pub card: Rect,
    pub cta: Option<Rect>,
    pub secondary_cta: Option<Rect>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) enum ClashBodyGeom {
    Overview(OverviewGeom),
    Proxies(ProxiesGeom),
    Profiles(ProfilesGeom),
    Connections(ConnectionsGeom),
    Rules(RulesGeom),
    Tools(ToolsGeom),
    TrafficFullscreen(TrafficFullscreenGeom),
    Fullscreen(Rect),
    Unavailable(UnavailableGeom),
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct ClashGeometry {
    pub area: Rect,
    pub tab_bar: Rect,
    pub tab_pills: Vec<ClashTabPill>,
    pub tab_overflow_left: bool,
    pub tab_overflow_right: bool,
    pub focus_btn: Option<Rect>,
    pub tab_rule_y: u16,
    pub body: ClashBodyGeom,
}

impl ClashGeometry {
    pub fn compute(area: Rect, app: &App, drawer_open: bool) -> Self {
        let cl = &app.clash;
        let i = &app.ctx.i18n;
        let theme = &app.theme.active;
        let framed = theme.flags.framed;

        let inner_x = area.x + 2;
        let inner_w = area.width.saturating_sub(4);
        let tab_y = area.y + crate::tui::render::tokens::card_top_inset(theme, area.height);

        let focus_text = if cl.fullscreen {
            format!(" [z] {} ", i.text("docker.key.unfocus"))
        } else {
            format!(" [z] {} ", i.text("docker.key.focus"))
        };
        let focus_w = focus_text.width() as u16;
        let focus_btn = if inner_w >= focus_w + 46 && !drawer_open {
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
        let (tab_pills, tab_overflow_left, tab_overflow_right) = build_tab_pills(tab_bar, cl, i);
        let tab_rule_y = (tab_y + 1).min(area.bottom().saturating_sub(1));

        let body_y = (tab_rule_y + 1).min(area.bottom());
        let body_h = area.bottom().saturating_sub(body_y + u16::from(framed));
        let body_rect = Rect::new(area.x + 1, body_y, area.width.saturating_sub(2), body_h);

        // Fullscreen traffic atlas, redacted log viewer, or connection inspector inside workspace
        if cl.traffic_fullscreen && (cl.snapshot.availability.is_ready() || cl.is_traffic_paused())
        {
            return Self {
                area,
                tab_bar,
                tab_pills,
                tab_overflow_left,
                tab_overflow_right,
                focus_btn,
                tab_rule_y,
                body: ClashBodyGeom::TrafficFullscreen(build_traffic_fullscreen(body_rect, cl, i)),
            };
        }
        if cl.tools_log_fullscreen || cl.conn_inspector_fullscreen {
            return Self {
                area,
                tab_bar,
                tab_pills,
                tab_overflow_left,
                tab_overflow_right,
                focus_btn,
                tab_rule_y,
                body: ClashBodyGeom::Fullscreen(body_rect),
            };
        }

        // Offline / stopped guidance screen on controller-dependent tabs (Profiles & Tools stay usable!)
        let needs_controller = matches!(
            cl.tab,
            ClashTab::Overview | ClashTab::Proxies | ClashTab::Connections | ClashTab::Rules
        );
        if needs_controller && !cl.snapshot.availability.is_ready() {
            let unavail = build_unavailable(body_rect, cl, i);
            return Self {
                area,
                tab_bar,
                tab_pills,
                tab_overflow_left,
                tab_overflow_right,
                focus_btn,
                tab_rule_y,
                body: ClashBodyGeom::Unavailable(unavail),
            };
        }

        let body = match cl.tab {
            ClashTab::Overview => ClashBodyGeom::Overview(build_overview(body_rect, cl, i)),
            ClashTab::Proxies => ClashBodyGeom::Proxies(build_proxies(body_rect, cl, i)),
            ClashTab::Profiles => ClashBodyGeom::Profiles(build_profiles(body_rect, cl, i, framed)),
            ClashTab::Connections => {
                ClashBodyGeom::Connections(build_connections(body_rect, cl, i, framed))
            }
            ClashTab::Rules => ClashBodyGeom::Rules(build_rules(body_rect, cl, i, framed)),
            ClashTab::Tools => ClashBodyGeom::Tools(build_tools(body_rect, cl, i, framed)),
        };

        Self {
            area,
            tab_bar,
            tab_pills,
            tab_overflow_left,
            tab_overflow_right,
            focus_btn,
            tab_rule_y,
            body,
        }
    }
}

fn build_unavailable(body: Rect, cl: &ClashState, i: &I18n) -> UnavailableGeom {
    let is_no_ctrl = matches!(
        cl.snapshot.availability,
        ClashAvailability::ControllerUnconfigured
    );
    let enabling_ctrl = is_no_ctrl && cl.busy_hint.as_deref() == Some("controller");
    let card_w = body
        .width
        .saturating_sub(if body.width < 48 { 2 } else { 6 })
        .min(if is_no_ctrl { 72 } else { 68 })
        .max(20.min(body.width));
    let target_h = if is_no_ctrl { 10u16 } else { 9u16 };
    let card_h = target_h
        .min(body.height.saturating_sub(2))
        .max(6.min(body.height));
    let card_x = body.x + (body.width.saturating_sub(card_w)) / 2;
    let card_y = body.y + (body.height.saturating_sub(card_h)) / 2;
    let card = Rect::new(card_x, card_y, card_w, card_h);

    let (cta, secondary_cta) = if matches!(
        cl.snapshot.availability,
        ClashAvailability::NotInstalled
            | ClashAvailability::Stopped
            | ClashAvailability::ControllerUnconfigured
    ) && card_h >= 6
        && card_w >= 20
    {
        let inner_w = card_w.saturating_sub(4);
        let btn_y = card_y + card_h.saturating_sub(2);
        let primary_label = match cl.snapshot.availability {
            ClashAvailability::NotInstalled => {
                format!(" [Enter] {} ", i.text("clash.avail.cta_install"))
            }
            ClashAvailability::Stopped => {
                format!(" [Enter] {} ", i.text("clash.avail.cta_start"))
            }
            ClashAvailability::ControllerUnconfigured => {
                if enabling_ctrl {
                    format!(" * {} ", i.text("clash.avail.enabling_ctrl"))
                } else {
                    format!(" [Enter] {} ", i.text("clash.avail.cta_configure"))
                }
            }
            _ => String::new(),
        };
        let primary_w = (primary_label.width() as u16).max(16).min(inner_w);
        if is_no_ctrl && !enabling_ctrl {
            let sec_label = format!(" [c] {} ", i.text("clash.avail.cta_custom"));
            let sec_w = (sec_label.width() as u16).max(12);
            if primary_w + 2 + sec_w <= inner_w {
                let total_w = primary_w + 2 + sec_w;
                let start_x = card_x + card_w.saturating_sub(total_w) / 2;
                (
                    Some(Rect::new(start_x, btn_y, primary_w, 1)),
                    Some(Rect::new(start_x + primary_w + 2, btn_y, sec_w, 1)),
                )
            } else {
                (
                    Some(Rect::new(
                        card_x + card_w.saturating_sub(primary_w) / 2,
                        btn_y,
                        primary_w,
                        1,
                    )),
                    None,
                )
            }
        } else {
            (
                Some(Rect::new(
                    card_x + card_w.saturating_sub(primary_w) / 2,
                    btn_y,
                    primary_w,
                    1,
                )),
                None,
            )
        }
    } else {
        (None, None)
    };

    UnavailableGeom {
        body,
        card,
        cta,
        secondary_cta,
    }
}

fn build_tab_pills(bar: Rect, cl: &ClashState, i: &I18n) -> (Vec<ClashTabPill>, bool, bool) {
    let mut specs: Vec<(ClashTab, String, String, u16)> = ClashTab::ALL
        .iter()
        .map(|&tab| {
            let label = if cl.fullscreen {
                format!("{} {}", tab.index() + 1, i.text(tab.label_key()))
            } else {
                i.text(tab.label_key()).to_owned()
            };
            let count = cl.tab_count(tab);
            let inner = if count.is_empty() {
                format!(" {label} ")
            } else {
                format!(" {label} {count} ")
            };
            let w = inner.width() as u16;
            (tab, label, count, w)
        })
        .collect();

    let total_w: u16 = specs
        .iter()
        .map(|s| s.3)
        .sum::<u16>()
        .saturating_add(specs.len().saturating_sub(1) as u16);

    let active_idx = cl.tab.index();
    let (start, end) = if total_w <= bar.width {
        (0, specs.len())
    } else {
        let budget = bar.width.saturating_sub(4);
        let mut s = active_idx;
        let mut e = active_idx + 1;
        let mut used = specs[active_idx].3;
        loop {
            let mut grew = false;
            if e < specs.len() && used + 1 + specs[e].3 <= budget {
                used += 1 + specs[e].3;
                e += 1;
                grew = true;
            }
            if s > 0 && used + 1 + specs[s - 1].3 <= budget {
                used += 1 + specs[s - 1].3;
                s -= 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        (s, e)
    };

    let overflow_left = start > 0;
    let overflow_right = end < specs.len();
    let mut x = bar.x + if overflow_left { 2 } else { 0 };
    let limit = bar
        .right()
        .saturating_sub(if overflow_right { 2 } else { 0 });
    let mut pills = Vec::new();

    for (tab, label, count, w) in specs.drain(start..end) {
        let avail = limit.saturating_sub(x);
        if avail < 3 {
            break;
        }
        let pw = w.min(avail);
        pills.push(ClashTabPill {
            tab,
            label,
            count,
            rect: Rect::new(x, bar.y, pw, 1),
        });
        x = x.saturating_add(pw + 1);
    }
    (pills, overflow_left, overflow_right)
}

fn build_overview(body: Rect, cl: &ClashState, i: &I18n) -> OverviewGeom {
    let w = body.width;
    let h = body.height;

    // Row 1: Full-width Hero Real-Time Traffic Atlas (always visible across all breakpoints!)
    let traffic_h = if h >= 28 {
        10
    } else if h >= 24 {
        9
    } else if h >= 20 {
        8
    } else if h >= 16 {
        7
    } else if h >= 12 {
        6
    } else {
        5
    }
    .min(h);
    let traffic_card = Rect::new(body.x, body.y, w, traffic_h);

    let mut traffic_mode_chip = None;
    let mut traffic_window_chip = None;
    let mut traffic_scale_chip = None;
    let mut traffic_pause_chip = None;
    let mut traffic_expand_btn = None;
    let mut traffic_plot = Rect::default();
    let mut traffic_side = Rect::default();

    if traffic_card.width >= 20 && traffic_card.height >= 4 {
        let ix = traffic_card.x + 2;
        let iw = traffic_card.width.saturating_sub(4);
        let kpi_y = traffic_card.y + 1;
        let mut right = ix + iw;

        // Allocate right-aligned interactive pills in priority order while preserving KPI space.
        let min_kpi_w: u16 = if iw >= 64 { 26 } else { 18 };
        let expand_lbl = format!(" [v] {} ", i.text("clash.key.traffic_atlas"));
        let ew = expand_lbl.width() as u16;
        if right >= ix + min_kpi_w + ew {
            right -= ew;
            traffic_expand_btn = Some(Rect::new(right, kpi_y, ew, 1));
            right = right.saturating_sub(1);
        }

        let mode_lbl = format!(" [g] {} ", i.text(cl.traffic_mode.label_key()));
        let mw = mode_lbl.width() as u16;
        if right >= ix + min_kpi_w + mw {
            right -= mw;
            traffic_mode_chip = Some(Rect::new(right, kpi_y, mw, 1));
            right = right.saturating_sub(1);
        }

        let win_lbl = format!(" [t] {} ", cl.traffic_window.label());
        let ww = win_lbl.width() as u16;
        if right >= ix + min_kpi_w + ww {
            right -= ww;
            traffic_window_chip = Some(Rect::new(right, kpi_y, ww, 1));
            right = right.saturating_sub(1);
        }

        let scale_lbl = format!(" [y] {} ", i.text(cl.traffic_scale.label_key()));
        let sw = scale_lbl.width() as u16;
        if right >= ix + min_kpi_w + 10 + sw {
            right -= sw;
            traffic_scale_chip = Some(Rect::new(right, kpi_y, sw, 1));
            right = right.saturating_sub(1);
        }

        let pause_key = if cl.is_traffic_paused() {
            "clash.traffic.resume"
        } else {
            "clash.traffic.pause"
        };
        let pause_lbl = format!(" {} ", i.text(pause_key));
        let pw = pause_lbl.width() as u16;
        if right >= ix + min_kpi_w + 16 + pw {
            right -= pw;
            traffic_pause_chip = Some(Rect::new(right, kpi_y, pw, 1));
        }

        // Plot body starts below the KPI row and ends above bottom card border.
        let plot_y = traffic_card.y + 2;
        let plot_h = traffic_card.bottom().saturating_sub(plot_y + 1);
        if plot_h >= 1 && iw >= 8 {
            if iw >= 80 && plot_h >= 4 {
                let side_w = (iw * 31 / 100).clamp(24, 34);
                let plot_w = iw.saturating_sub(side_w + 3);
                traffic_plot = Rect::new(ix, plot_y, plot_w, plot_h);
                traffic_side = Rect::new(ix + plot_w + 3, plot_y, side_w, plot_h);
            } else {
                traffic_plot = Rect::new(ix, plot_y, iw, plot_h);
            }
        }
    }

    // Row 2: Sleek 1-row Outbound Mode strip (Rule / Global / Direct)
    let mode_y = body.y + traffic_h;
    let mode_h = u16::from(mode_y < body.bottom());
    let mode_bar = Rect::new(body.x, mode_y, w, mode_h);
    let mut mode_pills = Vec::new();
    if mode_h > 0 && w >= 24 {
        let py = mode_bar.y;
        let prefix_w = if w >= 48 {
            i.text("clash.overview.mode").width() as u16 + 3
        } else {
            2
        };
        let mut mx = mode_bar.x + prefix_w;
        for (mode_id, key) in [
            ("rule", "clash.mode.rule"),
            ("global", "clash.mode.global"),
            ("direct", "clash.mode.direct"),
        ] {
            let lbl = format!(" {} ", i.text(key));
            let pw = lbl.width() as u16;
            if mx + pw <= mode_bar.right().saturating_sub(1) {
                mode_pills.push((Rect::new(mx, py, pw, 1), mode_id));
                mx += pw + 1;
            }
        }
    }

    // Lower Dashboard Bento Grid (adapts smoothly from 40x16 up to 160x50+)
    let lower_y = mode_y + mode_h;
    let rem = body.bottom().saturating_sub(lower_y);

    let (status_card, services_card, profile_card, connections_card, groups_card) =
        if rem >= 15 && w >= 66 {
            // Wide / Medium (120x36 & 100x30): all 5 lower cards visible
            let row3_h = if rem >= 18 {
                7
            } else if rem >= 16 {
                6
            } else {
                5
            };
            let row4_h = if rem >= 18 { 6 } else { 5 };
            let row5_h = rem.saturating_sub(row3_h + row4_h);
            let r3_left = (w * 53 / 100).max(32).min(w.saturating_sub(28));
            let r3_right = w.saturating_sub(r3_left + 1);
            let r4_left = (w * 48 / 100).max(28).min(w.saturating_sub(28));
            let r4_right = w.saturating_sub(r4_left + 1);
            let row4_y = lower_y + row3_h;
            let row5_y = row4_y + row4_h;
            (
                Rect::new(body.x, lower_y, r3_left, row3_h),
                Rect::new(body.x + r3_left + 1, lower_y, r3_right, row3_h),
                Rect::new(body.x, row4_y, r4_left, row4_h),
                Rect::new(body.x + r4_left + 1, row4_y, r4_right, row4_h),
                if row5_h >= 4 {
                    Rect::new(body.x, row5_y, w, row5_h)
                } else {
                    Rect::default()
                },
            )
        } else if rem >= 9 && w >= 66 {
            // Compact (80x24): Status|Profile + Connections|Groups
            let row3_h = rem / 2;
            let row4_h = rem.saturating_sub(row3_h);
            let left_w = (w * 52 / 100).max(28).min(w.saturating_sub(26));
            let right_w = w.saturating_sub(left_w + 1);
            let row4_y = lower_y + row3_h;
            (
                Rect::new(body.x, lower_y, left_w, row3_h),
                Rect::default(),
                Rect::new(body.x + left_w + 1, lower_y, right_w, row3_h),
                Rect::new(body.x, row4_y, left_w, row4_h),
                Rect::new(body.x + left_w + 1, row4_y, right_w, row4_h),
            )
        } else if rem >= 9 {
            // Mobile (59x24): stacked full-width Status + Groups
            let row3_h = rem / 2;
            let row4_h = rem.saturating_sub(row3_h);
            (
                Rect::new(body.x, lower_y, w, row3_h),
                Rect::default(),
                Rect::default(),
                Rect::default(),
                Rect::new(body.x, lower_y + row3_h, w, row4_h),
            )
        } else if rem >= 4 {
            // Ultra-compact (40x16): full-width Status below Traffic Graph & Mode strip
            (
                Rect::new(body.x, lower_y, w, rem),
                Rect::default(),
                Rect::default(),
                Rect::default(),
                Rect::default(),
            )
        } else {
            (
                Rect::default(),
                Rect::default(),
                Rect::default(),
                Rect::default(),
                Rect::default(),
            )
        };

    let mut outbound_badge_hit = None;
    let mut outbound_ip_hit = None;
    let mut outbound_detail_btn = None;
    let mut start_stop_btn = None;
    let mut reload_btn = None;
    if status_card.width >= 18 && status_card.height >= 4 {
        let ix = status_card.x + 2;
        let iw = status_card.width.saturating_sub(4);
        let ip_y = status_card.y + 2;
        if ip_y < status_card.bottom().saturating_sub(1) && iw >= 10 {
            if cl.exit_probe.is_some() {
                let badge_w = 4u16.min(iw);
                outbound_badge_hit = Some(Rect::new(ix, ip_y, badge_w, 1));
                if iw > badge_w + 1 {
                    outbound_ip_hit = Some(Rect::new(ix + badge_w + 1, ip_y, iw - badge_w - 1, 1));
                }
            } else {
                outbound_ip_hit = Some(Rect::new(ix, ip_y, iw, 1));
            }
        }
    }
    if status_card.width >= 30 && status_card.height >= 5 {
        let btn_y = status_card.bottom().saturating_sub(2);
        let s_lbl = format!(" [s] {} ", i.text("clash.key.stop"));
        let r_lbl = format!(" [R] {} ", i.text("clash.key.reload"));
        let d_lbl = format!(" [i] {} ", i.text("clash.key.outbound_detail"));
        let sw = s_lbl.width() as u16;
        let rw = r_lbl.width() as u16;
        let dw = d_lbl.width() as u16;
        let left = status_card.x + 2;
        let right = status_card.right().saturating_sub(2);
        if right > left + rw + sw {
            reload_btn = Some(Rect::new(right - rw, btn_y, rw, 1));
            let s_x = right - rw - 1 - sw;
            start_stop_btn = Some(Rect::new(s_x, btn_y, sw, 1));
            if s_x > left + dw {
                outbound_detail_btn = Some(Rect::new(left, btn_y, dw, 1));
            }
        }
    }

    let mut profile_switch_btn = None;
    let mut profile_update_btn = None;
    if profile_card.width >= 30 && profile_card.height >= 4 {
        let by = profile_card.y + 1;
        let updating_active = cl
            .snapshot
            .active_profile()
            .is_some_and(|p| cl.busy_profiles.contains(&p.name));
        let u_lbl = if updating_active {
            format!(" * {} ", i.text("clash.key.updating"))
        } else {
            format!(" [u] {} ", i.text("clash.key.update"))
        };
        let sw_lbl = format!(" [p] {} ", i.text("clash.key.switch"));
        let uw = u_lbl.width() as u16;
        let sww = sw_lbl.width() as u16;
        let right = profile_card.right().saturating_sub(2);
        if right > profile_card.x + 12 + uw + sww {
            profile_switch_btn = Some(Rect::new(right - sww, by, sww, 1));
            profile_update_btn = Some(Rect::new(right - sww - 1 - uw, by, uw, 1));
        }
    }

    let mut exit_probe_btn = None;
    let mut socks_secret_btn = None;
    let mut webui_btn = None;
    if services_card.width >= 28 && services_card.height >= 4 {
        let by = services_card.bottom().saturating_sub(2);
        let n_lbl = format!(" [N] {} ", i.text("clash.key.probe_exit"));
        let s_lbl = format!(" [S] {} ", i.text("clash.key.socks_card"));
        let w_key = if cl.snapshot.summary.panel_url.is_some() {
            "clash.key.webui_copy"
        } else {
            "clash.key.webui_install"
        };
        let w_lbl = format!(" [w] {} ", i.text(w_key));
        let nw = n_lbl.width() as u16;
        let sw = s_lbl.width() as u16;
        let ww = w_lbl.width() as u16;
        let left_bound = services_card.x + 2;
        let mut right = services_card.right().saturating_sub(2);
        if right >= left_bound + nw {
            right -= nw;
            exit_probe_btn = Some(Rect::new(right, by, nw, 1));
            right = right.saturating_sub(1);
        }
        if right >= left_bound + sw {
            right -= sw;
            socks_secret_btn = Some(Rect::new(right, by, sw, 1));
            right = right.saturating_sub(1);
        }
        if right >= left_bound + ww {
            right -= ww;
            webui_btn = Some(Rect::new(right, by, ww, 1));
        }
    }

    let mut group_card_hits = Vec::new();
    let mut group_cols = 1usize;
    if groups_card.width >= 18 && groups_card.height >= 4 {
        let inner = Rect::new(
            groups_card.x + 2,
            groups_card.y + 1,
            groups_card.width.saturating_sub(4),
            groups_card.height.saturating_sub(2),
        );
        let cols = if inner.width >= 72 {
            3
        } else if inner.width >= 36 {
            2
        } else {
            1
        };
        group_cols = cols;
        let gap = if cols > 1 { 1u16 } else { 0 };
        let cell_w = inner
            .width
            .saturating_sub(gap.saturating_mul(cols.saturating_sub(1) as u16))
            / (cols as u16);
        let rows = (inner.height / 2).max(1) as usize;
        let groups = cl.visible_groups();
        for idx in 0..groups.len().min(rows * cols) {
            let r = idx / cols;
            let c = idx % cols;
            let cx = inner.x + (c as u16) * (cell_w + gap);
            let cy = inner.y + (r as u16) * 2;
            if cy + 1 < inner.bottom() {
                group_card_hits.push((Rect::new(cx, cy, cell_w, 2), idx));
            }
        }
    }
    cl.last_overview_group_cols.set(group_cols);

    OverviewGeom {
        status_card,
        outbound_badge_hit,
        outbound_ip_hit,
        outbound_detail_btn,
        start_stop_btn,
        reload_btn,
        traffic_card,
        traffic_plot,
        traffic_side,
        traffic_mode_chip,
        traffic_window_chip,
        traffic_scale_chip,
        traffic_pause_chip,
        traffic_expand_btn,
        mode_bar,
        mode_pills,
        profile_card,
        profile_switch_btn,
        profile_update_btn,
        services_card,
        exit_probe_btn,
        socks_secret_btn,
        webui_btn,
        connections_card,
        groups_card,
        group_card_hits,
    }
}

fn build_traffic_fullscreen(body: Rect, cl: &ClashState, i: &I18n) -> TrafficFullscreenGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let toolbar = Rect::new(x, body.y, w, u16::from(body.height > 0));
    let mut right = toolbar.right();
    let title_min = i.text("clash.traffic.atlas_title").width() as u16 + 3;
    let min_title_w: u16 = if w >= 54 {
        title_min.max(20)
    } else {
        title_min.min(w.saturating_sub(12))
    };

    let mut close_btn = None;
    let mut breakdown_chip = None;
    let mut pause_chip = None;
    let mut scale_chip = None;
    let mut window_chip = None;
    let mut mode_chip = None;

    if toolbar.height > 0 && w >= 24 {
        let close_lbl = format!(" [Esc] {} ", i.text("key.close"));
        let cw = close_lbl.width() as u16;
        if right >= x + min_title_w + cw {
            right -= cw;
            close_btn = Some(Rect::new(right, toolbar.y, cw, 1));
            right = right.saturating_sub(1);
        }

        let bd_lbl = format!(" [b] {} ", i.text(cl.traffic_breakdown.label_key()));
        let bw = bd_lbl.width() as u16;
        if right >= x + min_title_w + bw {
            right -= bw;
            breakdown_chip = Some(Rect::new(right, toolbar.y, bw, 1));
            right = right.saturating_sub(1);
        }

        let pause_key = if cl.is_traffic_paused() {
            "clash.traffic.resume"
        } else {
            "clash.traffic.pause"
        };
        let pause_lbl = format!(" [Space] {} ", i.text(pause_key));
        let pw = pause_lbl.width() as u16;
        if right >= x + min_title_w + pw {
            right -= pw;
            pause_chip = Some(Rect::new(right, toolbar.y, pw, 1));
            right = right.saturating_sub(1);
        }

        let scale_lbl = format!(" [y] {} ", i.text(cl.traffic_scale.label_key()));
        let sw = scale_lbl.width() as u16;
        if right >= x + min_title_w + sw {
            right -= sw;
            scale_chip = Some(Rect::new(right, toolbar.y, sw, 1));
            right = right.saturating_sub(1);
        }

        let win_lbl = format!(" [t] {} ", cl.traffic_window.label());
        let ww = win_lbl.width() as u16;
        if right >= x + min_title_w + ww {
            right -= ww;
            window_chip = Some(Rect::new(right, toolbar.y, ww, 1));
            right = right.saturating_sub(1);
        }

        let mode_lbl = format!(" [g] {} ", i.text(cl.traffic_mode.label_key()));
        let mw = mode_lbl.width() as u16;
        if right >= x + min_title_w + mw {
            right -= mw;
            mode_chip = Some(Rect::new(right, toolbar.y, mw, 1));
        }
    }

    let main_y = body.y.saturating_add(1).min(body.bottom());
    let main_h = body.bottom().saturating_sub(main_y);

    let (waveform_card, panel_one, panel_two, panel_three) = if main_h >= 12 {
        let wave_h = (main_h * 58 / 100).max(7).min(main_h.saturating_sub(5));
        let bot_y = main_y + wave_h;
        let bot_h = main_h.saturating_sub(wave_h);
        let wave = Rect::new(body.x, main_y, body.width, wave_h);
        if body.width >= 90 {
            let p1_w = (body.width.saturating_sub(2)) / 3;
            let p2_w = (body.width.saturating_sub(2)) / 3;
            let p3_w = body.width.saturating_sub(p1_w + p2_w + 2);
            (
                wave,
                Rect::new(body.x, bot_y, p1_w, bot_h),
                Rect::new(body.x + p1_w + 1, bot_y, p2_w, bot_h),
                Rect::new(body.x + p1_w + 1 + p2_w + 1, bot_y, p3_w, bot_h),
            )
        } else if body.width >= 62 {
            let p1_w = body.width.saturating_sub(1) / 2;
            let p2_w = body.width.saturating_sub(p1_w + 1);
            (
                wave,
                Rect::new(body.x, bot_y, p1_w, bot_h),
                Rect::new(body.x + p1_w + 1, bot_y, p2_w, bot_h),
                Rect::default(),
            )
        } else {
            (
                wave,
                Rect::new(body.x, bot_y, body.width, bot_h),
                Rect::default(),
                Rect::default(),
            )
        }
    } else {
        (
            Rect::new(body.x, main_y, body.width, main_h),
            Rect::default(),
            Rect::default(),
            Rect::default(),
        )
    };

    TrafficFullscreenGeom {
        area: body,
        toolbar,
        mode_chip,
        window_chip,
        scale_chip,
        pause_chip,
        breakdown_chip,
        close_btn,
        waveform_card,
        panel_one,
        panel_two,
        panel_three,
    }
}

fn build_proxies(body: Rect, cl: &ClashState, i: &I18n) -> ProxiesGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let mut y = body.y;

    // Row 0: Policy group pill bar (FlClash style) + optional right quick-picker `[g]` button
    let picker_lbl = format!(" [g] {} ", i.text("clash.key.groups"));
    let picker_w = picker_lbl.width() as u16;
    let group_picker_btn = if w >= picker_w + 26 && cl.visible_groups().len() > 1 {
        Some(Rect::new(x + w - picker_w, y, picker_w, 1))
    } else {
        None
    };
    let gbar_w = group_picker_btn
        .map(|r| r.x.saturating_sub(x + 1))
        .unwrap_or(w);
    let group_bar = Rect::new(x, y, gbar_w, 1);
    let (group_pills, group_overflow_left, group_overflow_right) = build_group_pills(group_bar, cl);
    y = y.saturating_add(1).min(body.bottom());

    // Row 1: Filter + action toolbar (`/` filter | `[t]` Test | `[o]` Sort | `[h]` Hide | `[P]` Providers)
    let toolbar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    let mut right_edge = toolbar.right();
    let mut providers_btn = None;
    let mut hide_chip = None;
    let mut sort_chip = None;
    let mut test_btn = None;

    if toolbar.height > 0 && w >= 28 {
        if !cl.snapshot.proxy_providers.is_empty() {
            let lbl = format!(
                " [P] {} ({}) ",
                i.text("clash.key.providers"),
                cl.snapshot.proxy_providers.len()
            );
            let bw = lbl.width() as u16;
            if right_edge >= x + 16 + bw {
                right_edge -= bw;
                providers_btn = Some(Rect::new(right_edge, toolbar.y, bw, 1));
                right_edge = right_edge.saturating_sub(1);
            }
        }
        let hide_lbl = format!(
            " [h] {} ",
            i.text(if cl.hide_timeout {
                "clash.filter.hide_on"
            } else {
                "clash.filter.hide_off"
            })
        );
        let hw = hide_lbl.width() as u16;
        if right_edge >= x + 18 + hw {
            right_edge -= hw;
            hide_chip = Some(Rect::new(right_edge, toolbar.y, hw, 1));
            right_edge = right_edge.saturating_sub(1);
        }
        let sort_lbl = format!(" [o] {}^ ", i.text(cl.proxy_sort.label_key()));
        let sw = sort_lbl.width() as u16;
        if right_edge >= x + 12 + sw {
            right_edge -= sw;
            sort_chip = Some(Rect::new(right_edge, toolbar.y, sw, 1));
            right_edge = right_edge.saturating_sub(1);
        }
        let test_lbl = format!(" [t] {} ", i.text("clash.key.test_group"));
        let tw = test_lbl.width() as u16;
        if right_edge >= x + 8 + tw {
            right_edge -= tw;
            test_btn = Some(Rect::new(right_edge, toolbar.y, tw, 1));
            right_edge = right_edge.saturating_sub(1);
        }
    }
    let filter_bar = Rect::new(x, toolbar.y, right_edge.saturating_sub(x), toolbar.height);
    y = y.saturating_add(1).min(body.bottom());

    // Row 2: Active group summary & resolved chain bar
    let chain_bar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    y = y.saturating_add(1).min(body.bottom());

    // Remaining area: Multi-column responsive node card grid (FlClash style)
    let grid_area = Rect::new(x, y, w, body.bottom().saturating_sub(y));
    let cols = if grid_area.width >= 124 {
        4
    } else if grid_area.width >= 88 {
        3
    } else if grid_area.width >= 56 {
        2
    } else {
        1
    };
    cl.last_grid_cols.set(cols);

    let card_h: u16 = if cols == 1 && grid_area.height < 12 {
        1
    } else if cols == 1 {
        2
    } else {
        3
    };
    let nodes = cl.filtered_nodes();
    let total_rows = nodes.len().div_ceil(cols);
    let visible_rows = (grid_area.height / card_h).max(1) as usize;
    let cursor_row = cl.node_cursor / cols;
    let scroll_rows = list_offset(total_rows, cursor_row, visible_rows);

    let gap = if cols > 1 { 1u16 } else { 0u16 };
    let total_gap = gap.saturating_mul(cols.saturating_sub(1) as u16);
    let card_w = grid_area.width.saturating_sub(total_gap) / (cols as u16);
    let mut node_hits = Vec::new();
    for r in 0..visible_rows {
        let abs_row = scroll_rows + r;
        if abs_row >= total_rows {
            break;
        }
        let cy = grid_area.y + (r as u16) * card_h;
        if cy + card_h > grid_area.bottom() {
            break;
        }
        for c in 0..cols {
            let idx = abs_row * cols + c;
            if idx >= nodes.len() {
                break;
            }
            let cx = grid_area.x + (c as u16) * (card_w + gap);
            node_hits.push((Rect::new(cx, cy, card_w, card_h), idx));
        }
    }

    ProxiesGeom {
        group_bar,
        group_pills,
        group_overflow_left,
        group_overflow_right,
        group_picker_btn,
        toolbar,
        filter_bar,
        test_btn,
        sort_chip,
        hide_chip,
        providers_btn,
        chain_bar,
        grid_area,
        node_hits,
    }
}

fn build_group_pills(bar: Rect, cl: &ClashState) -> (Vec<GroupPillHit>, bool, bool) {
    let groups = cl.visible_groups();
    if groups.is_empty() || bar.width < 6 {
        return (Vec::new(), false, false);
    }
    let active = cl.group_cursor.min(groups.len() - 1);
    let widths: Vec<u16> = groups
        .iter()
        .map(|g| (g.name.width().min(18) + 2) as u16)
        .collect();
    let total: u16 = widths
        .iter()
        .sum::<u16>()
        .saturating_add(widths.len().saturating_sub(1) as u16);

    let (start, end) = if total <= bar.width {
        (0, groups.len())
    } else {
        let budget = bar.width.saturating_sub(4);
        let mut s = active;
        let mut e = active + 1;
        let mut used = widths[active].min(budget);
        loop {
            let mut grew = false;
            if e < groups.len() && used + 1 + widths[e] <= budget {
                used += 1 + widths[e];
                e += 1;
                grew = true;
            }
            if s > 0 && used + 1 + widths[s - 1] <= budget {
                used += 1 + widths[s - 1];
                s -= 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        (s, e)
    };

    let overflow_left = start > 0;
    let overflow_right = end < groups.len();
    let mut x = bar.x + if overflow_left { 2 } else { 0 };
    let limit = bar
        .right()
        .saturating_sub(if overflow_right { 2 } else { 0 });
    let mut out = Vec::new();
    for idx in start..end {
        let avail = limit.saturating_sub(x);
        if avail < 4 {
            break;
        }
        let pw = widths[idx].min(avail);
        out.push(GroupPillHit {
            group_idx: idx,
            label: groups[idx].name.clone(),
            rect: Rect::new(x, bar.y, pw, 1),
        });
        x = x.saturating_add(pw + 1);
    }
    (out, overflow_left, overflow_right)
}

fn build_profiles(body: Rect, cl: &ClashState, i: &I18n, framed: bool) -> ProfilesGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let toolbar = Rect::new(x, body.y, w, u16::from(body.height > 0));
    let mut right = toolbar.right();
    let mut providers_btn = None;
    let mut import_btn = None;
    let mut add_btn = None;
    let mut update_all_btn = None;

    if toolbar.height > 0 && w >= 24 {
        for (slot, lbl) in [
            (
                &mut providers_btn,
                format!(" [P] {} ", i.text("clash.key.providers")),
            ),
            (
                &mut import_btn,
                format!(" [i] {} ", i.text("clash.key.import")),
            ),
            (&mut add_btn, format!(" [a] {} ", i.text("clash.key.add"))),
            (
                &mut update_all_btn,
                format!(" [U] {} ", i.text("clash.key.update_all")),
            ),
        ] {
            let bw = lbl.width() as u16;
            if right >= x + 10 + bw {
                right -= bw;
                *slot = Some(Rect::new(right, toolbar.y, bw, 1));
                right = right.saturating_sub(1);
            }
        }
    }
    let filter_bar = Rect::new(x, toolbar.y, right.saturating_sub(x), toolbar.height);

    let main_y = body.y.saturating_add(1).min(body.bottom());
    let main_h = body.bottom().saturating_sub(main_y);
    let (list_area, inspector_area) = if w >= 78 && main_h >= 6 {
        let lw = (w * 56 / 100).max(38);
        let rw = w.saturating_sub(lw + 1);
        (
            Rect::new(x, main_y, lw, main_h),
            Rect::new(x + lw + 1, main_y, rw, main_h),
        )
    } else {
        (Rect::new(x, main_y, w, main_h), Rect::default())
    };

    let pad = u16::from(framed);
    let list_inner = Rect::new(
        list_area.x + pad + 1,
        list_area.y + pad,
        list_area.width.saturating_sub(pad * 2 + 2),
        list_area.height.saturating_sub(pad * 2),
    );
    let row_height: u16 = if list_inner.height >= 8 { 3 } else { 2 };
    let profiles = cl.filtered_profiles();
    let visible = (list_inner.height / row_height).max(1) as usize;
    let list_scroll = list_offset(profiles.len(), cl.profile_cursor, visible);
    let mut item_hits = Vec::new();
    for slot in 0..visible {
        let idx = list_scroll + slot;
        if idx >= profiles.len() {
            break;
        }
        let ry = list_inner.y + (slot as u16) * row_height;
        if ry + row_height <= list_inner.bottom() {
            item_hits.push((
                Rect::new(list_inner.x, ry, list_inner.width, row_height),
                idx,
            ));
        }
    }

    ProfilesGeom {
        toolbar,
        filter_bar,
        update_all_btn,
        add_btn,
        import_btn,
        providers_btn,
        list_area,
        list_inner,
        item_hits,
        inspector_area,
    }
}

fn build_connections(body: Rect, cl: &ClashState, i: &I18n, framed: bool) -> ConnectionsGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let mut y = body.y;

    let toolbar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    let mut right = toolbar.right();
    let mut close_all_btn = None;
    let mut sort_chip = None;
    let mut scope_chip = None;

    if toolbar.height > 0 && w >= 24 {
        let ca_lbl = format!(" [X] {} ", i.text("clash.key.close_all"));
        let cw = ca_lbl.width() as u16;
        if right >= x + 16 + cw {
            right -= cw;
            close_all_btn = Some(Rect::new(right, toolbar.y, cw, 1));
            right = right.saturating_sub(1);
        }
        let sort_lbl = format!(" [o] {}^ ", i.text(cl.conn_sort.label_key()));
        let sw = sort_lbl.width() as u16;
        if right >= x + 12 + sw {
            right -= sw;
            sort_chip = Some(Rect::new(right, toolbar.y, sw, 1));
            right = right.saturating_sub(1);
        }
        let scope_lbl = format!(" [a] {} ", i.text(cl.conn_scope.label_key()));
        let scw = scope_lbl.width() as u16;
        if right >= x + 8 + scw {
            right -= scw;
            scope_chip = Some(Rect::new(right, toolbar.y, scw, 1));
            right = right.saturating_sub(1);
        }
    }
    let filter_bar = Rect::new(x, toolbar.y, right.saturating_sub(x), toolbar.height);
    y = y.saturating_add(1).min(body.bottom());

    // Facet filter chip strip (`All`, `TCP`, `UDP`, `Domain`, `DomainSuffix`, `GeoIP`, ...)
    let facet_bar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    let mut facet_chips = Vec::new();
    if facet_bar.height > 0 {
        let mut fx = facet_bar.x;
        let all_lbl = format!(" {} ", i.text("clash.filter.all"));
        let aw = all_lbl.width() as u16;
        if fx + aw <= facet_bar.right() {
            facet_chips.push((Rect::new(fx, facet_bar.y, aw, 1), None));
            fx += aw + 1;
        }
        for facet in cl.available_conn_facets() {
            let lbl = format!(" {facet} ");
            let fw = lbl.width() as u16;
            if fx + fw > facet_bar.right() {
                break;
            }
            facet_chips.push((Rect::new(fx, facet_bar.y, fw, 1), Some(facet)));
            fx += fw + 1;
        }
    }
    y = y.saturating_add(1).min(body.bottom());

    let main_h = body.bottom().saturating_sub(y);
    let (list_area, inspector_area) = if w >= 98 && main_h >= 6 {
        let lw = (w * 64 / 100).max(56);
        let rw = w.saturating_sub(lw + 1);
        (
            Rect::new(x, y, lw, main_h),
            Rect::new(x + lw + 1, y, rw, main_h),
        )
    } else {
        (Rect::new(x, y, w, main_h), Rect::default())
    };

    let pad = u16::from(framed);
    let table_header = Rect::new(
        list_area.x + pad + 1,
        list_area.y + pad,
        list_area.width.saturating_sub(pad * 2 + 2),
        u16::from(list_area.height > pad * 2 + 1),
    );
    let list_inner = Rect::new(
        table_header.x,
        table_header.y.saturating_add(table_header.height),
        table_header.width,
        list_area
            .bottom()
            .saturating_sub(pad)
            .saturating_sub(table_header.y.saturating_add(table_header.height)),
    );
    let total = cl.filtered_connections().len();
    let list_scroll = list_offset(total, cl.conn_cursor, list_inner.height.max(1) as usize);

    let inspector_body = if inspector_area.width >= 16 && inspector_area.height >= 4 {
        Rect::new(
            inspector_area.x + pad + 1,
            inspector_area.y + pad + 1,
            inspector_area.width.saturating_sub(pad * 2 + 2),
            inspector_area.height.saturating_sub(pad * 2 + 1),
        )
    } else {
        Rect::default()
    };

    ConnectionsGeom {
        toolbar,
        filter_bar,
        scope_chip,
        sort_chip,
        close_all_btn,
        facet_bar,
        facet_chips,
        list_area,
        table_header,
        list_inner,
        list_scroll,
        inspector_area,
        inspector_body,
    }
}

fn build_rules(body: Rect, cl: &ClashState, i: &I18n, framed: bool) -> RulesGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let mut y = body.y;

    let toolbar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    let mut right = toolbar.right();
    let mut providers_btn = None;
    if toolbar.height > 0 && w >= 24 {
        let lbl = format!(" [P] {} ", i.text("clash.key.rule_providers"));
        let bw = lbl.width() as u16;
        if right >= x + 10 + bw {
            right -= bw;
            providers_btn = Some(Rect::new(right, toolbar.y, bw, 1));
            right = right.saturating_sub(1);
        }
    }
    let filter_bar = Rect::new(x, toolbar.y, right.saturating_sub(x), toolbar.height);
    y = y.saturating_add(1).min(body.bottom());

    let facet_bar = Rect::new(x, y, w, u16::from(y < body.bottom()));
    let mut facet_chips = Vec::new();
    if facet_bar.height > 0 {
        let mut fx = facet_bar.x;
        let all_lbl = format!(" {} ", i.text("clash.filter.all"));
        let aw = all_lbl.width() as u16;
        if fx + aw <= facet_bar.right() {
            facet_chips.push((Rect::new(fx, facet_bar.y, aw, 1), None));
            fx += aw + 1;
        }
        for facet in cl.available_rule_facets() {
            let lbl = format!(" {facet} ");
            let fw = lbl.width() as u16;
            if fx + fw > facet_bar.right() {
                break;
            }
            facet_chips.push((Rect::new(fx, facet_bar.y, fw, 1), Some(facet)));
            fx += fw + 1;
        }
    }
    y = y.saturating_add(1).min(body.bottom());

    let main_h = body.bottom().saturating_sub(y);
    let (list_area, providers_area) =
        if w >= 92 && !cl.snapshot.rule_providers.is_empty() && main_h >= 6 {
            let lw = (w * 66 / 100).max(52);
            let rw = w.saturating_sub(lw + 1);
            (
                Rect::new(x, y, lw, main_h),
                Rect::new(x + lw + 1, y, rw, main_h),
            )
        } else {
            (Rect::new(x, y, w, main_h), Rect::default())
        };

    let pad = u16::from(framed);
    let table_header = Rect::new(
        list_area.x + pad + 1,
        list_area.y + pad,
        list_area.width.saturating_sub(pad * 2 + 2),
        u16::from(list_area.height > pad * 2 + 1),
    );
    let list_inner = Rect::new(
        table_header.x,
        table_header.y.saturating_add(table_header.height),
        table_header.width,
        list_area
            .bottom()
            .saturating_sub(pad)
            .saturating_sub(table_header.y.saturating_add(table_header.height)),
    );
    let total = cl.filtered_rules().len();
    let list_scroll = list_offset(total, cl.rule_cursor, list_inner.height.max(1) as usize);

    RulesGeom {
        toolbar,
        filter_bar,
        providers_btn,
        facet_bar,
        facet_chips,
        list_area,
        table_header,
        list_inner,
        list_scroll,
        providers_area,
    }
}

fn build_tools(body: Rect, cl: &ClashState, i: &I18n, framed: bool) -> ToolsGeom {
    let x = body.x + 1;
    let w = body.width.saturating_sub(2);
    let toolbar = Rect::new(x, body.y, w, u16::from(body.height > 0));
    let mut right = toolbar.right();
    let mut logs_btn = None;
    if toolbar.height > 0 && w >= 24 {
        let lbl = format!(
            " [l] {} ",
            i.text(if cl.tools_log_mode {
                "clash.tools.back_list"
            } else {
                "clash.tools.view_logs"
            })
        );
        let bw = lbl.width() as u16;
        if right >= x + 10 + bw {
            right -= bw;
            logs_btn = Some(Rect::new(right, toolbar.y, bw, 1));
            right = right.saturating_sub(1);
        }
    }
    let filter_bar = Rect::new(x, toolbar.y, right.saturating_sub(x), toolbar.height);

    let main_y = body.y.saturating_add(1).min(body.bottom());
    let main_h = body.bottom().saturating_sub(main_y);
    let (list_area, inspector_area) = if cl.tools_log_mode {
        (Rect::default(), Rect::new(x, main_y, w, main_h))
    } else if w >= 78 && main_h >= 6 {
        let lw = (w * 52 / 100).max(36);
        let rw = w.saturating_sub(lw + 1);
        (
            Rect::new(x, main_y, lw, main_h),
            Rect::new(x + lw + 1, main_y, rw, main_h),
        )
    } else {
        (Rect::new(x, main_y, w, main_h), Rect::default())
    };

    let pad = u16::from(framed);
    let list_inner = if list_area.width >= 8 && list_area.height >= 3 {
        Rect::new(
            list_area.x + pad + 1,
            list_area.y + pad,
            list_area.width.saturating_sub(pad * 2 + 2),
            list_area.height.saturating_sub(pad * 2),
        )
    } else {
        Rect::default()
    };
    let total = cl.filtered_tools(i).len();
    let list_scroll = list_offset(total, cl.tool_cursor, list_inner.height.max(1) as usize);

    let inspector_body = if inspector_area.width >= 12 && inspector_area.height >= 4 {
        Rect::new(
            inspector_area.x + pad + 1,
            inspector_area.y + pad + 1,
            inspector_area.width.saturating_sub(pad * 2 + 2),
            inspector_area.height.saturating_sub(pad * 2 + 1),
        )
    } else {
        Rect::default()
    };

    ToolsGeom {
        toolbar,
        filter_bar,
        logs_btn,
        list_area,
        list_inner,
        list_scroll,
        inspector_area,
        inspector_body,
    }
}

/// Interactive row action inside `ClashModal::OutboundIp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum OutboundRowAction {
    CopyIp,
    CopyOrg,
    CopyAsn,
    ToggleHideIp,
}

/// Logical display row shared by `ClashModalGeometry` (hit-testing/scrolling) and `modals::draw`.
#[derive(Debug, Clone)]
pub(in crate::tui) enum OutboundModalRow {
    Tip(String),
    Blank,
    Section(String),
    Probing,
    IpRow {
        label: String,
        value: String,
        level: Option<IpQualityLevel>,
        failed: bool,
        copyable: bool,
    },
    KeyValue {
        label: String,
        value: String,
        tone: Option<Role>,
        action: Option<OutboundRowAction>,
        copy_hint: Option<&'static str>,
    },
    FlagsRow {
        label: String,
        flags: Vec<String>,
        none_label: String,
    },
    HideIpRow {
        label: String,
        enabled: bool,
    },
}

impl OutboundModalRow {
    pub fn action(&self) -> Option<OutboundRowAction> {
        match self {
            Self::IpRow { copyable: true, .. } => Some(OutboundRowAction::CopyIp),
            Self::KeyValue { action, .. } => *action,
            Self::HideIpRow { .. } => Some(OutboundRowAction::ToggleHideIp),
            _ => None,
        }
    }
}

/// Build the ordered content rows of `ClashModal::OutboundIp` so layout and renderer stay aligned.
pub(in crate::tui) fn outbound_modal_rows(cl: &ClashState, i: &I18n) -> Vec<OutboundModalRow> {
    let mut rows = vec![
        OutboundModalRow::Tip(i.text("clash.outbound.tip").to_owned()),
        OutboundModalRow::Blank,
        OutboundModalRow::Section(i.text("clash.outbound.sec_basic").to_owned()),
    ];

    if let Some(probe) = &cl.exit_probe {
        let level = probe.quality_ready().map(|q| q.level());
        rows.push(OutboundModalRow::IpRow {
            label: i.text("clash.outbound.ip_address").to_owned(),
            value: cl.displayed_ip(&probe.ip).to_owned(),
            level,
            failed: false,
            copyable: true,
        });
        if !probe.country.is_empty() {
            rows.push(OutboundModalRow::KeyValue {
                label: i.text("clash.outbound.region").to_owned(),
                value: probe.country.clone(),
                tone: Some(Role::Accent),
                action: None,
                copy_hint: None,
            });
        }
        match &probe.quality {
            IpQualityOutcome::Loading => {
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.level").to_owned(),
                    value: i.text("clash.outbound.quality_loading").to_owned(),
                    tone: Some(Role::Text3),
                    action: None,
                    copy_hint: None,
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.type").to_owned(),
                    value: "...".to_owned(),
                    tone: Some(Role::Text3),
                    action: None,
                    copy_hint: None,
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.flags").to_owned(),
                    value: "...".to_owned(),
                    tone: Some(Role::Text3),
                    action: None,
                    copy_hint: None,
                });
                rows.push(OutboundModalRow::Blank);
                rows.push(OutboundModalRow::Section(
                    i.text("clash.outbound.sec_network").to_owned(),
                ));
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.organization").to_owned(),
                    value: probe.organization_name().unwrap_or("...").to_owned(),
                    tone: probe.organization_name().is_none().then_some(Role::Text3),
                    action: probe
                        .organization_name()
                        .map(|_| OutboundRowAction::CopyOrg),
                    copy_hint: probe.organization_name().map(|_| "[2]"),
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.asn").to_owned(),
                    value: "...".to_owned(),
                    tone: Some(Role::Text3),
                    action: None,
                    copy_hint: None,
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.source").to_owned(),
                    value: if probe.ip_source.is_empty() {
                        "-".to_owned()
                    } else {
                        probe.ip_source.clone()
                    },
                    tone: Some(Role::Text2),
                    action: None,
                    copy_hint: None,
                });
            }
            IpQualityOutcome::Ready(q) => {
                let level_role = match q.level() {
                    IpQualityLevel::Good => Some(Role::Success),
                    IpQualityLevel::Normal => None,
                    IpQualityLevel::Risky => Some(Role::Danger),
                };
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.level").to_owned(),
                    value: i.text(q.level().label_key()).to_owned(),
                    tone: level_role,
                    action: None,
                    copy_hint: None,
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.type").to_owned(),
                    value: i.text(q.ip_type.label_key()).to_owned(),
                    tone: None,
                    action: None,
                    copy_hint: None,
                });
                let flags: Vec<String> = q
                    .flag_keys()
                    .into_iter()
                    .map(|k| i.text(k).to_owned())
                    .collect();
                rows.push(OutboundModalRow::FlagsRow {
                    label: i.text("clash.outbound.flags").to_owned(),
                    flags,
                    none_label: i.text("clash.outbound.flag_none").to_owned(),
                });

                rows.push(OutboundModalRow::Blank);
                rows.push(OutboundModalRow::Section(
                    i.text("clash.outbound.sec_network").to_owned(),
                ));
                let org_opt = probe.organization_name();
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.organization").to_owned(),
                    value: org_opt.unwrap_or("-").to_owned(),
                    tone: org_opt.is_none().then_some(Role::Text3),
                    action: org_opt.map(|_| OutboundRowAction::CopyOrg),
                    copy_hint: org_opt.map(|_| "[2]"),
                });
                let asn_opt = q.asn_label();
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.asn").to_owned(),
                    value: asn_opt.clone().unwrap_or_else(|| "-".to_owned()),
                    tone: asn_opt.is_none().then_some(Role::Text3),
                    action: asn_opt.as_ref().map(|_| OutboundRowAction::CopyAsn),
                    copy_hint: asn_opt.as_ref().map(|_| "[3]"),
                });
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.source").to_owned(),
                    value: q.source.label().to_owned(),
                    tone: Some(Role::Text2),
                    action: None,
                    copy_hint: None,
                });
            }
            IpQualityOutcome::Failed(failures) => {
                rows.push(OutboundModalRow::KeyValue {
                    label: i.text("clash.outbound.type").to_owned(),
                    value: i.text("clash.outbound.quality_failed").to_owned(),
                    tone: Some(Role::Danger),
                    action: None,
                    copy_hint: None,
                });
                if !failures.is_empty() {
                    rows.push(OutboundModalRow::Blank);
                    rows.push(OutboundModalRow::Section(
                        i.text("clash.outbound.sec_sources").to_owned(),
                    ));
                    for failure in failures {
                        rows.push(OutboundModalRow::KeyValue {
                            label: failure.source.label().to_owned(),
                            value: i.text(failure.status.label_key()).to_owned(),
                            tone: Some(Role::Text2),
                            action: None,
                            copy_hint: None,
                        });
                    }
                }
            }
        }
        if probe.google_ms.is_some() || probe.github_ms.is_some() {
            let g = probe
                .google_ms
                .map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".to_owned());
            let gh = probe
                .github_ms
                .map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".to_owned());
            rows.push(OutboundModalRow::KeyValue {
                label: i.text("clash.outbound.latency").to_owned(),
                value: format!("Google {g} / GitHub {gh}"),
                tone: Some(Role::Text2),
                action: None,
                copy_hint: None,
            });
        }
    } else if cl.exit_probing {
        rows.push(OutboundModalRow::Probing);
    } else if let Some(err) = &cl.exit_probe_error {
        rows.push(OutboundModalRow::IpRow {
            label: i.text("clash.outbound.ip_address").to_owned(),
            value: i.text("clash.outbound.check_failed").to_owned(),
            level: None,
            failed: true,
            copyable: false,
        });
        rows.push(OutboundModalRow::KeyValue {
            label: i.text("clash.outbound.error_detail").to_owned(),
            value: err.clone(),
            tone: Some(Role::Danger),
            action: None,
            copy_hint: None,
        });
    } else {
        rows.push(OutboundModalRow::IpRow {
            label: i.text("clash.outbound.ip_address").to_owned(),
            value: "-".to_owned(),
            level: None,
            failed: false,
            copyable: false,
        });
    }

    rows.push(OutboundModalRow::Blank);
    rows.push(OutboundModalRow::Section(
        i.text("clash.outbound.sec_other").to_owned(),
    ));
    rows.push(OutboundModalRow::HideIpRow {
        label: i.text("clash.outbound.hide_ip").to_owned(),
        enabled: cl.hide_ip,
    });
    rows
}

/// Centered modal geometry shared between renderer and mouse hit-tester.
#[derive(Debug, Clone)]
pub(in crate::tui) struct ClashModalGeometry {
    pub box_rect: Rect,
    pub inner: Rect,
    pub subtab_proxy: Option<Rect>,
    pub subtab_rules: Option<Rect>,
    pub field_rects: Vec<Rect>,
    pub item_rows: Vec<(Rect, usize)>,
    pub outbound_body: Rect,
    pub outbound_row_hits: Vec<(Rect, OutboundRowAction)>,
    pub outbound_refresh_btn: Option<Rect>,
    pub outbound_copy_btn: Option<Rect>,
    pub outbound_hide_btn: Option<Rect>,
    pub outbound_close_btn: Option<Rect>,
    pub confirm_yes: Option<Rect>,
    pub confirm_no: Option<Rect>,
}

impl ClashModalGeometry {
    pub fn compute(area: Rect, modal: &ClashModal, cl: &ClashState, i: &I18n) -> Self {
        let outbound_rows =
            matches!(modal, ClashModal::OutboundIp { .. }).then(|| outbound_modal_rows(cl, i));
        let (target_w, target_h) = match modal {
            ClashModal::DelaySettings { .. } => (62u16, 11u16),
            ClashModal::GroupPicker { .. } => {
                let rows = cl.visible_groups().len().clamp(3, 14) as u16;
                (56u16, rows + 5)
            }
            ClashModal::Providers { show_rules, .. } => {
                let count = if *show_rules {
                    cl.snapshot.rule_providers.len()
                } else {
                    cl.snapshot.proxy_providers.len()
                };
                let rows = count.clamp(3, 12) as u16;
                (66u16, rows + 7)
            }
            ClashModal::OutboundIp { .. } => {
                let content_rows = outbound_rows.as_ref().map_or(14, Vec::len).clamp(8, 22) as u16;
                (64u16, content_rows + 5)
            }
            ClashModal::Confirm { .. } => (56u16, 9u16),
        };
        let w = target_w
            .min(area.width.saturating_sub(2))
            .max(24.min(area.width));
        let h = target_h
            .min(area.height.saturating_sub(2))
            .max(7.min(area.height));
        let x = area.x + area.width.saturating_sub(w) / 2;
        let y = area.y + area.height.saturating_sub(h) / 2;
        let box_rect = Rect::new(x, y, w, h);
        let inner = Rect::new(x + 2, y + 2, w.saturating_sub(4), h.saturating_sub(4));

        let mut subtab_proxy = None;
        let mut subtab_rules = None;
        let mut field_rects = Vec::new();
        let mut item_rows = Vec::new();
        let mut outbound_body = Rect::default();
        let mut outbound_row_hits = Vec::new();
        let mut outbound_refresh_btn = None;
        let mut outbound_copy_btn = None;
        let mut outbound_hide_btn = None;
        let mut outbound_close_btn = None;
        let mut confirm_yes = None;
        let mut confirm_no = None;

        match modal {
            ClashModal::GroupPicker { cursor } => {
                let total = cl.visible_groups().len();
                let vis = inner.height.saturating_sub(1).max(1) as usize;
                let scroll = list_offset(total, *cursor, vis);
                for slot in 0..vis {
                    let idx = scroll + slot;
                    if idx >= total {
                        break;
                    }
                    item_rows.push((
                        Rect::new(inner.x, inner.y + slot as u16, inner.width, 1),
                        idx,
                    ));
                }
            }
            ClashModal::Providers { show_rules, cursor } => {
                let p_lbl = format!(
                    " {} ({}) ",
                    i.text("clash.modal.proxy_providers"),
                    cl.snapshot.proxy_providers.len()
                );
                let r_lbl = format!(
                    " {} ({}) ",
                    i.text("clash.modal.rule_providers"),
                    cl.snapshot.rule_providers.len()
                );
                let pw = (p_lbl.width() as u16).min(inner.width / 2);
                let rw = (r_lbl.width() as u16).min(inner.width.saturating_sub(pw + 1));
                if inner.height > 0 && pw > 0 {
                    subtab_proxy = Some(Rect::new(inner.x, inner.y, pw, 1));
                }
                if inner.height > 0 && rw > 0 {
                    subtab_rules = Some(Rect::new(inner.x + pw + 1, inner.y, rw, 1));
                }

                let total = if *show_rules {
                    cl.snapshot.rule_providers.len()
                } else {
                    cl.snapshot.proxy_providers.len()
                };
                let start_y = inner.y + 2;
                let vis = inner.bottom().saturating_sub(start_y) as usize;
                let scroll = list_offset(total, *cursor, vis.max(1));
                for slot in 0..vis {
                    let idx = scroll + slot;
                    if idx >= total {
                        break;
                    }
                    item_rows.push((
                        Rect::new(inner.x, start_y + slot as u16, inner.width, 1),
                        idx,
                    ));
                }
            }
            ClashModal::OutboundIp { scroll } => {
                let by = box_rect.bottom().saturating_sub(2);
                let body_y = box_rect.y + 1;
                let body_h = by.saturating_sub(body_y + 1);
                outbound_body = Rect::new(inner.x, body_y, inner.width, body_h);

                if let Some(rows) = &outbound_rows {
                    let vis = body_h as usize;
                    let max_scroll = rows.len().saturating_sub(vis).min(u16::MAX as usize) as u16;
                    cl.max_outbound_modal_scroll.set(max_scroll);
                    let start = (*scroll).min(max_scroll) as usize;
                    for (slot, row) in rows.iter().skip(start).take(vis).enumerate() {
                        if let Some(action) = row.action() {
                            outbound_row_hits.push((
                                Rect::new(
                                    outbound_body.x,
                                    outbound_body.y + slot as u16,
                                    outbound_body.width,
                                    1,
                                ),
                                action,
                            ));
                        }
                    }
                }

                if by > box_rect.y + 1 && inner.width >= 16 {
                    let mut bx = inner.x;
                    let max_r = inner.right();
                    let r_lbl = format!(" [r] {} ", i.text("clash.outbound.retry"));
                    let rw = r_lbl.width() as u16;
                    if bx + rw <= max_r {
                        outbound_refresh_btn = Some(Rect::new(bx, by, rw, 1));
                        bx = bx.saturating_add(rw + 1);
                    }
                    if cl.exit_probe.is_some() {
                        let c_lbl = format!(" [1] {} ", i.text("clash.outbound.copy_ip"));
                        let cw = c_lbl.width() as u16;
                        if bx + cw <= max_r {
                            outbound_copy_btn = Some(Rect::new(bx, by, cw, 1));
                            bx = bx.saturating_add(cw + 1);
                        }
                    }
                    let h_lbl = format!(" [h] {} ", i.text("clash.outbound.hide_ip"));
                    let hw = h_lbl.width() as u16;
                    if bx + hw <= max_r {
                        outbound_hide_btn = Some(Rect::new(bx, by, hw, 1));
                        bx = bx.saturating_add(hw + 1);
                    }
                    let close_lbl = format!(" [Esc] {} ", i.text("key.close"));
                    let clw = close_lbl.width() as u16;
                    if max_r >= bx + clw {
                        outbound_close_btn = Some(Rect::new(max_r - clw, by, clw, 1));
                    }
                }
            }
            ClashModal::Confirm { .. } => {
                let by = box_rect.bottom().saturating_sub(2);
                let btn_w = 12u16.min(inner.width / 2);
                confirm_yes = Some(Rect::new(
                    inner.x + inner.width.saturating_sub(btn_w * 2 + 2),
                    by,
                    btn_w,
                    1,
                ));
                confirm_no = Some(Rect::new(
                    inner.x + inner.width.saturating_sub(btn_w),
                    by,
                    btn_w,
                    1,
                ));
            }
            ClashModal::DelaySettings { .. } => {
                if inner.height >= 2 {
                    field_rects.push(Rect::new(inner.x, inner.y, inner.width, 2));
                }
                if inner.height >= 5 {
                    field_rects.push(Rect::new(inner.x, inner.y + 3, inner.width, 2));
                }
            }
        }

        Self {
            box_rect,
            inner,
            subtab_proxy,
            subtab_rules,
            field_rects,
            item_rows,
            outbound_body,
            outbound_row_hits,
            outbound_refresh_btn,
            outbound_copy_btn,
            outbound_hide_btn,
            outbound_close_btn,
            confirm_yes,
            confirm_no,
        }
    }

    pub fn contains(&self, pos: Position) -> bool {
        self.box_rect.contains(pos)
    }
}
