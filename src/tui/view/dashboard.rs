//! The Status_Area: a system panel (CPU and memory, disk and swap, then host facts) beside
//! a network panel (BBR, TCP tuning, public addresses and proxy), seven rows in all. Fact
//! rows are Segment lists shortened from their end; values reflect real readings only and
//! placeholders never pass for measurements. The overview dialog lists every fact in full.
use super::{Segment, pad, theme, wrap_cells};
use crate::{
    dashboard::{Reading, Snapshot, Usage},
    ops::clash::State,
    theme::{Level, Role},
    tui::{App, Tone},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

const GAUGE_GAP: u16 = 2;
/// The shortest sparkline or bar that still reads as a level. The used/total amounts and
/// the CPU count appear only while every gauge keeps at least this much (Requirements 4.3,
/// 4.12).
const MIN_GRAPH: usize = 6;
/// `{:>4.0}%` renders up to `100%` in five cells.
const PERCENT_WIDTH: usize = 5;
/// Gauge labels in `gauge_targets` order: CPU and memory on the first row, disk and swap on
/// the second.
const GAUGES: [&str; 4] = ["dash.cpu", "dash.memory", "dash.disk", "dash.swap"];
/// Content rows between each panel's borders.
const ROWS: usize = 5;
/// The Status_Area's fixed height (Requirement 4.1): a border above and below the content.
pub(super) const HEIGHT: u16 = ROWS as u16 + 2;
/// Labels of the system panel's fact rows, under its two gauge rows.
const SYSTEM_ROWS: [&str; 3] = ["dash.host", "dash.row_kernel", "dash.row_uptime"];
const NETWORK_ROWS: [&str; ROWS] = [
    "dash.row_bbr",
    "dash.row_tcp",
    "dash.ipv4",
    "dash.ipv6",
    "dash.row_proxy",
];
/// A product name, shown as is in every language.
const MIHOMO: &str = "Mihomo";

pub(super) struct Fact {
    pub key: &'static str,
    pub value: String,
    pub tone: Tone,
}

/// Text for every non-value reading state, so a placeholder never passes for a measurement.
/// Callers handle `Value` first; it maps to the same text as `Unavailable` only for
/// exhaustiveness.
fn placeholder<T>(app: &App, reading: &Reading<T>) -> (String, Tone) {
    let i = &app.ctx.i18n;
    match reading {
        Reading::Loading => (
            format!("{} {}", app.spin(), i.text("dash.loading")),
            Tone::Info,
        ),
        Reading::NotQueried => (i.text("dash.not_queried").into(), Tone::Muted),
        Reading::NotApplicable => (i.text("dash.na").into(), Tone::Muted),
        Reading::Unavailable(_) | Reading::Value(_) => {
            (i.text("dash.unknown").into(), Tone::Warning)
        }
    }
}
fn reading<T>(app: &App, value: &Reading<T>, show: impl FnOnce(&T) -> String) -> (String, Tone) {
    match value {
        Reading::Value(v) => (show(v), Tone::Plain),
        other => placeholder(app, other),
    }
}
/// On/off facts lead with a filled or hollow dot, so state is readable without color.
fn switch(app: &App, value: &Reading<bool>, on: &str, off: &str) -> (String, Tone) {
    match value {
        Reading::Value(state) => dot(app, *state, on, off),
        other => placeholder(app, other),
    }
}
fn dot(app: &App, state: bool, on: &str, off: &str) -> (String, Tone) {
    if state { lit(app, on) } else { unlit(app, off) }
}
fn lit(app: &App, key: &str) -> (String, Tone) {
    (
        format!(
            "{} {}",
            theme::glyph(&app.theme.active, theme::Glyph::On),
            app.ctx.i18n.text(key)
        ),
        Tone::Success,
    )
}
fn unlit(app: &App, key: &str) -> (String, Tone) {
    (
        format!(
            "{} {}",
            theme::glyph(&app.theme.active, theme::Glyph::Off),
            app.ctx.i18n.text(key)
        ),
        Tone::Muted,
    )
}
fn mihomo(app: &App) -> (String, Tone) {
    let i = &app.ctx.i18n;
    match &app.snapshot.local.mihomo_state {
        Reading::Value(State::Running(pid)) => (
            format!(
                "{} {}",
                theme::glyph(&app.theme.active, theme::Glyph::On),
                i.format("dash.mihomo_running", &[("pid", &pid.to_string())])
            ),
            Tone::Success,
        ),
        Reading::Value(State::Stopped) => unlit(app, "dash.stopped"),
        Reading::Value(State::NotInstalled) => unlit(app, "dash.not_installed"),
        Reading::Value(State::Unknown) => (i.text("dash.unknown").into(), Tone::Warning),
        other => placeholder(app, other),
    }
}
fn uptime(seconds: u64) -> String {
    format!(
        "{}d {:02}h {:02}m",
        seconds / 86400,
        seconds % 86400 / 3600,
        seconds % 3600 / 60
    )
}
fn bbr(app: &App) -> String {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let enabled = match &data.congestion {
        Reading::Value(v) => i
            .text(if v == "bbr" {
                "status.enabled"
            } else {
                "status.disabled"
            })
            .to_owned(),
        other => placeholder(app, other).0,
    };
    let available = match &data.bbr_available {
        Reading::Value(v) => i
            .text(if *v {
                "dash.available"
            } else {
                "dash.unavailable"
            })
            .to_owned(),
        other => placeholder(app, other).0,
    };
    let version = match &data.bbr_available {
        Reading::Value(false) | Reading::NotApplicable => String::new(),
        _ => format!(" / {}", reading(app, &data.bbr_version, String::clone).0),
    };
    format!(
        "{enabled} {} {available}{version}",
        theme::glyph(&app.theme.active, theme::Glyph::Separator)
    )
}
/// Congestion control, queue and BBR collapse into one fact; Linux-only parts drop out
/// elsewhere instead of repeating "N/A".
pub(super) fn network_summary(app: &App) -> String {
    let data = &app.snapshot.local;
    let mut parts = Vec::new();
    for value in [&data.congestion, &data.qdisc] {
        if !matches!(value, Reading::NotApplicable) {
            parts.push(reading(app, value, String::clone).0);
        }
    }
    if !matches!(data.bbr_available, Reading::NotApplicable)
        || !matches!(data.bbr_version, Reading::NotApplicable)
    {
        parts.push(bbr(app));
    }
    if parts.is_empty() {
        app.ctx.i18n.text("dash.na").into()
    } else {
        parts.join(" / ")
    }
}
fn network(app: &App) -> (String, Tone) {
    if matches!(app.snapshot.local.congestion, Reading::Loading) {
        return placeholder(app, &app.snapshot.local.congestion);
    }
    let summary = network_summary(app);
    let tone = if summary == app.ctx.i18n.text("dash.na") {
        Tone::Muted
    } else {
        Tone::Plain
    };
    (summary, tone)
}

/// Every dashboard fact, keyed by its label, for the overview dialog.
pub(super) fn facts(app: &App) -> Vec<Fact> {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let public = &app.snapshot.public;
    let text = |value: &Reading<String>| reading(app, value, String::clone);
    let rows = [
        ("dash.host", text(&data.host)),
        ("dash.os", text(&data.os)),
        (
            "dash.kernel_status",
            reading(app, &data.kernel, |version| {
                i.format("dash.kernel_running", &[("version", version)])
            }),
        ),
        (
            "dash.arch",
            match &data.uptime {
                Reading::Value(seconds) => {
                    (format!("{} · {}", data.arch, uptime(*seconds)), Tone::Plain)
                }
                other => placeholder(app, other),
            },
        ),
        ("dash.cpu", text(&data.cpu)),
        ("dash.timezone", text(&data.timezone)),
        (
            "dash.tcp",
            switch(
                app,
                &data.tcp_configured,
                "dash.configured",
                "dash.not_configured",
            ),
        ),
        ("dash.network", network(app)),
        ("dash.ipv4", text(&public.ipv4)),
        ("dash.ipv6", text(&public.ipv6)),
        ("dash.isp", text(&public.isp)),
        (
            "dash.terminal_proxy",
            switch(
                app,
                &data.terminal_proxy,
                "status.enabled",
                "status.disabled",
            ),
        ),
        ("dash.mihomo_state", mihomo(app)),
        ("dash.mihomo_version", text(&data.mihomo_version)),
    ];
    rows.into_iter()
        .map(|(key, (value, tone))| Fact { key, value, tone })
        .collect()
}

/// What one reading adds to a Status_Area row: `show` for a Value, otherwise its
/// placeholder. A Linux-only reading that does not apply adds nothing (Requirement 4.11).
fn part<T>(
    app: &App,
    value: &Reading<T>,
    linux_only: bool,
    show: impl FnOnce(&T) -> (String, Tone),
) -> Option<(String, Tone)> {
    match value {
        Reading::Value(v) => Some(show(v)),
        Reading::NotApplicable if linux_only => None,
        other => Some(placeholder(app, other)),
    }
}
fn text_part(app: &App, value: &Reading<String>, linux_only: bool) -> Option<(String, Tone)> {
    part(app, value, linux_only, |text| (text.clone(), Tone::Plain))
}
/// A row's Segments in order. A Segment equal to the one before it adds nothing, so a row
/// waiting on one probe shows its placeholder once. A row left without any Segment reads
/// `dash.na` (Requirement 4.11).
fn row(app: &App, parts: impl IntoIterator<Item = Option<(String, Tone)>>) -> Vec<Segment> {
    let t = &app.theme.active;
    let mut segments: Vec<Segment> = Vec::new();
    for (text, tone) in parts.into_iter().flatten() {
        let segment = Segment::new(text, theme::tone(t, tone));
        if segments.last() != Some(&segment) {
            segments.push(segment);
        }
    }
    if segments.is_empty() {
        segments.push(Segment::new(
            app.ctx.i18n.text("dash.na"),
            theme::tone(t, Tone::Muted),
        ));
    }
    segments
}
/// The system panel's fact rows: host · OS; kernel · architecture · CPU model; uptime ·
/// time zone (Requirement 4.4).
fn system_rows(app: &App) -> [Vec<Segment>; 3] {
    let data = &app.snapshot.local;
    let text = |value| text_part(app, value, false);
    [
        row(app, [text(&data.host), text(&data.os)]),
        row(
            app,
            [
                text(&data.kernel),
                Some((data.arch.clone(), Tone::Plain)),
                text(&data.cpu),
            ],
        ),
        row(
            app,
            [
                part(app, &data.uptime, false, |seconds| {
                    (uptime(*seconds), Tone::Plain)
                }),
                text(&data.timezone),
            ],
        ),
    ]
}
/// The network panel's rows (Requirement 4.5): BBR state · availability / version ·
/// congestion control; TCP tuning · default queue; IPv4 · ISP; IPv6; Mihomo state ·
/// version · terminal proxy. The congestion control and the Mihomo version follow a state
/// that was read, so a probe still pending shows a single placeholder.
fn network_rows(app: &App) -> [Vec<Segment>; ROWS] {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let public = &app.snapshot.public;
    let bbr = part(app, &data.congestion, true, |algorithm| {
        dot(app, algorithm == "bbr", "status.enabled", "status.disabled")
    });
    let support = part(app, &data.bbr_available, true, |available| {
        if !available {
            return (i.text("dash.unavailable").into(), Tone::Plain);
        }
        let version = match &data.bbr_version {
            Reading::Value(version) => format!(" / {version}"),
            Reading::NotApplicable => String::new(),
            other => format!(" / {}", placeholder(app, other).0),
        };
        (
            format!("{}{version}", i.text("dash.available")),
            Tone::Plain,
        )
    });
    let congestion = match &data.congestion {
        Reading::Value(algorithm) => Some((algorithm.clone(), Tone::Plain)),
        _ => None,
    };
    let tuning = part(app, &data.tcp_configured, true, |configured| {
        dot(app, *configured, "dash.configured", "dash.not_configured")
    });
    let state = part(app, &data.mihomo_state, true, |state| match state {
        State::Running(_) => lit(app, "dash.running"),
        State::Stopped => unlit(app, "dash.stopped"),
        State::NotInstalled => unlit(app, "dash.not_installed"),
        State::Unknown => (i.text("dash.unknown").into(), Tone::Warning),
    });
    let version = match &data.mihomo_state {
        Reading::Value(_) => text_part(app, &data.mihomo_version, true),
        _ => None,
    };
    let terminal = part(app, &data.terminal_proxy, false, |enabled| {
        dot(app, *enabled, "status.enabled", "status.disabled")
    });
    let labeled = |label: &str, part: Option<(String, Tone)>| {
        part.map(|(text, tone)| (format!("{label} {text}"), tone))
    };
    [
        row(app, [bbr, support, congestion]),
        row(app, [tuning, text_part(app, &data.qdisc, true)]),
        row(
            app,
            [
                text_part(app, &public.ipv4, false),
                text_part(app, &public.isp, false),
            ],
        ),
        row(app, [text_part(app, &public.ipv6, false)]),
        row(
            app,
            [
                labeled(MIHOMO, state),
                version,
                labeled(i.text("dash.terminal"), terminal),
            ],
        ),
    ]
}

fn usage_percent(value: &Reading<Usage>) -> Option<f64> {
    match value {
        Reading::Value(usage) if usage.total > 0 => Some(usage.percent()),
        _ => None,
    }
}
/// Measured gauge levels in display order; the event loop eases each bar toward these.
pub(in crate::tui) fn gauge_targets(snapshot: &Snapshot) -> [Option<f64>; 4] {
    let data = &snapshot.local;
    [
        match data.cpu_usage {
            Reading::Value(value) => Some(f64::from(value)),
            _ => None,
        },
        usage_percent(&data.memory),
        usage_percent(&data.disk),
        usage_percent(&data.swap),
    ]
}
/// True while any reading is still being collected, which keeps spinners turning.
pub(in crate::tui) fn pending(snapshot: &Snapshot) -> bool {
    let public = &snapshot.public;
    snapshot.local.updated.is_none()
        || matches!(snapshot.local.cpu_usage, Reading::Loading)
        || [&public.ipv4, &public.ipv6, &public.isp]
            .iter()
            .any(|value| matches!(value, Reading::Loading))
}
/// What a gauge shows instead of a level. A zero-sized swap is disabled, not unknown.
fn gauge_status(app: &App, index: usize) -> (String, Tone) {
    let data = &app.snapshot.local;
    let usage = |value: &Reading<Usage>| match value {
        Reading::Value(_) => unlit(app, "status.disabled"),
        other => placeholder(app, other),
    };
    match index {
        0 => placeholder(app, &data.cpu_usage),
        1 => usage(&data.memory),
        2 => usage(&data.disk),
        _ => usage(&data.swap),
    }
}
fn gauge_usage(app: &App, index: usize) -> Option<&Usage> {
    let data = &app.snapshot.local;
    match [None, Some(&data.memory), Some(&data.disk), Some(&data.swap)][index] {
        Some(Reading::Value(usage)) => Some(usage),
        _ => None,
    }
}
/// Compact used/total, e.g. `24.9/29.8G`, shown only when the bar keeps a useful length.
fn amounts(usage: &Usage) -> String {
    const GIB: f64 = 1_073_741_824.0;
    const MIB: f64 = 1_048_576.0;
    let (scale, unit) = if usage.total as f64 >= GIB {
        (GIB, "G")
    } else {
        (MIB, "M")
    };
    let used = usage.used as f64 / scale;
    let total = usage.total as f64 / scale;
    if total >= 100.0 {
        format!("{used:.0}/{total:.0}{unit}")
    } else {
        format!("{used:.1}/{total:.1}{unit}")
    }
}

/// Right-aligned freshness text, shortened to `compact` when `room` is tight and omitted
/// rather than overwriting the panel title.
fn fresh(
    app: &App,
    full: String,
    compact: String,
    tone: Tone,
    room: usize,
) -> Option<Line<'static>> {
    [full, compact]
        .into_iter()
        .map(|text| format!(" {text} "))
        .find(|text| text.width() <= room)
        .map(|text| {
            Line::from(Span::styled(text, theme::tone(&app.theme.active, tone))).right_aligned()
        })
}
/// Freshness of the local data (Requirement 4.8): a refresh in flight spins until new data
/// lands; otherwise the age of the last sample.
pub(super) fn freshness(app: &App, room: usize) -> Option<Line<'static>> {
    let i = &app.ctx.i18n;
    let (full, compact, tone) = if app.refreshing.is_some() {
        let spin = app.spin();
        (
            format!("{spin} {}", i.text("dash.refreshing")),
            spin.to_owned(),
            Tone::Info,
        )
    } else if let Some(at) = app.snapshot.local.updated {
        let age = format!("{}s", app.now.saturating_duration_since(at).as_secs());
        (i.format("dash.age", &[("age", &age)]), age, Tone::Muted)
    } else {
        let spin = app.spin();
        (
            format!("{spin} {}", i.text("dash.loading")),
            spin.to_owned(),
            Tone::Info,
        )
    };
    fresh(app, full, compact, tone, room)
}
/// Freshness of the public data (Requirement 4.9): not looked up yet, a lookup in flight,
/// or the age of the last lookup.
fn public_freshness(app: &App, room: usize) -> Option<Line<'static>> {
    let i = &app.ctx.i18n;
    let public = &app.snapshot.public;
    let (full, compact, tone) = if [&public.ipv4, &public.ipv6, &public.isp]
        .iter()
        .any(|value| matches!(value, Reading::Loading))
    {
        let spin = app.spin();
        (
            format!("{spin} {}", i.text("dash.querying")),
            spin.to_owned(),
            Tone::Info,
        )
    } else if let Some(at) = public.updated {
        let age = format!("{}s", app.now.saturating_duration_since(at).as_secs());
        (
            i.format("dash.public_updated", &[("age", &age)]),
            age,
            Tone::Muted,
        )
    } else {
        let text = i.text("dash.public_not_queried").to_owned();
        (text.clone(), text, Tone::Muted)
    };
    fresh(app, full, compact, tone, room)
}
/// Cells left of a block's top border for a right title: borders, the left title and a gap.
pub(super) fn title_room(width: u16, title: &Line<'_>) -> usize {
    (width as usize).saturating_sub(2 + title.width() + 1)
}

