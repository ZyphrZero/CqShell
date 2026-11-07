//! Baselines of the screens the view tests draw, recorded before the theme refactor. The
//! Unchanged screens reproduce their baseline cell for cell. Intentional redesigns use
//! separately named classic snapshots, leaving the historical files intact.
//!
//! Each case starts from `fixture_app`, so no screen depends on the clock, the test
//! process's privileges or the host OS.
use super::{Background, Case, assert_snapshot, fixture_app, fixture_snapshot, render};
use crate::{
    actions,
    dashboard::Reading,
    error::Error,
    i18n::I18n,
    ops::Choice,
    runtime::Context,
    tui::{
        App, Confirmation, FormMode, InputBuffer, Loading, OPTIMIZATION_GROUP, Toast, Tone,
        view::Regions,
    },
};
use ratatui::layout::{Position, Rect};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

const LOCALES: [&str; 2] = ["zh-CN", "en-US"];

/// Compare with the original baseline or a separately recorded, intentional redesign.
/// The current screen is classic on a dark terminal, so nothing was detected.
fn record(app: &App, state: &str, width: u16, height: u16) {
    let locale = app.ctx.i18n.locale().to_owned();
    // Intentional redesigns get new expectations; the original baseline files stay intact.
    let prefix = if state == "small" {
        "baseline"
    } else {
        "classic_stage2"
    };
    let case = Case {
        name: format!("{prefix}_{state}_{width}x{height}_{locale}"),
        theme: "classic".into(),
        background: Background::Unavailable,
        locale,
    };
    assert_snapshot(&case, &render(app, width, height));
}

fn spec(id: &str) -> actions::Spec {
    actions::find(id).expect("static action ID")
}

#[test]
fn main_view_in_both_sizes_and_languages() {
    for locale in LOCALES {
        let app = fixture_app(locale);
        for (width, height) in [(80, 24), (120, 36)] {
            record(&app, "main", width, height);
        }
    }
}

#[test]
fn optimization_view_with_and_without_a_selection() {
    let mut app = fixture_app("en-US");
    app.group = OPTIMIZATION_GROUP;
    record(&app, "optimization", 120, 36);

    // Marks show as checkboxes, in the category's count and in the panel title.
    let mut app = fixture_app("zh-CN");
    app.group = OPTIMIZATION_GROUP;
    app.marked
        .extend(["bbr.enable".to_owned(), "tcp.tune".to_owned()]);
    app.positions[OPTIMIZATION_GROUP] = actions::OPTIMIZATION
        .iter()
        .position(|id| *id == "tcp.tune")
        .expect("tcp.tune is an optimization");
    record(&app, "optimization_marked", 80, 24);
}

#[test]
fn readings_that_failed_or_do_not_apply() {
    let mut app = fixture_app("en-US");
    app.snapshot.local.mihomo_state = Reading::Unavailable("invalid PID file".into());
    app.snapshot.local.mihomo_version = Reading::Unavailable("version probe failed".into());
    record(&app, "main_probe_errors", 120, 36);
    app.overview = true;
    record(&app, "overview_probe_errors", 120, 36);

    let mut app = fixture_app("en-US");
    app.snapshot.local.congestion = Reading::NotApplicable;
    app.snapshot.local.qdisc = Reading::NotApplicable;
    app.snapshot.local.bbr_available = Reading::NotApplicable;
    app.snapshot.local.bbr_version = Reading::NotApplicable;
    record(&app, "main_not_applicable", 120, 36);
}

#[test]
fn inline_logs_and_the_full_log_view() {
    let mut app = fixture_app("en-US");
    app.logs_open = true;
    app.log("working");
    record(&app, "logs_inline", 160, 50);

    let mut app = fixture_app("en-US");
    app.log_modal = true;
    for n in 0..200 {
        app.log(format!("line {n}"));
    }
    record(&app, "log_modal", 80, 24);
}

#[test]
fn parameter_forms() {
    // As in the minimum-size form test: defaults, with the submit button selected.
    for locale in LOCALES {
        let mut app = fixture_app(locale);
        let mut form = app.make_form(spec("tcp.tune"), FormMode::Run).unwrap();
        form.index = form.values.len();
        app.form = Some(form);
        record(&app, "form_submit", 80, 24);
    }
    let mut app = fixture_app("en-US");
    app.form = Some(app.make_form(spec("clash.sub.add"), FormMode::Run).unwrap());
    record(&app, "form", 120, 36);
}

