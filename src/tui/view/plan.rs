use super::{
    App, Frame, Level, Line, Paragraph, Rect, Regions, Role, Row, Span, Spec, Style, TAG_GAP,
    Theme, Tone, UnicodeWidthStr, clip_spans, clip_with, ellipsis, impact_lines, name_line,
    preview, related, task_marker, theme, wrap_words,
};
use crate::{
    actions::Parameters,
    runtime::{Outcome, Step, StepState},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum StepView {
    Pending,
    Running(Instant),
    Done(Duration),
    Failed(Duration),
    Cancelled(Duration),
    Skipped,
}

#[derive(Debug)]
pub(in crate::tui) struct Progress {
    pub started: Instant,
    pub steps: Vec<StepView>,
}

impl Progress {
    pub fn new(total: usize, started: Instant) -> Self {
        Self {
            started,
            steps: vec![StepView::Pending; total],
        }
    }

    pub fn apply(&mut self, step: &Step, now: Instant) {
        assert_eq!(step.total, self.steps.len());
        self.steps[step.index] = match step.state {
            StepState::Started => StepView::Running(now),
            StepState::Finished { outcome, elapsed } => StepView::finished(outcome, elapsed),
        };
    }

    pub fn finish(&mut self, outcome: Outcome, now: Instant) {
        for step in &mut self.steps {
            *step = match *step {
                StepView::Pending => StepView::Skipped,
                StepView::Running(started) => {
                    StepView::finished(outcome, now.saturating_duration_since(started))
                }
                other => other,
            };
        }
    }

    pub fn current(&self) -> Option<usize> {
        self.steps
            .iter()
            .position(|step| matches!(step, StepView::Running(_)))
    }
}

impl StepView {
    fn finished(outcome: Outcome, elapsed: Duration) -> Self {
        match outcome {
            Outcome::Done => Self::Done(elapsed),
            Outcome::Failed => Self::Failed(elapsed),
            Outcome::Cancelled => Self::Cancelled(elapsed),
        }
    }

    fn role(self) -> Role {
        match self {
            Self::Pending | Self::Skipped => Role::Text3,
            Self::Running(_) => Role::Info,
            Self::Done(_) => Role::Success,
            Self::Failed(_) => Role::Danger,
            Self::Cancelled(_) => Role::Warning,
        }
    }
}

pub(super) fn elapsed_text(elapsed: Duration) -> String {
    format!(
        "{:02}:{:02}",
        elapsed.as_secs() / 60,
        elapsed.as_secs() % 60
    )
}

fn step_line(app: &App, index: usize, state: StepView, width: usize) -> Line<'static> {
    let theme = &app.theme.active;
    let symbol = match state {
        StepView::Running(_) => app.spin(),
        StepView::Done(_) => theme::glyph(theme, theme::Glyph::Success),
        StepView::Failed(_) => theme::glyph(theme, theme::Glyph::Failure),
        StepView::Cancelled(_) => theme::glyph(theme, theme::Glyph::Cancelled),
        StepView::Pending => theme::glyph(theme, theme::Glyph::Pending),
        StepView::Skipped => theme::glyph(theme, theme::Glyph::Skipped),
    };
    let elapsed = match state {
        StepView::Running(started) => Some(app.now.saturating_duration_since(started)),
        StepView::Done(elapsed) | StepView::Failed(elapsed) | StepView::Cancelled(elapsed) => {
            Some(elapsed)
        }
        _ => None,
    };
    let suffix = elapsed.map(elapsed_text).unwrap_or_else(|| {
        if state == StepView::Skipped {
            app.ctx.i18n.text("ui.skipped").to_owned()
        } else {
            String::new()
        }
    });
    let prefix = format!("{symbol} {} ", index + 1);
    let name = app
        .ctx
        .i18n
        .text(&format!("action.{}", app.task_ids[index]))
        .to_owned();
    let available = width.saturating_sub(prefix.width() + suffix.width() + 1);
    let name = clip_with(&name, available, ellipsis(theme));
    let gap = width.saturating_sub(prefix.width() + name.width() + suffix.width());
    let restricted =
        theme::unframed(theme) || theme.patterns.glyphs == crate::theme::GlyphSet::Ascii;
    let mut spans = if restricted {
        vec![
            Span::styled(symbol, theme::fg(theme, state.role())),
            Span::styled(
                format!(" {} ", index + 1),
                theme::text(theme, Level::Secondary),
            ),
        ]
    } else {
        vec![Span::styled(prefix, theme::fg(theme, state.role()))]
    };
    spans.extend([
        Span::styled(name, theme::text(theme, Level::Primary)),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            suffix,
            if restricted {
                theme::text(theme, Level::Secondary)
            } else {
                theme::fg(theme, state.role())
            },
        ),
    ]);
    Line::from(spans)
}

