//! Live status, split into two side-by-side panels: system and resources (gauges over host
//! facts) and network and proxy. Values reflect real readings only; placeholders are muted
//! and never shown as measurements.
use super::{clip, pad, theme, wrap_cells};
use crate::{
    dashboard::{Reading, Snapshot, Usage},
    ops::clash::State,
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

const LABEL_CAP: usize = 14;
const GAUGE_GAP: u16 = 2;
const MIN_BAR: usize = 8;
/// `{:>4.0}%` renders up to `100%` in five cells.
const PERCENT_WIDTH: usize = 5;
const GAUGES: [&str; 4] = ["dash.cpu", "dash.memory", "dash.disk", "dash.swap"];
/// Gauges sit in a 2×2 grid above the host facts.
const GAUGE_ROWS: usize = 2;
/// Host facts under the gauges in the system panel.
const SYSTEM_FACTS: [&str; 6] = [
    "dash.host",
    "dash.os",
    "dash.kernel_status",
    "dash.arch",
    "dash.cpu",
    "dash.timezone",
];
/// Everything about connectivity in one panel: kernel network stack, public identity, proxy.
const NETWORK_FACTS: [&str; 8] = [
    "dash.network",
    "dash.tcp",
    "dash.ipv4",
    "dash.ipv6",
    "dash.isp",
    "dash.terminal_proxy",
    "dash.mihomo_state",
    "dash.mihomo_version",
];
const FACT_COUNT: usize = SYSTEM_FACTS.len() + NETWORK_FACTS.len();
const fn panel_rows() -> usize {
    let system = GAUGE_ROWS + SYSTEM_FACTS.len();
    if system > NETWORK_FACTS.len() {
        system
    } else {
        NETWORK_FACTS.len()
    }
}
/// Both panels share one height: borders plus the taller panel's rows.
pub(super) const HEIGHT: u16 = 2 + panel_rows() as u16;

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
    let i = &app.ctx.i18n;
    match value {
        Reading::Value(true) => (format!("● {}", i.text(on)), Tone::Success),
        Reading::Value(false) => (format!("○ {}", i.text(off)), Tone::Muted),
        other => placeholder(app, other),
    }
}
fn mihomo(app: &App) -> (String, Tone) {
    let i = &app.ctx.i18n;
    match &app.snapshot.local.mihomo_state {
        Reading::Value(State::Running(pid)) => (
            format!(
                "● {}",
                i.format("dash.mihomo_running", &[("pid", &pid.to_string())])
            ),
            Tone::Success,
        ),
        Reading::Value(State::Stopped) => (format!("○ {}", i.text("dash.stopped")), Tone::Muted),
        Reading::Value(State::NotInstalled) => {
            (format!("○ {}", i.text("dash.not_installed")), Tone::Muted)
        }
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
    format!("{enabled} · {available}{version}")
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

/// Every dashboard fact, keyed by its label; panels and the overview select by key.
pub(super) fn facts(app: &App) -> Vec<Fact> {
    let i = &app.ctx.i18n;
    let data = &app.snapshot.local;
    let public = &app.snapshot.public;
    let text = |value: &Reading<String>| reading(app, value, String::clone);
    let rows: [(&'static str, (String, Tone)); FACT_COUNT] = [
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
    let disabled = || {
        (
            format!("○ {}", app.ctx.i18n.text("status.disabled")),
            Tone::Muted,
        )
    };
    let usage = |value: &Reading<Usage>| match value {
        Reading::Value(_) => disabled(),
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

/// Right-aligned freshness indicator; a refresh in flight spins until new data lands. It
/// shortens to the bare age or spinner when `room` is tight and is omitted rather than
/// overwriting the panel title.
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
    [full, compact]
        .into_iter()
        .map(|text| format!(" {text} "))
        .find(|text| text.width() <= room)
        .map(|text| Line::from(Span::styled(text, theme::fg(tone))).right_aligned())
}
/// Cells left of a block's top border for a right title: borders, the left title and a gap.
pub(super) fn title_room(width: u16, title: &Line<'_>) -> usize {
    (width as usize).saturating_sub(2 + title.width() + 1)
}
pub(super) fn draw(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let facts = facts(app);
    let pick = |keys: &[&str]| {
        keys.iter()
            .map(|key| {
                facts
                    .iter()
                    .find(|fact| fact.key == *key)
                    .expect("panel keys are dashboard facts")
            })
            .collect::<Vec<_>>()
    };
    let [system, network] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);

    let heading = i.text("dash.panel_system");
    let mut block = theme::panel(heading, false).padding(Padding::horizontal(1));
    if let Some(line) = freshness(
        app,
        title_room(system.width, &theme::title(heading, Tone::Plain)),
    ) {
        block = block.title(line);
    }
    let inside = block.inner(system);
    f.render_widget(block, system);
    let [gauge_area, fact_area] =
        Layout::vertical([Constraint::Length(GAUGE_ROWS as u16), Constraint::Min(0)]).areas(inside);
    draw_gauges(app, f, gauge_area);
    draw_facts(app, f, fact_area, &pick(&SYSTEM_FACTS));

    let block = theme::panel(i.text("dash.panel_network"), false).padding(Padding::horizontal(1));
    let inside = block.inner(network);
    f.render_widget(block, network);
    draw_facts(app, f, inside, &pick(&NETWORK_FACTS));
}
fn draw_gauges(app: &App, f: &mut Frame, area: Rect) {
    let i = &app.ctx.i18n;
    let labels = GAUGES.map(|key| i.text(key));
    let label_width = labels.iter().map(|label| label.width()).max().unwrap_or(0);
    let targets = gauge_targets(&app.snapshot);
    // CPU and memory on the first row, disk and swap on the second.
    let cells = Layout::vertical([Constraint::Length(1); GAUGE_ROWS])
        .split(area)
        .iter()
        .flat_map(|row| {
            Layout::horizontal([Constraint::Fill(1); 2])
                .spacing(GAUGE_GAP)
                .split(*row)
                .to_vec()
        })
        .collect::<Vec<_>>();
    let room_of = |index: usize| (cells[index].width as usize).saturating_sub(label_width + 1);
    // Used/total for each gauge that draws a bar; CPU has none.
    let extras: [Option<String>; 4] = std::array::from_fn(|index| {
        gauge_usage(app, index)
            .filter(|_| targets[index].is_some())
            .map(|usage| format!(" {}", amounts(usage)))
    });
    // Amounts appear on every bar or on none, so the row reads as one scale.
    let show_amounts = extras.iter().enumerate().all(|(index, extra)| {
        extra
            .as_ref()
            .is_none_or(|text| room_of(index) >= PERCENT_WIDTH + MIN_BAR + text.width())
    });
    for (index, label) in labels.iter().enumerate() {
        let cell = cells[index];
        let mut spans = vec![Span::styled(pad(label, label_width + 1), theme::muted())];
        let room = room_of(index);
        match targets[index] {
            Some(target) => {
                let shown = app.gauges[index].map_or(target, |tween| tween.value(app.now));
                let tone = theme::load_tone(target);
                let percent = format!("{target:>4.0}%");
                let extra = extras[index].clone().filter(|_| show_amounts);
                let bar_width = room.saturating_sub(
                    percent.width() + extra.as_ref().map_or(0, |text| text.width()),
                );
                spans.extend(theme::bar(shown, bar_width, tone));
                spans.push(Span::styled(percent, theme::fg(tone)));
                if let Some(text) = extra {
                    spans.push(Span::styled(text, theme::muted()));
                }
            }
            None => {
                let (text, tone) = gauge_status(app, index);
                spans.push(Span::styled(clip(&text, room), theme::fg(tone)));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), cell);
    }
}
/// One aligned label/value column. The label column fits the longest label up to a cap, so
/// one long translation shifts only its own row.
fn draw_facts(app: &App, f: &mut Frame, area: Rect, group: &[&Fact]) {
    let i = &app.ctx.i18n;
    let label_width = group
        .iter()
        .map(|fact| i.text(fact.key).width())
        .max()
        .unwrap_or(0)
        .min(LABEL_CAP);
    let lines = group
        .iter()
        .map(|fact| {
            let label = i.text(fact.key);
            let label = if label.width() > label_width {
                label.to_owned()
            } else {
                pad(label, label_width)
            };
            let room = (area.width as usize).saturating_sub(label.width() + 1);
            Line::from(vec![
                Span::styled(format!("{label} "), theme::muted()),
                Span::styled(clip(&fact.value, room), theme::fg(fact.tone)),
            ])
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), area);
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
            theme::fg(Tone::Accent).add_modifier(Modifier::BOLD),
        ));
        for (key, (value, tone)) in rows {
            for (index, part) in wrap_cells(&value, value_width).into_iter().enumerate() {
                let label = if index == 0 { i.text(key) } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  ", pad(label, label_width)), theme::muted()),
                    Span::styled(part, theme::fg(tone)),
                ]));
            }
        }
    }
    let errors = diagnostics(app);
    if !errors.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            i.text("dash.diagnostics").to_owned(),
            theme::fg(Tone::Warning).add_modifier(Modifier::BOLD),
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
