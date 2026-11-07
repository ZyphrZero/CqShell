use super::*;
use crate::{
    theme::{self, builtin},
    tui::{
        FormMode, InputBuffer, OPTIMIZATION_GROUP, RECOMMENDED,
        palette::CommandPalette,
        view::plan::{Progress, StepView},
    },
};

fn record(app: &App, name: String, width: u16, height: u16) {
    let background = match &app.theme.background {
        theme::Background::Detected((red, green, blue)) => {
            Background::Detected(*red, *green, *blue)
        }
        theme::Background::Disabled => Background::Disabled,
        theme::Background::Unavailable(_) => Background::Unavailable,
    };
    let case = Case {
        name,
        theme: app.theme.active.id.clone(),
        background,
        locale: app.ctx.i18n.locale().into(),
    };
    assert_snapshot(&case, &render(app, width, height));
}

#[test]
fn final_main_matrix_44_cases() {
    let mut variants = vec![
        (
            "aurora-dark",
            "aurora",
            theme::Background::Detected((11, 17, 25)),
        ),
        (
            "aurora-light",
            "aurora",
            theme::Background::Detected((246, 247, 249)),
        ),
        (
            "aurora-unavailable",
            "aurora",
            theme::Background::Unavailable("fixture".into()),
        ),
        (
            "classic-dark",
            "classic",
            theme::Background::Unavailable("fixture".into()),
        ),
        (
            "classic-light",
            "classic",
            theme::Background::Detected((255, 255, 255)),
        ),
    ];
    for id in ["phosphor", "neon", "nord", "sand", "high-contrast", "plain"] {
        variants.push((id, id, theme::Background::Unavailable("fixture".into())));
    }
    let mut count = 0;
    for (variant, id, background) in variants {
        for locale in ["zh-CN", "en-US"] {
            for (width, height) in [(80, 24), (120, 36)] {
                let mut app = fixture_app(locale);
                app.preview_theme(id, background.clone()).unwrap();
                record(
                    &app,
                    format!("final_main_{variant}_{width}x{height}_{locale}"),
                    width,
                    height,
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, 44);
}

#[test]
fn final_state_matrix_42_cases() {
    let mut count = 0;
    for id in ["aurora", "classic"] {
        for (locale, width, height) in [("zh-CN", 80, 24), ("zh-CN", 120, 36), ("en-US", 80, 24)] {
            for state in [
                "palette_bbr",
                "style_neon",
                "clean_chips",
                "recommended",
                "progress_2_of_4",
                "batch_confirm",
                "root_header",
            ] {
                let mut app = fixture_app(locale);
                app.preview_theme(id, theme::Background::Detected((11, 17, 25)))
                    .unwrap();
                match state {
                    "palette_bbr" => {
                        app.command_palette = Some(CommandPalette {
                            query: InputBuffer::new("bbr".into()),
                            ..CommandPalette::default()
                        })
                    }
                    "style_neon" => {
                        app.open_style();
                        app.preview_style(3);
                    }
                    "clean_chips" => {
                        app.form = Some(
                            app.make_form(
                                crate::actions::find("system.clean").unwrap(),
                                FormMode::Run,
                            )
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
                        app.log("2/4");
                    }
                    "batch_confirm" => {
                        app.group = OPTIMIZATION_GROUP;
                        app.marked = RECOMMENDED.iter().map(|id| (*id).to_owned()).collect();
                        app.confirmation = Some(super::stage2::confirmation(&app));
                    }
                    "root_header" => app.root = true,
                    _ => unreachable!(),
                }
                record(
                    &app,
                    format!("final_state_{id}_{state}_{width}x{height}_{locale}"),
                    width,
                    height,
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, 42);
    assert_eq!(builtin::all().len(), 8);
}