fn progress_bar(app: &App, progress: &Progress, width: usize) -> Line<'static> {
    let total = progress.steps.len();
    if total == 0 {
        return Line::default();
    }
    let room = width.saturating_sub(total.saturating_sub(1));
    let mut spans = Vec::new();
    for (index, step) in progress.steps.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let size = (index + 1) * room / total - index * room / total;
        spans.extend(theme::bar(&app.theme.active, 100.0, size, step.role()));
    }
    Line::from(spans)
}

pub(super) fn draw_progress(app: &App, frame: &mut Frame, regions: &Regions) {
    let progress = app.progress.as_ref().expect("active batch progress");
    let theme = &app.theme.active;
    let mut block = theme::panel(theme, app.ctx.i18n.text("ui.progress_title"), false);
    if theme::unframed(theme) {
        block = block.padding(theme::panel_padding(theme, regions.details.width));
    }
    if regions.stacked && !theme::unframed(theme) {
        block = block.border_set(theme::joined_border_set(theme));
    }
    let inner = block.inner(regions.details);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    if regions.stacked
        && let Row::Action(spec) = regions.selected_row(app)
    {
        lines.push(name_line(app, spec, width));
        lines.extend(impact_lines(app, spec, width));
    }
    let page = (inner.height as usize).saturating_sub(lines.len() + 1);
    let selected = progress.current().unwrap_or_else(|| {
        progress
            .steps
            .iter()
            .rposition(|step| !matches!(step, StepView::Pending | StepView::Skipped))
            .unwrap_or(0)
    });
    let offset = super::list_offset(selected, progress.steps.len(), page);
    lines.extend(
        progress
            .steps
            .iter()
            .copied()
            .enumerate()
            .skip(offset)
            .take(page)
            .map(|(index, step)| step_line(app, index, step, width)),
    );
    if progress.steps.len() > page {
        block = block.title(
            Line::styled(
                format!(
                    " {}–{}/{} ",
                    offset + 1,
                    (offset + page).min(progress.steps.len()),
                    progress.steps.len()
                ),
                theme::muted(theme),
            )
            .right_aligned(),
        );
    }
    frame.render_widget(block, regions.details);
    frame.render_widget(Paragraph::new(fit(lines, theme, width)), inner);
    if inner.height > 0 {
        frame.render_widget(
            Paragraph::new(progress_bar(app, progress, width)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

fn summary(app: &App, spec: &Spec, params: &Parameters) -> Span<'static> {
    let theme = &app.theme.active;
    let i18n = &app.ctx.i18n;
    if let Some(field) = spec.fields.iter().find(|field| {
        field.required
            && params
                .get(field.name)
                .map_or(field.default, String::as_str)
                .is_empty()
    }) {
        return Span::styled(
            i18n.format(
                "ui.missing_required",
                &[("field", i18n.text(&format!("field_short.{}", field.name)))],
            ),
            theme::tone(theme, Tone::Warning),
        );
    }
    let mut text = related::summary(spec, params);
    if related::related(spec, &app.snapshot, i18n, app.now)
        .is_some_and(|state| (state.matches_current)(params, &app.snapshot))
    {
        text.push(' ');
        text.push_str(i18n.text("ui.current_equal"));
    }
    Span::styled(text, theme::text(theme, Level::Secondary))
}

pub(super) fn action_line(app: &App, spec: &Spec, width: usize) -> Line<'static> {
    let theme = &app.theme.active;
    let marked = app.marked.contains(spec.id);
    let checkbox = theme::glyph(
        theme,
        if marked {
            theme::Glyph::Checked
        } else {
            theme::Glyph::Unchecked
        },
    );
    let mut spans = vec![Span::styled(
        format!("{checkbox} "),
        theme::tone(theme, if marked { Tone::Accent } else { Tone::Muted }),
    )];
    let marker = task_marker(app, spec);
    let room = width.saturating_sub(checkbox.width() + 1 + marker.as_ref().map_or(0, Span::width));
    let mut name = app.ctx.i18n.text(&spec.label()).to_owned();
    if !spec.fields.is_empty() {
        name.push_str(theme::glyph(theme, theme::Glyph::Ellipsis));
    }
    if spec.external {
        name.push(' ');
        name.push_str(theme::glyph(theme, theme::Glyph::External));
    }
    let params = app.saved_parameters(spec);
    let summary = summary(app, spec, &params);
    let summary_width = summary.width().min(room.saturating_sub(TAG_GAP + 8));
    let gap = if summary_width > 0 { TAG_GAP } else { 0 };
    let name = clip_with(
        &name,
        room.saturating_sub(summary_width + gap),
        ellipsis(theme),
    );
    let style = if app.available(spec).is_ok() {
        Style::default()
    } else {
        theme::muted(theme)
    };
    spans.push(Span::styled(name, style));
    spans.extend(marker);
    if summary_width > 0 {
        let used = spans.iter().map(Span::width).sum::<usize>();
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + summary_width)),
        ));
        spans.extend(clip_spans(vec![summary], summary_width, ellipsis(theme)));
    }
    Line::from(clip_spans(spans, width, ellipsis(theme)))
}

