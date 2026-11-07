//! Related_Reading (design component 4): the Snapshot readings that show what an Action
//! changes, turned into its Row_Tag, the Details_Panel's "current state" and the "= current"
//! rule. A reading that is not a Value adds no tag; the state shows its placeholder instead
//! (Requirements 4.10, 5.7, 5.8). Nothing here reads the clock or the theme: callers pass
//! `now`, and the view styles the tag.
use super::view::Segment;
use crate::{
    actions::{Parameters, Spec},
    dashboard::{Reading, Snapshot, Usage},
    i18n::I18n,
    ops::{clash::State, memory},
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::time::{Duration, Instant};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// The "= current" rule: whether running the Action with these parameters would leave the
/// readings as they are.
type MatchesCurrent = fn(&Parameters, &Snapshot) -> bool;

/// What the Snapshot says about one Action.
pub(super) struct Related {
    /// The Row_Tag's Segments in order, each in the default style for the view to draw in
    /// its tag style; empty while the reading is not a Value.
    pub tag: Vec<Segment>,
    /// The Details_Panel's "current state": a label span, a gap and a value span per
    /// reading; a reading that is not a Value shows its placeholder.
    pub state: Vec<Line<'static>>,
    /// Never holds for an Action whose table row has no rule.
    pub matches_current: MatchesCurrent,
}

pub(super) fn summary(spec: &Spec, params: &Parameters) -> String {
    spec.fields
        .iter()
        .filter_map(|field| {
            let value = params.get(field.name).map_or(field.default, String::as_str);
            if value.is_empty() {
                return None;
            }
            Some(match field.name {
                "bandwidth" => format!("{value} Mbit/s"),
                "rtt" => format!("{value} ms"),
                "size_mb" => format!("{value} MiB"),
                _ => value.to_owned(),
            })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Change {
    pub label: &'static str,
    pub from: String,
    pub to: String,
}

pub(super) fn expected(spec: &Spec, params: &Parameters, snapshot: &Snapshot) -> Vec<Change> {
    let local = &snapshot.local;
    let change = |label, from: Option<&String>, to: Option<&String>| {
        from.zip(to).map(|(from, to)| Change {
            label,
            from: from.clone(),
            to: to.clone(),
        })
    };
    match spec.id {
        "tcp.tune" => {
            let target = value(&local.effective_memory_kb)
                .zip(params.get("bandwidth").zip(params.get("rtt")))
                .and_then(|(memory_kb, (bandwidth, rtt))| {
                    memory::tcp_buffer(bandwidth, rtt, *memory_kb).ok()
                });
            value(&local.tcp_rmem_max)
                .zip(target)
                .map(|(from, to)| Change {
                    label: "related.tcp_buffer",
                    from: mib(*from),
                    to: mib(to),
                })
                .into_iter()
                .collect()
        }
        "bbr.enable" => [
            value(&local.congestion).map(|from| Change {
                label: "related.congestion",
                from: from.clone(),
                to: "bbr".into(),
            }),
            change("dash.qdisc", value(&local.qdisc), params.get("qdisc")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "system.timezone" => change(
            "dash.timezone",
            value(&local.timezone),
            params.get("timezone"),
        )
        .into_iter()
        .collect(),
        _ => Vec::new(),
    }
}

fn mib(bytes: u64) -> String {
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    }
}

/// The Related_Reading of `spec`, or `None` for an Action the table does not list. `now` is
/// the frame clock, which dates the public lookup.
pub(super) fn related(
    spec: &Spec,
    snapshot: &Snapshot,
    i18n: &I18n,
    now: Instant,
) -> Option<Related> {
    let local = &snapshot.local;
    let text = |key: &str| i18n.text(key).to_owned();
    let (tag, state, matches_current): (Vec<String>, Vec<Line<'static>>, MatchesCurrent) =
        match spec.id {
            "system.timezone" => (
                value(&local.timezone).cloned().into_iter().collect(),
                vec![fact(i18n, "dash.timezone", &local.timezone, String::clone)],
                same_timezone,
            ),
            "swap.create" | "swap.auto" | "swap.remove" => (
                value(&local.swap)
                    .map(|swap| {
                        if swap.total == 0 {
                            text("status.disabled")
                        } else {
                            i18n.format("tag.swap", &[("size", &capacity(swap.total))])
                        }
                    })
                    .into_iter()
                    .collect(),
                vec![fact(i18n, "dash.swap", &local.swap, |swap| {
                    if swap.total == 0 {
                        text("status.disabled")
                    } else {
                        format!("{} / {}", capacity(swap.used), capacity(swap.total))
                    }
                })],
                never,
            ),
            "system.clean" => (
                value(&local.disk)
                    .map(|disk| format!("{} {:.0}%", i18n.text("dash.disk"), disk.percent()))
                    .into_iter()
                    .collect(),
                vec![fact(i18n, "dash.disk", &local.disk, |disk| {
                    disk_state(i18n, disk)
                })],
                never,
            ),
            "bbr.status" | "bbr.enable" => {
                let (tag, state) = bbr(snapshot, i18n);
                let rule: MatchesCurrent = if spec.id == "bbr.enable" {
                    bbr_current
                } else {
                    never
                };
                (tag, state, rule)
            }
            "tcp.tune" => {
                let tuned = |configured: &bool| {
                    if *configured {
                        lit(i18n.text("tag.tcp_tuned"))
                    } else {
                        unlit(i18n.text("tag.tcp_untuned"))
                    }
                };
                let tag = match value(&local.tcp_configured) {
                    Some(true) => std::iter::once(tuned(&true))
                        .chain(value(&local.tcp_rmem_max).map(|max| capacity(*max)))
                        .collect(),
                    Some(false) => vec![tuned(&false)],
                    None => Vec::new(),
                };
                let state = vec![
                    fact(i18n, "dash.tcp", &local.tcp_configured, tuned),
                    fact(i18n, "related.tcp_buffer", &local.tcp_rmem_max, |max| {
                        capacity(*max)
                    }),
                    fact(
                        i18n,
                        "related.effective_memory",
                        &local.effective_memory_kb,
                        |kb| capacity(kb.saturating_mul(1024)),
                    ),
                ];
                (tag, state, tcp_current)
            }
            "system.public_ip" => {
                let public = &snapshot.public;
                let tag = match public.updated {
                    Some(at) => vec![relative_age(i18n, now.saturating_duration_since(at))],
                    // A lookup in flight has no age yet and is no longer "not queried".
                    None if [&public.ipv4, &public.ipv6, &public.isp]
                        .iter()
                        .any(|reading| matches!(reading, Reading::Loading)) =>
                    {
                        Vec::new()
                    }
                    None => vec![text("tag.not_queried")],
                };
                let state = [
                    ("dash.ipv4", &public.ipv4),
                    ("dash.ipv6", &public.ipv6),
                    ("dash.isp", &public.isp),
                ]
                .into_iter()
                .map(|(label, reading)| fact(i18n, label, reading, String::clone))
                .collect();
                (tag, state, never)
            }
            "clash.status" | "clash.start" | "clash.stop" => (
                value(&local.mihomo_state)
                    .map(|state| mihomo(i18n, state))
                    .into_iter()
                    .collect(),
                vec![fact(
                    i18n,
                    "dash.mihomo_state",
                    &local.mihomo_state,
                    |state| match state {
                        State::Running(pid) => {
                            lit(&i18n.format("dash.mihomo_running", &[("pid", &pid.to_string())]))
                        }
                        other => mihomo(i18n, other),
                    },
                )],
                never,
            ),
            "clash.install" => {
                // No version is read when Mihomo is not installed; its state says so.
                let missing = matches!(local.mihomo_state, Reading::Value(State::NotInstalled));
                let version = match (&local.mihomo_version, missing) {
                    (Reading::Value(version), _) => Some(version.clone()),
                    (_, true) => Some(text("dash.not_installed")),
                    _ => None,
                };
                let state = match &version {
                    Some(version) => line(i18n, "dash.mihomo_version", version.clone()),
                    None => fact(i18n, "dash.mihomo_version", &local.mihomo_version, |_| {
                        String::new()
                    }),
                };
                (version.into_iter().collect(), vec![state], never)
            }
            _ => return None,
        };
    Some(Related {
        tag: tag
            .into_iter()
            .map(|text| Segment::new(text, Style::default()))
            .collect(),
        state,
        matches_current,
    })
}

/// Relative_Age (Requirement 29.3): seconds under a minute, minutes under an hour, hours
/// under a day, then days, each rounded down.
pub(super) fn relative_age(i18n: &I18n, age: Duration) -> String {
    let seconds = age.as_secs();
    let (key, n) = match seconds {
        0..60 => ("age.seconds", seconds),
        60..3_600 => ("age.minutes", seconds / 60),
        3_600..86_400 => ("age.hours", seconds / 3_600),
        _ => ("age.days", seconds / 86_400),
    };
    i18n.format(key, &[("n", &n.to_string())])
}

/// A capacity in bytes: GiB with one decimal from 1 GiB; below it MiB, without a decimal
/// when whole and with one otherwise.
fn capacity(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        mib(bytes)
    }
}

fn value<T>(reading: &Reading<T>) -> Option<&T> {
    match reading {
        Reading::Value(value) => Some(value),
        _ => None,
    }
}
/// The text a reading that is not a Value shows, as in the Status_Area. Callers handle
/// `Value` first; it maps to the same text as `Unavailable` only for exhaustiveness.
fn placeholder<T>(i18n: &I18n, reading: &Reading<T>) -> String {
    i18n.text(match reading {
        Reading::Loading => "dash.loading",
        Reading::NotQueried => "dash.not_queried",
        Reading::NotApplicable => "dash.na",
        Reading::Unavailable(_) | Reading::Value(_) => "dash.unknown",
    })
    .to_owned()
}
/// A state line: the label, a gap and the value.
fn line(i18n: &I18n, label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::raw(i18n.text(label).to_owned()),
        Span::raw("  "),
        Span::raw(value),
    ])
}
/// A state line for `reading`: `show` for a Value, otherwise its placeholder.
fn fact<T>(
    i18n: &I18n,
    label: &str,
    reading: &Reading<T>,
    show: impl FnOnce(&T) -> String,
) -> Line<'static> {
    let value = match reading {
        Reading::Value(value) => show(value),
        other => placeholder(i18n, other),
    };
    line(i18n, label, value)
}
/// On and off lead with a filled or hollow dot, as in the Status_Area.
fn lit(text: &str) -> String {
    format!("● {text}")
}
fn unlit(text: &str) -> String {
    format!("○ {text}")
}

fn disk_state(i18n: &I18n, disk: &Usage) -> String {
    format!(
        "{:.0}% · {} / {} · {} {}",
        disk.percent(),
        capacity(disk.used),
        capacity(disk.total),
        i18n.text("dash.available"),
        capacity(disk.total.saturating_sub(disk.used)),
    )
}

/// bbr.status and bbr.enable: `● Enabled v3 · fq` while BBR runs; otherwise
/// `○ Disabled · available v3` or `○ Disabled · unavailable`.
fn bbr(snapshot: &Snapshot, i18n: &I18n) -> (Vec<String>, Vec<Line<'static>>) {
    let local = &snapshot.local;
    let with_version = |text: String| match value(&local.bbr_version) {
        Some(version) => format!("{text} {version}"),
        None => text,
    };
    let support = match local.bbr_available {
        Reading::Value(true) => Some(with_version(i18n.text("dash.available").to_owned())),
        Reading::Value(false) => Some(i18n.text("tag.unavailable").to_owned()),
        _ => None,
    };
    let tag = match value(&local.congestion) {
        Some(algorithm) if algorithm == "bbr" => {
            std::iter::once(with_version(lit(i18n.text("status.enabled"))))
                .chain(value(&local.qdisc).cloned())
                .collect()
        }
        Some(_) => std::iter::once(unlit(i18n.text("status.disabled")))
            .chain(support.clone())
            .collect(),
        None => Vec::new(),
    };
    let state = vec![
        fact(i18n, "related.congestion", &local.congestion, String::clone),
        fact(i18n, "dash.qdisc", &local.qdisc, String::clone),
        fact(i18n, "dash.row_bbr", &local.bbr_available, |_| {
            support.unwrap_or_default()
        }),
    ];
    (tag, state)
}

fn mihomo(i18n: &I18n, state: &State) -> String {
    match state {
        State::Running(_) => lit(i18n.text("dash.running")),
        State::Stopped => unlit(i18n.text("dash.stopped")),
        State::NotInstalled => unlit(i18n.text("dash.not_installed")),
        State::Unknown => i18n.text("dash.unknown").to_owned(),
    }
}

fn never(_: &Parameters, _: &Snapshot) -> bool {
    false
}
/// system.timezone: the saved time zone is the one read.
fn same_timezone(parameters: &Parameters, snapshot: &Snapshot) -> bool {
    value(&snapshot.local.timezone)
        .is_some_and(|timezone| parameters.get("timezone") == Some(timezone))
}
/// bbr.enable: BBR already runs on the saved queue.
fn bbr_current(parameters: &Parameters, snapshot: &Snapshot) -> bool {
    let local = &snapshot.local;
    value(&local.congestion).is_some_and(|algorithm| algorithm == "bbr")
        && value(&local.qdisc).is_some_and(|qdisc| parameters.get("qdisc") == Some(qdisc))
}
/// tcp.tune: the buffer the saved bandwidth and RTT size against the effective memory is
/// the ceiling already set. `tcp_buffer` takes KiB and returns bytes, the unit of
/// `tcp_rmem_max`.
fn tcp_current(parameters: &Parameters, snapshot: &Snapshot) -> bool {
    let local = &snapshot.local;
    let (Some(max), Some(memory_kb)) = (
        value(&local.tcp_rmem_max),
        value(&local.effective_memory_kb),
    ) else {
        return false;
    };
    let (Some(bandwidth), Some(rtt)) = (parameters.get("bandwidth"), parameters.get("rtt")) else {
        return false;
    };
    memory::tcp_buffer(bandwidth, rtt, *memory_kb).is_ok_and(|buffer| buffer == *max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, tui::snapshot::fixture_snapshot};

    /// Changes the fixture readings for one case.
    type Change = fn(&mut Snapshot);
    /// Replaces a row's reading by the `usize`-th state that is not a Value.
    type Unset = fn(&mut Snapshot, usize);

    fn i18n(locale: &str) -> I18n {
        I18n::load(Some(locale), None).expect("bundled locale")
    }
    fn spec(id: &str) -> Spec {
        actions::find(id).expect("static action ID")
    }
    /// The fixture readings and the frame clock they are dated against.
    fn fixture() -> (Snapshot, Instant) {
        let now = Instant::now();
        (fixture_snapshot(now), now)
    }
    /// The Row_Tag's texts; `None` for an Action without a Related_Reading.
    fn tag(id: &str, snapshot: &Snapshot, i18n: &I18n, now: Instant) -> Option<Vec<String>> {
        related(&spec(id), snapshot, i18n, now)
            .map(|related| related.tag.into_iter().map(|s| s.text).collect())
    }
    fn state(id: &str, snapshot: &Snapshot, i18n: &I18n, now: Instant) -> Vec<String> {
        related(&spec(id), snapshot, i18n, now)
            .expect("a related action")
            .state
            .iter()
            .map(ToString::to_string)
            .collect()
    }
    fn matches(id: &str, parameters: &[(&str, &str)], snapshot: &Snapshot) -> bool {
        let (_, now) = fixture();
        let parameters = parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Parameters>();
        let related = related(&spec(id), snapshot, &i18n("en-US"), now).expect("related");
        (related.matches_current)(&parameters, snapshot)
    }

    #[test]
    fn optimization_summaries_keep_field_order_units_and_nonempty_values() {
        for (id, expected) in [
            ("system.update", ""),
            ("tcp.tune", "1000 Mbit/s · 50 ms"),
            ("swap.create", "2048 MiB"),
            ("bbr.enable", "fq"),
            ("ssh.key", ""),
            ("system.timezone", "Asia/Shanghai"),
        ] {
            let spec = spec(id);
            assert_eq!(summary(&spec, &spec.defaults()), expected, "{id}");
        }
        assert_eq!(
            summary(
                &spec("tcp.tune"),
                &Parameters::from([("rtt".into(), "0.125".into())])
            ),
            "1000 Mbit/s · 0.125 ms"
        );
        assert_eq!(
            summary(&spec("terminal.apply"), &spec("terminal.apply").defaults()),
            "bash · full · cyan · off · on"
        );
    }

    #[test]
    fn expected_changes_use_the_snapshot_and_operation_calculation() {
        let (mut snapshot, _) = fixture();
        let changes = |id: &str, snapshot: &Snapshot| {
            let spec = spec(id);
            expected(&spec, &spec.defaults(), snapshot)
                .into_iter()
                .map(|change| (change.label, change.from, change.to))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            changes("tcp.tune", &snapshot),
            vec![("related.tcp_buffer", "6 MiB".into(), "12 MiB".into())]
        );
        assert_eq!(
            changes("bbr.enable", &snapshot),
            vec![
                ("related.congestion", "cubic".into(), "bbr".into()),
                ("dash.qdisc", "fq_codel".into(), "fq".into()),
            ]
        );
        assert_eq!(
            changes("system.timezone", &snapshot),
            vec![(
                "dash.timezone",
                "Asia/Shanghai".into(),
                "Asia/Shanghai".into()
            )]
        );
        assert!(changes("system.clean", &snapshot).is_empty());
        snapshot.local.tcp_rmem_max = Reading::Value(6 * MIB + MIB / 2);
        assert_eq!(changes("tcp.tune", &snapshot)[0].1, "6.5 MiB");
        snapshot.local.tcp_rmem_max = Reading::Value(2 * GIB);
        assert_eq!(changes("tcp.tune", &snapshot)[0].1, "2048 MiB");
    }

    #[test]
    fn expected_changes_omit_missing_readings_and_invalid_calculations() {
        let (original, _) = fixture();
        let tcp = spec("tcp.tune");
        for missing in [
            Reading::Loading,
            Reading::NotQueried,
            Reading::NotApplicable,
            Reading::Unavailable("denied".into()),
        ] {
            let mut snapshot = original.clone();
            snapshot.local.tcp_rmem_max = missing.clone();
            assert!(expected(&tcp, &tcp.defaults(), &snapshot).is_empty());
            snapshot.local.tcp_rmem_max = original.local.tcp_rmem_max.clone();
            snapshot.local.effective_memory_kb = missing;
            assert!(expected(&tcp, &tcp.defaults(), &snapshot).is_empty());
        }
        let mut params = tcp.defaults();
        params.insert("bandwidth".into(), "invalid".into());
        assert!(expected(&tcp, &params, &original).is_empty());
        params.remove("bandwidth");
        assert!(expected(&tcp, &params, &original).is_empty());
        let mut snapshot = original.clone();
        snapshot.local.effective_memory_kb = Reading::Value(1);
        assert!(expected(&tcp, &tcp.defaults(), &snapshot).is_empty());
        snapshot.local.timezone = Reading::NotApplicable;
        assert!(
            expected(
                &spec("system.timezone"),
                &spec("system.timezone").defaults(),
                &snapshot
            )
            .is_empty()
        );
        let bbr = spec("bbr.enable");
        snapshot.local.congestion = Reading::Unavailable("denied".into());
        assert_eq!(expected(&bbr, &bbr.defaults(), &snapshot).len(), 1);
        snapshot.local.qdisc = Reading::NotQueried;
        assert!(expected(&bbr, &bbr.defaults(), &snapshot).is_empty());
        snapshot.local.congestion = Reading::Value("bbr".into());
        let remaining = expected(&bbr, &bbr.defaults(), &snapshot);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].from, remaining[0].to);
    }

    #[test]
    fn every_table_row_tags_the_fixture_readings_in_both_languages() {
        let (snapshot, now) = fixture();
        let rows: [(&[&str], &[&str], &[&str]); 8] = [
            (&["system.timezone"], &["Asia/Shanghai"], &["Asia/Shanghai"]),
            (
                &["swap.create", "swap.auto", "swap.remove"],
                &["已有 2.0 GiB"],
                &["existing 2.0 GiB"],
            ),
            (&["system.clean"], &["磁盘 41%"], &["Disk 41%"]),
            (
                &["bbr.status", "bbr.enable"],
                &["○ 未启用", "可用 v3"],
                &["○ Disabled", "available v3"],
            ),
            (&["tcp.tune"], &["○ 未调优"], &["○ not tuned"]),
            (&["system.public_ip"], &["12s 前"], &["12s ago"]),
            (
                &["clash.status", "clash.start", "clash.stop"],
                &["● 运行中"],
                &["● Running"],
            ),
            (&["clash.install"], &["v1.19.2"], &["v1.19.2"]),
        ];
        for (locale, column) in [("zh-CN", 1), ("en-US", 2)] {
            let i18n = i18n(locale);
            for row in rows {
                let (ids, expected) = (row.0, if column == 1 { row.1 } else { row.2 });
                let expected = expected.iter().map(|s| s.to_string()).collect::<Vec<_>>();
                for id in ids {
                    assert_eq!(
                        tag(id, &snapshot, &i18n, now),
                        Some(expected.clone()),
                        "{id} ({locale})"
                    );
                    assert!(!state(id, &snapshot, &i18n, now).is_empty(), "{id}");
                }
            }
            for id in ["system.info", "system.update", "bbr.restore", "tcp.restore"] {
                assert!(related(&spec(id), &snapshot, &i18n, now).is_none(), "{id}");
            }
        }
    }

    #[test]
    fn other_values_of_the_same_readings() {
        let (fixture, now) = fixture();
        let i18n = i18n("zh-CN");
        let changed = |id: &str, change: fn(&mut Snapshot)| {
            let mut snapshot = fixture.clone();
            change(&mut snapshot);
            tag(id, &snapshot, &i18n, now).expect("related")
        };
        let cases: [(&str, Change, &[&str]); 9] = [
            (
                "swap.create",
                |s| s.local.swap = Reading::Value(Usage { used: 0, total: 0 }),
                &["未启用"],
            ),
            (
                "bbr.enable",
                |s| {
                    s.local.congestion = Reading::Value("bbr".into());
                    s.local.qdisc = Reading::Value("fq".into());
                },
                &["● 已启用 v3", "fq"],
            ),
            (
                "bbr.status",
                |s| s.local.bbr_available = Reading::Value(false),
                &["○ 未启用", "不可用"],
            ),
            (
                "tcp.tune",
                |s| s.local.tcp_configured = Reading::Value(true),
                &["● 已调优", "6 MiB"],
            ),
            (
                "system.public_ip",
                |s| s.public = Default::default(),
                &["未查询"],
            ),
            (
                "clash.start",
                |s| s.local.mihomo_state = Reading::Value(State::Stopped),
                &["○ 已停止"],
            ),
            (
                "clash.status",
                |s| s.local.mihomo_state = Reading::Value(State::NotInstalled),
                &["○ 未安装"],
            ),
            (
                "clash.stop",
                |s| s.local.mihomo_state = Reading::Value(State::Unknown),
                &["未知"],
            ),
            (
                "clash.install",
                |s| {
                    s.local.mihomo_state = Reading::Value(State::NotInstalled);
                    s.local.mihomo_version = Reading::NotApplicable;
                },
                &["未安装"],
            ),
        ];
        for (id, change, expected) in cases {
            assert_eq!(changed(id, change), expected, "{id}");
        }
    }

    fn not_a_value<T>(kind: usize) -> Reading<T> {
        match kind {
            0 => Reading::Loading,
            1 => Reading::NotQueried,
            2 => Reading::Unavailable("probe failed".into()),
            _ => Reading::NotApplicable,
        }
    }

    #[test]
    fn readings_that_are_not_values_add_no_tag_and_show_their_placeholder() {
        let (fixture, now) = fixture();
        let i18n = i18n("en-US");
        // Each row's own reading, replaced by a state that is not a Value.
        let cases: [(&[&str], Unset); 7] = [
            (&["system.timezone"], |s, k| {
                s.local.timezone = not_a_value(k)
            }),
            (&["swap.create", "swap.auto", "swap.remove"], |s, k| {
                s.local.swap = not_a_value(k)
            }),
            (&["system.clean"], |s, k| s.local.disk = not_a_value(k)),
            (&["bbr.status", "bbr.enable"], |s, k| {
                s.local.congestion = not_a_value(k)
            }),
            (&["tcp.tune"], |s, k| {
                s.local.tcp_configured = not_a_value(k)
            }),
            (&["clash.status", "clash.start", "clash.stop"], |s, k| {
                s.local.mihomo_state = not_a_value(k)
            }),
            (&["clash.install"], |s, k| {
                s.local.mihomo_version = not_a_value(k)
            }),
        ];
        let placeholders = [
            "dash.loading",
            "dash.not_queried",
            "dash.unknown",
            "dash.na",
        ];
        for (ids, change) in cases {
            for (kind, key) in placeholders.iter().enumerate() {
                let mut snapshot = fixture.clone();
                change(&mut snapshot, kind);
                for id in ids {
                    assert_eq!(
                        tag(id, &snapshot, &i18n, now),
                        Some(Vec::new()),
                        "{id} {key}"
                    );
                    let placeholder = i18n.text(key);
                    assert!(
                        state(id, &snapshot, &i18n, now)
                            .iter()
                            .any(|line| line.contains(placeholder)),
                        "{id} shows {placeholder}"
                    );
                }
            }
        }
        // A public lookup in flight has no age yet.
        let mut snapshot = fixture.clone();
        snapshot.public.ipv4 = Reading::Loading;
        snapshot.public.ipv6 = Reading::Loading;
        snapshot.public.isp = Reading::Loading;
        snapshot.public.updated = None;
        assert_eq!(
            tag("system.public_ip", &snapshot, &i18n, now),
            Some(Vec::new())
        );
        assert!(
            state("system.public_ip", &snapshot, &i18n, now)
                .iter()
                .all(|line| line.contains(i18n.text("dash.loading")))
        );
    }

    #[test]
    fn the_current_value_rules() {
        let (fixture, _) = fixture();
        // system.timezone: the saved value is the one read.
        assert!(matches(
            "system.timezone",
            &[("timezone", "Asia/Shanghai")],
            &fixture
        ));
        assert!(!matches(
            "system.timezone",
            &[("timezone", "UTC")],
            &fixture
        ));
        let mut unread = fixture.clone();
        unread.local.timezone = Reading::Loading;
        assert!(!matches(
            "system.timezone",
            &[("timezone", "Asia/Shanghai")],
            &unread
        ));

        // bbr.enable: BBR runs, and on the saved queue.
        assert!(
            !matches("bbr.enable", &[("qdisc", "fq_codel")], &fixture),
            "cubic"
        );
        let mut bbr = fixture.clone();
        bbr.local.congestion = Reading::Value("bbr".into());
        assert!(matches("bbr.enable", &[("qdisc", "fq_codel")], &bbr));
        assert!(!matches("bbr.enable", &[("qdisc", "fq")], &bbr));
        let mut unread = bbr.clone();
        unread.local.qdisc = Reading::Unavailable("sysctl failed".into());
        assert!(!matches("bbr.enable", &[("qdisc", "fq_codel")], &unread));
        assert!(!matches("bbr.status", &[], &bbr), "bbr.status has no rule");

        // tcp.tune: 1000 Mbit/s over 50 ms sizes 12 MiB, while the fixture's ceiling is 6 MiB.
        let defaults = [("bandwidth", "1000"), ("rtt", "50")];
        assert!(!matches("tcp.tune", &defaults, &fixture));
        assert!(matches(
            "tcp.tune",
            &[("bandwidth", "500"), ("rtt", "50")],
            &fixture
        ));
        let mut tuned = fixture.clone();
        tuned.local.tcp_rmem_max = Reading::Value(12 * MIB);
        assert!(matches("tcp.tune", &defaults, &tuned));
        assert!(
            !matches("tcp.tune", &[("bandwidth", "fast"), ("rtt", "50")], &tuned),
            "tcp_buffer fails"
        );
        let mut unread = tuned.clone();
        unread.local.effective_memory_kb = Reading::NotApplicable;
        assert!(!matches("tcp.tune", &defaults, &unread));
        let mut unread = tuned;
        unread.local.tcp_rmem_max = Reading::Loading;
        assert!(!matches("tcp.tune", &defaults, &unread));

        // Rows without a rule never match.
        for id in [
            "swap.create",
            "system.clean",
            "system.public_ip",
            "clash.start",
        ] {
            assert!(!matches(id, &[], &fixture), "{id}");
        }
    }

    #[test]
    fn relative_age_changes_unit_at_each_boundary() {
        let cases = [
            (0, "0s 前", "0s ago"),
            (59, "59s 前", "59s ago"),
            (60, "1 分钟前", "1m ago"),
            (59 * 60, "59 分钟前", "59m ago"),
            (60 * 60, "1 小时前", "1h ago"),
            (23 * 3_600, "23 小时前", "23h ago"),
            (24 * 3_600, "1 天前", "1d ago"),
            (3 * 86_400 + 3_599, "3 天前", "3d ago"),
        ];
        let (zh, en) = (i18n("zh-CN"), i18n("en-US"));
        for (seconds, chinese, english) in cases {
            let age = Duration::from_secs(seconds);
            assert_eq!(relative_age(&zh, age), chinese, "{seconds}");
            assert_eq!(relative_age(&en, age), english, "{seconds}");
        }
        // Fractions of a second round down.
        assert_eq!(relative_age(&en, Duration::from_millis(59_999)), "59s ago");
    }

    #[test]
    fn capacities_use_gib_with_one_decimal_and_whole_mib_without() {
        assert_eq!(capacity(2 * GIB), "2.0 GiB");
        assert_eq!(capacity(1_992_294 * 1024), "1.9 GiB");
        assert_eq!(capacity(6_291_456), "6 MiB");
        assert_eq!(capacity(MIB + MIB / 2), "1.5 MiB");
        assert_eq!(capacity(1023 * MIB), "1023 MiB");
        let (snapshot, now) = fixture();
        let state = state("tcp.tune", &snapshot, &i18n("en-US"), now);
        assert!(
            state[1].ends_with("6 MiB") && state[2].ends_with("1.9 GiB"),
            "{state:?}"
        );
    }
}
