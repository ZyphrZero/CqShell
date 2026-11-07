use super::*;
use crate::{
    actions,
    runtime::{Event, Outcome},
    tui::{
        Confirmation, FormMode, InputBuffer, OPTIMIZATION_GROUP, RECOMMENDED,
        palette::CommandPalette,
        view::plan::{Progress, StepView},
    },
};
use std::{sync::mpsc, thread};

pub(super) fn confirmation(app: &App) -> Confirmation {
    let mut ctx = app.ctx.clone();
    ctx.yes = false;
    let (sender, receiver) = mpsc::channel();
    ctx.events = Some(sender);
    let jobs = RECOMMENDED
        .iter()
        .map(|id| {
            let spec = actions::find(id).unwrap();
            let params = spec.defaults();
            (spec, params)
        })
        .collect::<Vec<_>>();
    let worker = thread::spawn(move || actions::batch_report(&ctx, &jobs));
    let Event::Confirm { text, token, reply } =
        receiver.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("batch must request confirmation before running any operation");
    };
    reply.send(false).unwrap();
    assert_eq!(
        worker.join().unwrap().result.unwrap_err().key,
        "error.cancelled"
    );
    Confirmation {
        text,
        token,
        reply: mpsc::channel().0,
        input: InputBuffer::default(),
        scroll: 0,
        accept: false,
    }
}

#[test]
fn classic_stage2_state_matrix() {
    for locale in ["zh-CN", "en-US"] {
        for (width, height) in [(80, 24), (120, 36)] {
            for state in [
                "palette_bbr",
                "clean_chips",
                "recommended",
                "progress_2_of_4",
                "batch_confirm",
                "root_header",
            ] {
                let mut app = fixture_app(locale);
                match state {
                    "palette_bbr" => {
                        app.command_palette = Some(CommandPalette {
                            query: InputBuffer::new("bbr".into()),
                            ..CommandPalette::default()
                        })
                    }
                    "clean_chips" => {
                        app.form = Some(
                            app.make_form(actions::find("system.clean").unwrap(), FormMode::Run)
                                .unwrap(),
                        )
                    }
                    "recommended" => {
                        app.group = OPTIMIZATION_GROUP;
                        app.root = true;
                        app.recommend().unwrap();
                        app.toast = None;
                    }
                    "progress_2_of_4" => {
                        app.group = OPTIMIZATION_GROUP;
                        app.phase = crate::tui::Phase::Running;
                        app.marked = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
                        app.task_ids = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
                        app.progress = Some(Progress {
                            started: app.now - Duration::from_secs(25),
                            steps: vec![
                                StepView::Done(Duration::from_secs(18)),
                                StepView::Running(app.now - Duration::from_secs(7)),
                                StepView::Pending,
                                StepView::Pending,
                            ],
                        });
                        app.logs_open = true;
                        for (index, state, tone, bold) in [
                            (
                                0,
                                crate::runtime::StepState::Finished {
                                    outcome: Outcome::Done,
                                    elapsed: Duration::from_secs(18),
                                },
                                crate::tui::Tone::Success,
                                false,
                            ),
                            (
                                1,
                                crate::runtime::StepState::Started,
                                crate::tui::Tone::Info,
                                true,
                            ),
                        ] {
                            let step = crate::runtime::Step {
                                index,
                                total: 4,
                                id: RECOMMENDED[index],
                                state,
                            };
                            app.log_styled(app.ctx.step_line(&step).unwrap(), tone, bold);
                        }
                    }
                    "batch_confirm" => {
                        app.group = OPTIMIZATION_GROUP;
                        app.confirmation = Some(confirmation(&app));
                    }
                    "root_header" => app.root = true,
                    _ => unreachable!(),
                }
                let case = Case {
                    name: format!("classic_stage2_{state}_{width}x{height}_{locale}"),
                    theme: "classic".into(),
                    background: Background::Unavailable,
                    locale: locale.into(),
                };
                assert_snapshot(&case, &render(&app, width, height));
            }
        }
    }
}

#[test]
fn compact_progress_keeps_all_recommended_steps_and_skipped_statuses() {
    for locale in ["zh-CN", "en-US"] {
        let mut app = fixture_app(locale);
        app.group = OPTIMIZATION_GROUP;
        app.task_ids = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
        let mut progress = Progress::new(4, app.now);
        progress.steps[0] = StepView::Failed(Duration::from_secs(1));
        progress.finish(Outcome::Failed, app.now);
        app.progress = Some(progress);
        let buffer = render(&app, 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .replace(' ', "");
        for id in RECOMMENDED {
            assert!(
                text.contains(&app.ctx.i18n.text(&format!("action.{id}")).replace(' ', "")),
                "{locale} {id}"
            );
        }
        assert_eq!(
            text.matches(&app.ctx.i18n.text("ui.skipped").replace(' ', ""))
                .count(),
            3
        );
        assert!(text.contains("00:01"));
    }
}