fn change_line(app: &App, change: related::Change) -> Line<'static> {
    let theme = &app.theme.active;
    let i18n = &app.ctx.i18n;
    let values = if change.from == change.to {
        format!("{} {}", change.to, i18n.text("ui.unchanged"))
    } else {
        let from = if change.from.ends_with(" MiB") && change.to.ends_with(" MiB") {
            change.from.trim_end_matches(" MiB")
        } else {
            &change.from
        };
        let arrow = if theme.patterns.glyphs == crate::theme::GlyphSet::Ascii {
            "->"
        } else {
            "→"
        };
        format!("{from} {arrow} {}", change.to)
    };
    Line::from(vec![
        Span::styled(
            format!("  {} ", i18n.text(change.label)),
            theme::muted(theme),
        ),
        Span::styled(values, theme::fg(theme, Role::Info)),
    ])
}

fn impacts(app: &App, jobs: &[(Spec, Parameters)], width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let i18n = &app.ctx.i18n;
    let mut labels = Vec::new();
    if jobs.iter().any(|(spec, _)| spec.changes) {
        labels.push(i18n.text("ui.mutating").to_owned());
    }
    if jobs.iter().any(|(spec, _)| spec.root) {
        labels.push(i18n.text("ui.root").to_owned());
    }
    if jobs.iter().any(|(spec, _)| spec.external) {
        labels.push(format!(
            "{} {}",
            i18n.text("ui.takes_terminal"),
            theme::glyph(theme, theme::Glyph::External)
        ));
    }
    wrap_words(&labels.join(" · "), width)
        .into_iter()
        .map(|line| Line::styled(line, theme::tone(theme, Tone::Warning)))
        .collect()
}

fn command(app: &App, jobs: &[(Spec, Parameters)], width: usize) -> Vec<Line<'static>> {
    let words = preview::batch_words(jobs);
    let theme = &app.theme.active;
    let style = if preview::incomplete(&words) {
        theme::text(theme, Level::Tertiary)
    } else {
        theme::fg(theme, Role::Accent)
    };
    let shown = words.iter().map(preview::Word::shown).collect::<Vec<_>>();
    preview::wrap(&shown, width)
        .into_iter()
        .map(|line| Line::styled(line, style))
        .collect()
}

fn body(app: &App, jobs: &[(Spec, Parameters)], width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let i18n = &app.ctx.i18n;
    if jobs.is_empty() {
        return wrap_words(i18n.text("ui.plan_empty"), width)
            .into_iter()
            .map(|line| Line::styled(line, theme::muted(theme)))
            .collect();
    }
    let mut lines = Vec::new();
    for (index, (spec, params)) in jobs.iter().enumerate() {
        let detail = summary(app, spec, params);
        let mut spans = vec![Span::styled(
            format!("{}. {}", index + 1, i18n.text(&spec.label())),
            theme::bold(),
        )];
        if detail.width() > 0 {
            spans.push(Span::raw(" · "));
            spans.push(detail);
        }
        lines.push(Line::from(spans));
        lines.extend(
            related::expected(spec, params, &app.snapshot)
                .into_iter()
                .map(|change| change_line(app, change)),
        );
    }
    lines.extend(impacts(app, jobs, width));
    lines.extend(
        wrap_words(i18n.text("ui.plan_once"), width)
            .into_iter()
            .map(|line| Line::styled(line, theme::muted(theme))),
    );
    lines.extend(command(app, jobs, width));
    lines
}