pub(super) fn draw(app: &App, f: &mut Frame, area: Rect) {
    let [mut system, network] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
    if theme::unframed(&app.theme.active) {
        system.width = system.width.saturating_sub(1);
    }
    let rows = draw_panel(app, f, system, "dash.panel_system", freshness);
    draw_gauges(app, f, [rows[0], rows[1]]);
    draw_rows(app, f, &rows[2..], &SYSTEM_ROWS, &system_rows(app));
    let rows = draw_panel(app, f, network, "dash.panel_network", public_freshness);
    draw_rows(app, f, &rows, &NETWORK_ROWS, &network_rows(app));
    if app.theme.active.patterns.border == crate::theme::BorderStyle::Card {
        for row in area.top()..area.bottom().saturating_sub(1) {
            f.buffer_mut()[(system.right(), row)]
                .set_symbol("│")
                .set_style(theme::fg(&app.theme.active, Role::Rule));
        }
    }
}
/// A bordered panel: the title and the freshness of its data on the top border, then its
/// content rows.
fn draw_panel(
    app: &App,
    f: &mut Frame,
    area: Rect,
    heading: &str,
    freshness: fn(&App, usize) -> Option<Line<'static>>,
) -> [Rect; ROWS] {
    let t = &app.theme.active;
    let heading = app.ctx.i18n.text(heading);
    let mut block = theme::panel(t, heading, false).padding(if theme::unframed(t) {
        let padding = theme::padding(t, area.width);
        Padding::new(padding, padding, 0, 1)
    } else {
        Padding::horizontal(1)
    });
    if let Some(line) = freshness(
        app,
        title_room(area.width, &theme::title(t, heading, Role::Text1)),
    ) {
        block = block.title(line);
    }
    let inside = block.inner(area);
    f.render_widget(block, area);
    Layout::vertical([Constraint::Length(1); ROWS]).areas(inside)
}
/// Two one-row gauges per row, CPU beside memory over disk beside swap. CPU draws its
/// history as a sparkline, the others a bar; each shows its percentage. The used/total
/// amounts and the CPU count show on every gauge or on none, so the rows read as one scale
/// (Requirements 4.2, 4.3, 4.12).
fn draw_gauges(app: &App, f: &mut Frame, rows: [Rect; 2]) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let halves = |row: Rect| -> [Rect; 2] {
        Layout::horizontal([Constraint::Fill(1); 2])
            .spacing(GAUGE_GAP)
            .areas(row)
    };
    let ([cpu, memory], [disk, swap]) = (halves(rows[0]), halves(rows[1]));
    let cells = [cpu, memory, disk, swap];
    let labels = GAUGES.map(|key| i.text(key));
    // Each column aligns its own labels: CPU over disk, memory over swap.
    let label_widths: [usize; 4] =
        std::array::from_fn(|index| labels[index % 2].width().max(labels[index % 2 + 2].width()));
    let rooms: [usize; 4] = std::array::from_fn(|index| {
        (cells[index].width as usize).saturating_sub(label_widths[index] + 1)
    });
    let targets = gauge_targets(&app.snapshot);
    let extras: [Option<String>; 4] = std::array::from_fn(|index| {
        targets[index]?;
        if index > 0 {
            return gauge_usage(app, index).map(|usage| format!(" {}", amounts(usage)));
        }
        match app.snapshot.local.cpu_count {
            Reading::Value(count) => Some(format!(
                " {}",
                i.format("dash.cores", &[("count", &count.to_string())])
            )),
            _ => None,
        }
    });
    let show_extras = extras.iter().zip(rooms).all(|(extra, room)| {
        extra
            .as_ref()
            .is_none_or(|text| room >= PERCENT_WIDTH + MIN_GRAPH + text.width())
    });
    for (index, cell) in cells.into_iter().enumerate() {
        let room = rooms[index];
        let mut spans = vec![Span::styled(
            pad(labels[index], label_widths[index] + 1),
            theme::muted(t),
        )];
        match targets[index] {
            Some(target) => {
                // The measured level picks the color; a bar glides toward it.
                let role = theme::gauge(target);
                let percent = format!("{target:>4.0}%");
                let extra = extras[index].clone().filter(|_| show_extras);
                let graph = room.saturating_sub(
                    percent.width() + extra.as_ref().map_or(0, |text| text.width()),
                );
                if index == 0 {
                    let history = theme::sparkline(t, &app.snapshot.cpu_history, graph);
                    spans.push(Span::styled(history, theme::fg(t, role)));
                } else {
                    let shown = app.gauges[index].map_or(target, |tween| tween.value(app.now));
                    spans.extend(theme::bar(t, shown, graph, role));
                }
                if theme::unframed(t) {
                    spans.push(Span::styled(
                        format!("{target:>4.0}"),
                        theme::text(t, Level::Primary).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled("%", theme::text(t, Level::Secondary)));
                } else {
                    spans.push(Span::styled(percent, theme::fg(t, role)));
                }
                if let Some(text) = extra {
                    if theme::unframed(t) {
                        spans.extend(text.chars().map(|character| {
                            Span::styled(
                                character.to_string(),
                                theme::text(
                                    t,
                                    if character.is_alphabetic() {
                                        Level::Tertiary
                                    } else {
                                        Level::Secondary
                                    },
                                ),
                            )
                        }));
                    } else {
                        spans.push(Span::styled(text, theme::muted(t)));
                    }
                }
            }
            None => {
                let (text, tone) = gauge_status(app, index);
                spans.push(Span::styled(
                    super::clip_with(&text, room, super::ellipsis(t)),
                    theme::tone(t, tone),
                ));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), cell);
    }
}
/// Labeled rows: a label column as wide as the panel's longest label, then as many whole
/// Segments as fit (Requirements 4.6, 4.7).
fn draw_rows(app: &App, f: &mut Frame, areas: &[Rect], labels: &[&str], rows: &[Vec<Segment>]) {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let label_width = labels
        .iter()
        .map(|key| i.text(key).width())
        .max()
        .unwrap_or(0);
    for ((area, key), segments) in areas.iter().zip(labels).zip(rows) {
        let room = (area.width as usize).saturating_sub(label_width + 1);
        let mut spans = vec![Span::styled(
            pad(i.text(key), label_width + 1),
            theme::muted(t),
        )];
        spans.extend(super::themed_segment_spans(
            t,
            segments,
            room,
            theme::text(t, Level::Tertiary),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), *area);
    }
}

