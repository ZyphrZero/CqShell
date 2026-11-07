use super::*;
use view::plan::StepView;

fn batch(ids: &[&str]) -> Task {
    Task::Batch(
        ids.iter()
            .map(|id| {
                let spec = actions::find(id).unwrap();
                let params = spec.defaults();
                (spec, params)
            })
            .collect(),
    )
}

fn app() -> App {
    let mut app = snapshot::fixture_app("en-US");
    app.platform_error = None;
    app.root = true;
    app.group = OPTIMIZATION_GROUP;
    app
}

#[test]
fn live_steps_update_progress_records_and_semantic_logs() {
    let mut app = app();
    app.record_start(&batch(RECOMMENDED));
    app.phase = Phase::Running;
    let (sender, receiver) = mpsc::channel();
    app.receiver = Some(receiver);
    for (index, outcome) in [Outcome::Done, Outcome::Failed, Outcome::Cancelled]
        .into_iter()
        .enumerate()
    {
        let id = RECOMMENDED[index];
        sender
            .send(Event::Step(Step {
                index,
                total: 4,
                id,
                state: StepState::Started,
            }))
            .unwrap();
        app.receive();
        assert_eq!(app.progress.as_ref().unwrap().current(), Some(index));
        let line = app.logs.back().unwrap();
        assert_eq!(line.tone, Tone::Info);
        assert!(line.bold);
        let elapsed = Duration::from_secs(index as u64 + 1);
        sender
            .send(Event::Step(Step {
                index,
                total: 4,
                id,
                state: StepState::Finished { outcome, elapsed },
            }))
            .unwrap();
        app.receive();
        assert_eq!(app.records.last[id].outcome, outcome);
        assert_eq!(app.records.last[id].elapsed, elapsed);
        if outcome == Outcome::Done {
            assert_eq!(app.logs.back().unwrap().tone, Tone::Success);
            assert!(!app.logs.back().unwrap().bold);
        }
    }
    sender
        .send(Event::Finished(Err(Error::new("error.cancelled", ""))))
        .unwrap();
    app.receive();
    assert_eq!(
        app.progress.as_ref().unwrap().steps,
        [
            StepView::Done(Duration::from_secs(1)),
            StepView::Failed(Duration::from_secs(2)),
            StepView::Cancelled(Duration::from_secs(3)),
            StepView::Skipped,
        ]
    );
    assert_eq!(
        app.records.recent,
        ["tcp.tune", "system.clean", "system.update"]
    );
    assert!(!app.records.last.contains_key("bbr.enable"));
}

#[test]
fn terminal_handoff_report_replays_steps_and_skips_unstarted_jobs() {
    let mut app = app();
    app.record_start(&batch(RECOMMENDED));
    let report = actions::BatchReport {
        steps: RECOMMENDED
            .iter()
            .enumerate()
            .map(|(index, id)| actions::StepRecord {
                id,
                outcome: (index < 2).then_some(if index == 0 {
                    Outcome::Done
                } else {
                    Outcome::Cancelled
                }),
                elapsed: (index < 2).then_some(Duration::from_secs(index as u64 + 1)),
            })
            .collect(),
        result: Err(Error::new("error.cancelled", "")),
    };
    app.record_report(&report);
    app.finish(report.result);
    assert_eq!(
        app.progress.as_ref().unwrap().steps,
        [
            StepView::Done(Duration::from_secs(1)),
            StepView::Cancelled(Duration::from_secs(2)),
            StepView::Skipped,
            StepView::Skipped
        ]
    );
    assert_eq!(app.records.recent, ["system.clean", "system.update"]);
    assert_eq!(app.records.last.len(), 2);
}

#[test]
fn progress_survives_navigation_but_not_changed_checklists_or_a_new_task() {
    let mut app = app();
    app.recommend().unwrap();
    app.record_start(&batch(RECOMMENDED));
    app.finish(Ok(()));
    app.cycle_category(true);
    assert!(app.progress.is_some());
    app.group = OPTIMIZATION_GROUP;
    app.recommend().unwrap();
    assert!(app.progress.is_some());
    app.toggle_selected().unwrap();
    assert!(app.progress.is_none());
    app.record_start(&batch(RECOMMENDED));
    let clean = actions::find("system.clean").unwrap();
    app.form = Some(
        app.make_form(clean.clone(), FormMode::Optimization)
            .unwrap(),
    );
    app.form.as_mut().unwrap().values[0].replace("full".into());
    app.submit_form().unwrap();
    assert!(app.progress.is_none());
    app.record_start(&batch(RECOMMENDED));
    app.record_start(&Task::Single {
        spec: clean.clone(),
        params: clean.defaults(),
        reviewed: false,
    });
    assert!(app.progress.is_none());
}