fn footer(app: &App, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme.active;
    let i18n = &app.ctx.i18n;
    let hints = [
        ("X", i18n.text("key.run_checklist")),
        ("A", i18n.text("key.recommended")),
    ];
    let needed = hints
        .iter()
        .map(|(key, text)| key.width() + 1 + text.width())
        .sum::<usize>()
        + 2;
    if needed <= width {
        vec![theme::key_hints(theme, &hints, None, width)]
    } else {
        hints
            .into_iter()
            .map(|hint| theme::key_hints(theme, &[], Some(hint), width))
            .collect()
    }
}

pub(super) fn draw(app: &App, frame: &mut Frame, regions: &Regions) {
    let theme = &app.theme.active;
    let mut block = theme::panel(theme, app.ctx.i18n.text("ui.plan_title"), false);
    if regions.stacked && !theme::unframed(theme) {
        block = block.border_set(theme::joined_border_set(theme));
    }
    let jobs = app.checklist_from_rows(&regions.rows);
    let mut inner = block.inner(regions.details);
    let mut lines = body(app, &jobs, inner.width.saturating_sub(2) as usize);
    let mut hints = footer(app, inner.width.saturating_sub(2) as usize);
    if lines.len() + hints.len() <= inner.height as usize {
        block = block.padding(theme::panel_padding(theme, regions.details.width));
        inner = block.inner(regions.details);
    } else {
        lines = body(app, &jobs, inner.width as usize);
        hints = footer(app, inner.width as usize);
    }
    let width = inner.width as usize;
    let room = (inner.height as usize).saturating_sub(hints.len());
    if regions.stacked
        && let Row::Action(spec) = regions.selected_row(app)
    {
        let mut compact = vec![name_line(app, spec, width)];
        compact.extend(impact_lines(app, spec, width));
        compact.extend(
            related::expected(spec, &app.saved_parameters(spec), &app.snapshot)
                .into_iter()
                .map(|change| change_line(app, change)),
        );
        compact.extend(lines);
        lines = compact;
    }
    frame.render_widget(block, regions.details);
    lines.truncate(room);
    frame.render_widget(
        Paragraph::new(fit(lines, theme, width)),
        Rect {
            height: room as u16,
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(fit(hints, theme, width)),
        Rect {
            y: inner.y + room as u16,
            height: inner.height.saturating_sub(room as u16),
            ..inner
        },
    );
}

fn fit(lines: Vec<Line<'static>>, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| Line::from(clip_spans(line.spans, width, ellipsis(theme))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        actions,
        dashboard::Reading,
        tui::{
            OPTIMIZATION_GROUP, RECOMMENDED,
            snapshot::{self, Background, Case},
        },
    };
    use ratatui::{Terminal, backend::TestBackend};

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]
        // Feature: tui-redesign, Property 9: 进度不变式
        #[test]
        fn progress_invariants(
            outcomes in proptest::collection::vec((0u8..3, 0u64..120_000), 1..40),
            stop in 0usize..40,
        ) {
            let started = Instant::now();
            let mut progress = Progress::new(outcomes.len(), started);
            let mut now = started;
            let count = stop % (outcomes.len() + 1);
            for (index, (outcome, millis)) in outcomes.iter().take(count).enumerate() {
                let elapsed = Duration::from_millis(*millis);
                let outcome = [Outcome::Done, Outcome::Failed, Outcome::Cancelled][*outcome as usize];
                for state in [StepState::Started, StepState::Finished { outcome, elapsed }] {
                    progress.apply(&Step { index, total: outcomes.len(), id: "tcp.tune", state }, now);
                    let mut counts = [0usize; 6];
                    for step in &progress.steps {
                        counts[match step {
                            StepView::Pending => 0,
                            StepView::Running(_) => 1,
                            StepView::Done(_) => 2,
                            StepView::Failed(_) => 3,
                            StepView::Cancelled(_) => 4,
                            StepView::Skipped => 5,
                        }] += 1;
                    }
                    proptest::prop_assert_eq!(counts.iter().sum::<usize>(), outcomes.len());
                    proptest::prop_assert!(counts[1] <= 1);
                    now += elapsed;
                }
            }
            progress.finish(Outcome::Failed, now);
            proptest::prop_assert_eq!(progress.steps.len(), outcomes.len());
            proptest::prop_assert!(progress.steps.iter().all(|step| !matches!(step, StepView::Pending | StepView::Running(_))));
            proptest::prop_assert!(progress.steps[count..].iter().all(|step| *step == StepView::Skipped));
        }
    }

    #[test]
    fn finishing_an_interrupted_worker_ends_the_running_step() {
        let now = Instant::now();
        for outcome in [Outcome::Failed, Outcome::Cancelled] {
            let mut progress = Progress::new(2, now);
            progress.apply(
                &Step {
                    index: 0,
                    total: 2,
                    id: "tcp.tune",
                    state: StepState::Started,
                },
                now,
            );
            progress.finish(outcome, now + Duration::from_secs(3));
            assert_eq!(
                progress.steps,
                [
                    StepView::finished(outcome, Duration::from_secs(3)),
                    StepView::Skipped
                ]
            );
        }
    }

    #[test]
    fn progress_rows_and_segments_keep_each_state_and_elapsed_time() {
        let mut app = plan_app("zh-CN", &[]);
        app.task_ids = [
            "system.update",
            "system.clean",
            "tcp.tune",
            "bbr.enable",
            "ssh.port",
            "swap.create",
        ]
        .map(str::to_owned)
        .to_vec();
        let progress = Progress {
            started: app.now - Duration::from_secs(25),
            steps: vec![
                StepView::Done(Duration::from_secs(18)),
                StepView::Running(app.now - Duration::from_secs(7)),
                StepView::Pending,
                StepView::Failed(Duration::from_secs(65)),
                StepView::Cancelled(Duration::from_secs(1)),
                StepView::Skipped,
            ],
        };
        for (index, (state, expected)) in progress
            .steps
            .iter()
            .zip(["00:18", "00:07", "○", "01:05", "00:01", "未执行"])
            .enumerate()
        {
            let line = step_line(&app, index, *state, 56);
            assert!(line.to_string().contains(expected), "{line}");
            assert!(line.width() <= 56);
            assert_eq!(
                line.spans[0].style.fg,
                Some(theme::color(&app.theme.active, state.role()))
            );
        }
        let bar = progress_bar(&app, &progress, 56);
        assert_eq!(bar.width(), 56);
        let colors = bar
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .filter_map(|span| span.style.fg)
            .collect::<Vec<_>>();
        for state in &progress.steps {
            assert!(colors.contains(&theme::color(&app.theme.active, state.role())));
        }
    }

    fn plan_app(locale: &str, ids: &[&str]) -> App {
        let mut app = snapshot::fixture_app(locale);
        app.group = OPTIMIZATION_GROUP;
        app.marked = ids.iter().map(|id| (*id).to_owned()).collect();
        app
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn region_text(buffer: &ratatui::buffer::Buffer, area: Rect) -> String {
        (area.top()..area.bottom())
            .map(|row| {
                let mut text = String::new();
                let mut column = area.left();
                while column < area.right() {
                    let symbol = buffer[(column, row)].symbol();
                    text.push_str(symbol);
                    column += symbol.width().max(1) as u16;
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn optimization_summary_marks_current_values_and_missing_required_fields() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = plan_app(locale, &[]);
            let timezone = actions::find("system.timezone").unwrap();
            let shown = summary(&app, &timezone, &timezone.defaults());
            assert!(
                shown
                    .content
                    .ends_with(app.ctx.i18n.text("ui.current_equal"))
            );
            app.snapshot.local.timezone = Reading::NotApplicable;
            assert_eq!(
                summary(&app, &timezone, &timezone.defaults()).content,
                "Asia/Shanghai"
            );
            let tcp = actions::find("tcp.tune").unwrap();
            app.snapshot.local.tcp_rmem_max = Reading::Value(12 << 20);
            assert!(
                summary(&app, &tcp, &tcp.defaults())
                    .content
                    .ends_with(app.ctx.i18n.text("ui.current_equal"))
            );
            let bbr = actions::find("bbr.enable").unwrap();
            app.snapshot.local.congestion = Reading::Value("bbr".into());
            app.snapshot.local.qdisc = Reading::Value("fq".into());
            assert!(
                summary(&app, &bbr, &bbr.defaults())
                    .content
                    .ends_with(app.ctx.i18n.text("ui.current_equal"))
            );
            for id in actions::OPTIMIZATION {
                let spec = actions::find(id).unwrap();
                for field in spec
                    .fields
                    .iter()
                    .filter(|field| field.required && field.default.is_empty())
                {
                    let key = format!("field_short.{}", field.name);
                    assert_ne!(app.ctx.i18n.text(&key), key);
                    let missing = summary(&app, &spec, &spec.defaults());
                    assert_eq!(
                        missing.style.fg,
                        Some(theme::color(&app.theme.active, Role::Warning))
                    );
                    assert_eq!(
                        missing.content,
                        app.ctx
                            .i18n
                            .format("ui.missing_required", &[("field", app.ctx.i18n.text(&key))])
                    );
                }
            }
        }
    }

    #[test]
    fn optimization_rows_keep_summaries_and_obey_width_and_theme_glyphs() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = plan_app(locale, RECOMMENDED);
            for id in actions::OPTIMIZATION {
                let spec = actions::find(id).unwrap();
                let row = action_line(&app, &spec, 120).to_string();
                let expected = summary(&app, &spec, &app.saved_parameters(&spec));
                assert!(row.ends_with(expected.content.as_ref()), "{row:?}");
                for width in 0..81 {
                    assert!(
                        action_line(&app, &spec, width).width() <= width,
                        "{locale} {id} {width}"
                    );
                }
            }
            let bbr = actions::find("bbr.enable").unwrap();
            assert!(action_line(&app, &bbr, 80).to_string().starts_with("[✓] "));
            app.theme.active.patterns.glyphs = crate::theme::GlyphSet::Ascii;
            assert!(action_line(&app, &bbr, 80).to_string().starts_with("[x] "));
            assert_eq!(theme::glyph(&app.theme.active, theme::Glyph::Execute), ">");
        }
    }

    #[test]
    fn plan_lists_execution_order_changes_impacts_and_equivalent_command() {
        for locale in ["zh-CN", "en-US"] {
            let app = plan_app(locale, RECOMMENDED);
            let shown = text(&body(&app, &app.checklist(), 100));
            let mut previous = 0;
            for (index, id) in RECOMMENDED.iter().enumerate() {
                let name = format!(
                    "{}. {}",
                    index + 1,
                    app.ctx.i18n.text(&format!("action.{id}"))
                );
                let position = shown.find(&name).unwrap();
                assert!(position >= previous);
                previous = position;
            }
            for value in [
                "1000 Mbit/s · 50 ms",
                "6 → 12 MiB",
                "cubic → bbr",
                "fq_codel → fq",
                "sudo cqs batch system.update system.clean tcp.tune bbr.enable",
            ] {
                assert!(shown.contains(value), "{locale}: {shown}");
            }
            for key in ["ui.mutating", "ui.root", "ui.plan_once"] {
                assert!(shown.contains(app.ctx.i18n.text(key)), "{locale}: {shown}");
            }
            let buffer = snapshot::render(&app, 120, 36);
            let regions = Regions::new(buffer.area, &app);
            let header = region_text(&buffer, regions.header);
            assert!(header.contains('4'), "{header}");
            let panel = region_text(&buffer, regions.details);
            assert!(panel.contains("6 → 12 MiB"), "{panel}");
            assert!(panel.contains("fq_codel → fq"), "{panel}");
        }
    }

    #[test]
    fn unchanged_values_and_incomplete_commands_are_explicit() {
        let app = plan_app("zh-CN", &["system.timezone"]);
        let shown = text(&body(&app, &app.checklist(), 100));
        assert!(shown.contains("Asia/Shanghai 不变"), "{shown}");
        assert!(!shown.contains('→'), "{shown}");
        let app = plan_app("en-US", &["ssh.key"]);
        let command = command(&app, &app.checklist(), 120);
        assert!(text(&command).contains("ssh.key.public_key=<public_key>"));
        assert!(
            command
                .iter()
                .all(|line| line.style.fg == Some(theme::color(&app.theme.active, Role::Text3)))
        );
    }

    #[test]
    fn selected_missing_required_summary_keeps_the_warning_color() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = plan_app(locale, &[]);
            app.root = true;
            app.positions[OPTIMIZATION_GROUP] = actions::OPTIMIZATION
                .iter()
                .position(|id| *id == "ssh.key")
                .unwrap();
            for (width, height) in [(80, 24), (120, 36)] {
                let buffer = snapshot::render(&app, width, height);
                let regions = Regions::new(buffer.area, &app);
                let row = crate::tui::row_index(&app.rows(app.group), app.selected());
                let row = regions.action_rows.y + (row - regions.action_offset) as u16;
                let warning = (regions.action_rows.left()..regions.action_rows.right())
                    .find(|column| buffer[(*column, row)].symbol() == "!")
                    .expect("missing public key warning");
                assert_eq!(
                    buffer[(warning, row)].fg,
                    theme::color(&app.theme.active, Role::Warning)
                );
            }
        }
    }

    #[test]
    fn empty_plan_and_footer_hints_are_visible_in_both_sizes_and_languages() {
        for locale in ["zh-CN", "en-US"] {
            let app = plan_app(locale, &[]);
            for (width, height) in [(80, 24), (120, 36)] {
                let buffer = snapshot::render(&app, width, height);
                let regions = Regions::new(buffer.area, &app);
                let shown = region_text(&buffer, regions.details);
                for key in ["key.run_checklist", "key.recommended"] {
                    assert!(
                        shown.contains(app.ctx.i18n.text(key)),
                        "{locale} {width}: {shown}"
                    );
                }
                assert!(shown.contains("Space") && shown.contains('A'), "{shown}");
                assert!(
                    shown.contains(app.ctx.i18n.text("ui.plan_title")),
                    "{shown}"
                );
            }
        }
    }

    #[test]
    fn plan_rendering_stays_inside_its_rectangle() {
        for locale in ["zh-CN", "en-US"] {
            let mut app = plan_app(locale, actions::OPTIMIZATION);
            for (width, height) in [(80, 24), (111, 24), (112, 24), (120, 36), (200, 60)] {
                let area = Rect::new(0, 0, width, height);
                let regions = Regions::new(area, &app);
                app.parameters.insert(
                    "system.timezone".into(),
                    Parameters::from([("timezone".into(), "洲/地区".repeat(100))]),
                );
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| draw(&app, frame, &regions)).unwrap();
                let buffer = terminal.backend().buffer();
                for row in 0..height {
                    for column in 0..width {
                        if !regions.details.contains((column, row).into()) {
                            assert_eq!(buffer[(column, row)], ratatui::buffer::Cell::default());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn compact_plan_keeps_the_selected_name_impact_and_empty_checklist_hint() {
        for locale in ["zh-CN", "en-US"] {
            for ids in [&[][..], RECOMMENDED] {
                let app = plan_app(locale, ids);
                let buffer = snapshot::render(&app, 80, 24);
                let regions = Regions::new(buffer.area, &app);
                let panel = region_text(&buffer, regions.details);
                for key in [
                    "action.system.update",
                    "ui.mutating",
                    "ui.root",
                    "key.run_checklist",
                    "key.recommended",
                ] {
                    assert!(panel.contains(app.ctx.i18n.text(key)), "{locale}: {panel}");
                }
                if ids.is_empty() {
                    assert!(
                        panel.contains(app.ctx.i18n.text("ui.plan_empty")),
                        "{panel}"
                    );
                }
            }
        }
    }

    #[test]
    fn classic_plan_snapshots_keep_the_original_baselines_separate() {
        for locale in ["zh-CN", "en-US"] {
            for (width, height) in [(80, 24), (120, 36)] {
                for (state, ids) in [("empty", &[][..]), ("recommended", RECOMMENDED)] {
                    let app = plan_app(locale, ids);
                    let case = Case {
                        name: format!("classic_stage2_plan_{state}_{width}x{height}_{locale}"),
                        theme: "classic".into(),
                        background: Background::Unavailable,
                        locale: locale.into(),
                    };
                    snapshot::assert_snapshot(&case, &snapshot::render(&app, width, height));
                }
            }
        }
    }
}