fn gib(value: u64) -> String {
    let gib = value as f64 / 1_073_741_824.0;
    if gib >= 1.0 {
        format!("{gib:.1} GiB")
    } else {
        format!("{:.0} MiB", value as f64 / 1_048_576.0)
    }
}
fn resource(app: &App, index: usize) -> (String, Tone) {
    let Some(percent) = gauge_targets(&app.snapshot)[index] else {
        return gauge_status(app, index);
    };
    let mut text = format!("{percent:.1}%");
    if let Some(usage) = gauge_usage(app, index) {
        text = format!("{text} · {} / {}", gib(usage.used), gib(usage.total));
    }
    if index == 2 && !app.snapshot.local.disk_name.is_empty() {
        text = format!("{} · {text}", app.snapshot.local.disk_name);
    }
    (text, theme::load_tone(percent))
}
/// Full system status for the overview dialog: grouped sections, untruncated values wrapped
/// under their label, then probe diagnostics.
pub(super) fn overview(app: &App, width: usize) -> Vec<Line<'static>> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let facts = facts(app);
    let fact = |key: &str| {
        let fact = facts
            .iter()
            .find(|fact| fact.key == key)
            .expect("overview keys are dashboard facts");
        (fact.value.clone(), fact.tone)
    };
    let mut network = vec![
        ("dash.network", fact("dash.network")),
        ("dash.tcp", fact("dash.tcp")),
        ("dash.ipv4", fact("dash.ipv4")),
        ("dash.ipv6", fact("dash.ipv6")),
        ("dash.isp", fact("dash.isp")),
    ];
    if let Some(at) = app.snapshot.public.updated {
        let age = format!("{}s", app.now.saturating_duration_since(at).as_secs());
        network.push(("dash.public_age", (age, Tone::Muted)));
    }
    let sections = [
        (
            "dash.section_system",
            [
                "dash.host",
                "dash.os",
                "dash.kernel_status",
                "dash.arch",
                "dash.cpu",
                "dash.timezone",
            ]
            .map(|key| (key, fact(key)))
            .to_vec(),
        ),
        (
            "dash.section_resources",
            vec![
                ("dash.cpu_usage", resource(app, 0)),
                ("dash.memory", resource(app, 1)),
                ("dash.disk", resource(app, 2)),
                ("dash.swap", resource(app, 3)),
            ],
        ),
        ("dash.section_network", network),
        (
            "dash.section_proxy",
            [
                "dash.terminal_proxy",
                "dash.mihomo_state",
                "dash.mihomo_version",
            ]
            .map(|key| (key, fact(key)))
            .to_vec(),
        ),
    ];
    let label_width = sections
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|(key, _)| i.text(key).width()))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let value_width = width.saturating_sub(label_width + 2).max(1);
    let mut lines = Vec::new();
    for (heading, rows) in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(
            i.text(heading).to_owned(),
            theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
        ));
        for (key, (value, tone)) in rows {
            for (index, part) in wrap_cells(&value, value_width).into_iter().enumerate() {
                let label = if index == 0 { i.text(key) } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  ", pad(label, label_width)), theme::muted(t)),
                    Span::styled(part, theme::tone(t, tone)),
                ]));
            }
        }
    }
    let errors = diagnostics(app);
    if !errors.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            i.text("dash.diagnostics").to_owned(),
            theme::fg(t, Role::Warning).add_modifier(Modifier::BOLD),
        ));
        for error in errors {
            for part in wrap_cells(&error, width.max(1)) {
                lines.push(Line::from(part));
            }
        }
    }
    lines
}
/// Probe errors stay visible as `label: detail`; nothing is replaced by an invented value.
fn diagnostics(app: &App) -> Vec<String> {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let public = &app.snapshot.public;
    let mut errors = Vec::new();
    for (key, value) in [
        ("dash.host", &data.host),
        ("dash.os", &data.os),
        ("dash.kernel_status", &data.kernel),
        ("dash.cpu", &data.cpu),
        ("dash.timezone", &data.timezone),
        ("dash.mihomo_version", &data.mihomo_version),
        ("dash.network", &data.congestion),
        ("dash.qdisc", &data.qdisc),
        ("dash.ipv4", &public.ipv4),
        ("dash.ipv6", &public.ipv6),
        ("dash.isp", &public.isp),
    ] {
        if let Reading::Unavailable(error) = value {
            errors.push(format!("{}: {error}", i.text(key)));
        }
    }
    match &data.mihomo_state {
        Reading::Unavailable(error) => {
            errors.push(format!("{}: {error}", i.text("dash.mihomo_state")));
        }
        Reading::Value(State::Unknown) => errors.push(i.text("dash.mihomo_unverified").into()),
        _ => (),
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::snapshot::{fixture_app, render};

    fn texts(row: &[Segment]) -> Vec<&str> {
        row.iter().map(|segment| segment.text.as_str()).collect()
    }
    /// The seven Status_Area rows of `app` drawn `width` cells wide; en-US cells are all
    /// single width.
    fn status(app: &App, width: u16) -> Vec<String> {
        let buffer = render(app, width, 36);
        (1..=HEIGHT)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn fixture_rows_follow_the_panel_layout() {
        let app = fixture_app("en-US");
        let t = &app.theme.active;
        let [host, kernel, uptime] = system_rows(&app);
        assert_eq!(
            texts(&host),
            ["vps-sg-01", "Debian GNU/Linux 12 (bookworm)"]
        );
        assert_eq!(
            texts(&kernel),
            ["6.12.8-joeyblog-bbrv3", "x86_64", "AMD EPYC 7B13 × 2"]
        );
        assert_eq!(texts(&uptime), ["3d 04h 00m", "Asia/Shanghai"]);
        let network = network_rows(&app);
        assert_eq!(
            network.iter().map(|row| texts(row)).collect::<Vec<_>>(),
            [
                vec!["○ Disabled", "available / v3", "cubic"],
                vec!["○ not configured", "fq_codel"],
                vec!["203.0.113.24", "AS64500 Example Net"],
                vec!["2001:db8:3c4d:15::1a2f"],
                vec!["Mihomo ● Running", "v1.19.2", "Terminal ● Enabled"],
            ]
        );
        assert_eq!(network[4][0].style, theme::tone(t, Tone::Success));
        assert_eq!(network[1][0].style, theme::tone(t, Tone::Muted));
        let zh = fixture_app("zh-CN");
        assert_eq!(
            texts(&network_rows(&zh)[4]),
            ["Mihomo ● 运行中", "v1.19.2", "终端 ● 已启用"]
        );

        // Gauges: the CPU history and count, then bars with percentages and amounts.
        // A top border with both titles, five content rows and a bottom border.
        let wide = status(&app, 120);
        assert!(wide[0].contains("updated 2s ago") && wide[0].contains("looked up 12s ago"));
        assert!(wide[1].contains("│ CPU  ▅▄▃▂▃▁▁▂▂▁  12% 2-core  Memory ━━━━──  63% 1.2/1.9G │"));
        assert!(wide[2].contains("│ Disk ━━╸───  41% 16.4/40.0G  Swap   ╸─────  10% 0.2/2.0G │"));
        assert!(wide[3].contains("│ Host   vps-sg-01 · Debian GNU/Linux 12 (bookworm)  "));
        assert!(wide[3].contains("│ IPv4  203.0.113.24 · AS64500 Example Net  "));
        assert!(wide[4].contains("│ Kernel 6.12.8-joeyblog-bbrv3 · x86_64  "));
        assert!(wide[4].contains("│ IPv6  2001:db8:3c4d:15::1a2f  "));
        assert!(wide[5].contains("│ Uptime 3d 04h 00m · Asia/Shanghai  "));
        assert!(wide[5].contains("│ Proxy Mihomo ● Running · v1.19.2 · Terminal ● Enabled  "));
        assert!(wide[6].starts_with('╰'));
        // At 80 columns the amounts and the CPU count go as one group, and rows lose whole
        // Segments from their end.
        let narrow = status(&app, 80);
        assert!(narrow[1].contains("│ CPU  ▂▃▁▁▂▂▁  12%  Memory ━━━──  63% │"));
        assert!(
            narrow[1..3]
                .iter()
                .all(|row| !row.chars().take(40).any(|c| c == '/')),
            "no used/total amounts in the system panel"
        );
        assert!(narrow[3].contains("│ Host   vps-sg-01     "));
        assert!(narrow[3].contains("│ IPv4  203.0.113.24     "));
        assert!(narrow[4].contains("│ Kernel 6.12.8-joeyblog-bbrv3     "));
        assert!(narrow[5].contains("│ Proxy Mihomo ● Running · v1.19.2     │"));
    }

    /// Every reading state other than Value, in the order of `placeholders`.
    fn states<T>() -> [Reading<T>; 4] {
        [
            Reading::Loading,
            Reading::NotQueried,
            Reading::Unavailable("probe failed".into()),
            Reading::NotApplicable,
        ]
    }
    #[test]
    fn readings_other_than_values_show_their_placeholder() {
        let mut app = fixture_app("en-US");
        let i = &app.ctx.i18n;
        let placeholders = [
            (
                format!("{} {}", app.spin(), i.text("dash.loading")),
                Tone::Info,
            ),
            (i.text("dash.not_queried").into(), Tone::Muted),
            (i.text("dash.unknown").into(), Tone::Warning),
            (i.text("dash.na").into(), Tone::Muted),
        ]
        .map(|(text, tone)| Segment::new(text, theme::tone(&app.theme.active, tone)));
        let terminal = i.text("dash.terminal").to_owned();
        let cases = placeholders
            .into_iter()
            .zip(states::<String>())
            .zip(states::<bool>());
        for ((expected, text), switch) in cases {
            app.snapshot.local.host = text.clone();
            app.snapshot.local.terminal_proxy = switch;
            app.snapshot.public.ipv4 = text.clone();
            app.snapshot.public.isp = text.clone();
            app.snapshot.public.ipv6 = text;
            let [host, ..] = system_rows(&app);
            assert_eq!(host[0], expected, "the host");
            assert_eq!(host[1].text, "Debian GNU/Linux 12 (bookworm)");
            let network = network_rows(&app);
            // Equal placeholders side by side show once.
            assert_eq!(network[2], std::slice::from_ref(&expected), "IPv4 and ISP");
            assert_eq!(network[3], std::slice::from_ref(&expected), "IPv6");
            assert_eq!(network[4][2].text, format!("{terminal} {}", expected.text));
            assert_eq!(network[4][2].style, expected.style);
        }
    }

    #[test]
    fn linux_only_readings_that_do_not_apply_leave_out_their_segments() {
        let mut app = fixture_app("en-US");
        let na = || {
            Segment::new(
                app.ctx.i18n.text("dash.na"),
                theme::muted(&app.theme.active),
            )
        };
        app.snapshot.local.bbr_version = Reading::NotApplicable;
        app.snapshot.local.qdisc = Reading::NotApplicable;
        let network = network_rows(&app);
        assert_eq!(texts(&network[0]), ["○ Disabled", "available", "cubic"]);
        assert_eq!(texts(&network[1]), ["○ not configured"]);

        // What a non-Linux sample holds: every Linux-only reading is NotApplicable.
        let data = &mut app.snapshot.local;
        data.congestion = Reading::NotApplicable;
        data.bbr_available = Reading::NotApplicable;
        data.tcp_configured = Reading::NotApplicable;
        data.mihomo_state = Reading::NotApplicable;
        data.mihomo_version = Reading::NotApplicable;
        let network = network_rows(&app);
        assert_eq!(network[0], [na()], "a row with nothing left reads N/A");
        assert_eq!(network[1], [na()]);
        assert_eq!(texts(&network[4]), ["Terminal ● Enabled"]);
        // A reading other platforms provide keeps its placeholder instead.
        app.snapshot.local.timezone = Reading::NotApplicable;
        assert_eq!(system_rows(&app)[2][1], na());
    }

    #[test]
    fn each_panel_title_shows_the_freshness_of_its_own_data() {
        let mut app = fixture_app("en-US");
        let split = 60;
        let title = |app: &App| status(app, 120).remove(0);
        let at = |line: &str, text: &str| line.find(text).map(|byte| line[..byte].chars().count());
        let line = title(&app);
        assert!(
            at(&line, "updated 2s ago").is_some_and(|x| x < split),
            "{line}"
        );
        assert!(
            at(&line, "looked up 12s ago").is_some_and(|x| x >= split),
            "{line}"
        );

        app.refreshing = Some(app.now);
        app.snapshot.public = Default::default();
        let line = title(&app);
        assert!(
            at(&line, "Refreshing…").is_some_and(|x| x < split),
            "{line}"
        );
        assert!(
            at(&line, "not looked up").is_some_and(|x| x >= split),
            "{line}"
        );

        app.snapshot.public.ipv4 = Reading::Loading;
        let looking = format!("{} Looking up…", app.spin());
        assert!(at(&title(&app), &looking).is_some_and(|x| x >= split));
    }
}