fn subscription(n: usize, current: bool) -> Choice {
    Choice {
        value: format!("订阅-{n}"),
        detail: format!("host{n}.example"),
        current,
        children: Vec::new(),
    }
}

#[test]
fn pickers_over_their_forms() {
    let subscriptions = || (0..30).map(|n| subscription(n, n == 20)).collect();
    let open = |locale: &str, choices: Vec<Choice>| {
        let mut app = fixture_app(locale);
        let mut form = app.make_form(spec("clash.sub.use"), FormMode::Run).unwrap();
        form.attach(vec![choices]);
        form.open_picker(String::new(), true);
        app.form = Some(form);
        app
    };
    for locale in LOCALES {
        record(&open(locale, subscriptions()), "picker", 80, 24);
    }
    let mut app = open("zh-CN", subscriptions());
    let form = app.form.as_mut().unwrap();
    form.picker.as_mut().unwrap().filter = InputBuffer::new("zzz".into());
    record(&app, "picker_no_match", 80, 24);
    // Closed again, the form shows the chosen item with its place and context.
    app.form.as_mut().unwrap().picker = None;
    record(&app, "picker_chosen", 80, 24);

    let short = vec![subscription(0, false), subscription(1, false)];
    record(&open("zh-CN", short), "picker_short", 80, 24);
}

#[test]
fn slow_listing_waits_in_a_cancellable_dialog() {
    let mut app = fixture_app("zh-CN");
    let (_sender, receiver) = mpsc::channel();
    app.loading = Some(Loading {
        spec: spec("clash.proxy.select"),
        mode: FormMode::Run,
        receiver,
        cancel: Arc::new(AtomicBool::new(false)),
        started: app.now - Duration::from_secs(1),
    });
    record(&app, "loading", 80, 24);
}

#[test]
fn hovered_action_row() {
    let mut app = fixture_app("en-US");
    let rows = Regions::new(Rect::new(0, 0, 120, 36), &app).action_rows;
    // system.public_ip: below the first Section title and the selected system.info.
    app.hover = Some(Position::new(rows.x + 4, rows.y + 2));
    record(&app, "hover", 120, 36);
}

#[test]
fn overlays_at_minimum_size() {
    for locale in LOCALES {
        let mut app = fixture_app(locale);
        app.help = true;
        record(&app, "help", 80, 24);

        let mut app = fixture_app(locale);
        app.overview = true;
        record(&app, "overview", 80, 24);

        // Raised on the frame clock, so the toast is drawn at full opacity.
        let mut app = fixture_app(locale);
        app.toast = Some(Toast {
            text: app.ctx.i18n.text("toast.order_saved").to_owned(),
            tone: Tone::Success,
            shown: app.now,
        });
        record(&app, "toast", 80, 24);

        let mut app = fixture_app(locale);
        let error = Error::new("error.root", "sudo cqs");
        app.notice = Some(app.ctx.i18n.error(&error));
        record(&app, "notice", 80, 24);

        record(&fixture_app(locale), "small", 40, 10);
    }
}

#[test]
fn confirmations_short_and_scrolling() {
    let confirmation = |text: String| Confirmation {
        text,
        token: "YES".into(),
        input: InputBuffer::default(),
        reply: mpsc::channel().0,
        scroll: 0,
        accept: false,
    };
    let mut app = fixture_app("en-US");
    app.confirmation = Some(confirmation("Proceed?".into()));
    record(&app, "confirm", 80, 24);
    let long = (0..40)
        .map(|n| format!("item {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.confirmation = Some(confirmation(long));
    record(&app, "confirm_long", 80, 24);
}

/// The default output of `examples/tui_snapshot.rs`: `App::new`, then the readings swapped
/// in, drawn at 120×36 in zh-CN. The example samples the host live; here the fixture
/// readings stand in, and the two host-dependent fields `App::new` reads are pinned to what
/// the example shows on Windows, the platform it previews on: preview mode for a non-root
/// user. The frame clock stays at `App::new`'s, so the data ages are 2 s and 12 s.
#[test]
fn tui_snapshot_example_default() {
    let paths = crate::tests::context(Path::new("/nonexistent/cqs-snapshot")).paths;
    let i18n = I18n::load(Some("zh-CN"), None).expect("bundled locale");
    let mut app = App::new(Context::new(paths, i18n, false));
    app.root = false;
    app.platform_error = Some(Error::new("error.platform", "windows"));
    app.snapshot = fixture_snapshot(app.now);
    record(&app, "tui_snapshot", 120, 36);
}
