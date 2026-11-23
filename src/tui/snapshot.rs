//! Deterministic fixture snapshot and streamlined golden snapshot harness for the redesigned TUI.
//! Snapshots are compared byte-for-byte; set `CQS_UPDATE_SNAPSHOTS=1` to record intentional updates.

use super::{
    palette,
    state::{App, FormMode, NavTarget, Phase, Progress, Tone},
};
use crate::{
    actions,
    dashboard::{LocalSnapshot, PublicSnapshot, Reading, Snapshot, Usage},
    i18n::I18n,
    ops::clash::State as ClashState,
    runtime::{Context, Outcome, Step, StepState},
    theme::{Background, ThemeSource},
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};
use std::{
    fs,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Fixed read-only snapshot fixture shared across TUI tests, `related.rs` tests, and golden renders.
pub(in crate::tui) fn fixture_snapshot(now: Instant) -> Snapshot {
    Snapshot {
        local: LocalSnapshot {
            host: Reading::Value("vps-sg-01".into()),
            os: Reading::Value("Debian GNU/Linux 12 (bookworm)".into()),
            kernel: Reading::Value("6.12.8-joeyblog-bbrv3".into()),
            arch: "x86_64".into(),
            cpu: Reading::Value("AMD EPYC 7B13".into()),
            cpu_usage: Reading::Value(12.0),
            cpu_count: Reading::Value(2),
            terminal_proxy: Reading::Value(true),
            mihomo_state: Reading::Value(ClashState::Running(4242)),
            mihomo_version: Reading::Value("v1.19.2".into()),
            memory: Reading::Value(Usage {
                used: (1.2 * GIB as f64) as u64,
                total: (1.9 * GIB as f64) as u64,
            }),
            effective_memory_kb: Reading::Value(1_992_294),
            swap: Reading::Value(Usage {
                used: (0.2 * GIB as f64) as u64,
                total: 2 * GIB,
            }),
            disk: Reading::Value(Usage {
                used: (16.4 * GIB as f64) as u64,
                total: 40 * GIB,
            }),
            disk_name: "/".into(),
            uptime: Reading::Value(3 * 86400 + 4 * 3600),
            timezone: Reading::Value("Asia/Shanghai".into()),
            congestion: Reading::Value("cubic".into()),
            qdisc: Reading::Value("fq_codel".into()),
            bbr_available: Reading::Value(true),
            bbr_version: Reading::Value("v3".into()),
            tcp_configured: Reading::Value(false),
            tcp_rmem_max: Reading::Value(6 * MIB),
            updated: Some(now - Duration::from_secs(2)),
        },
        public: PublicSnapshot {
            ipv4: Reading::Value("203.0.113.24".into()),
            ipv6: Reading::Value("2001:db8:3c4d:15::1a2f".into()),
            isp: Reading::Value("AS64500 Example Net".into()),
            updated: Some(now - Duration::from_secs(12)),
        },
        cpu_history: vec![
            18.0, 26.0, 48.0, 39.0, 25.0, 14.0, 22.0, 9.0, 11.0, 15.0, 16.0, 12.0,
        ],
    }
}

fn fixture_app(locale: &str, theme_id: &str, root: bool) -> App {
    let temp = tempfile::tempdir().unwrap();
    let paths = crate::tests::context(temp.path()).paths;
    let ctx = Context::new(paths, I18n::load(Some(locale), None).unwrap(), false);
    let mut app = App::new(ctx);
    let now = Instant::now();
    app.started = now - Duration::from_secs(10);
    app.now = now;
    app.view_changed = now - Duration::from_secs(5);
    app.root = root;
    app.platform_error = None;
    app.snapshot = fixture_snapshot(now);
    app.theme.reduced_motion = true;
    app.preview_theme(theme_id, Background::Detected((0x0d, 0x11, 0x17)))
        .unwrap();
    app.theme.source = ThemeSource::Default;
    app.tick(now);
    app
}

fn format_color(c: Color) -> String {
    match c {
        Color::Reset => "reset".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn render_snapshot(name: &str, width: u16, height: u16, app: &mut App) -> String {
    app.viewport = ratatui::layout::Rect::new(0, 0, width, height);
    app.tick(app.now);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buf = terminal.backend().buffer();

    let mut out = format!(
        "# case {name}\n# size {width}x{height}\n# theme {}\n# locale {}\n",
        app.theme.active.id,
        app.ctx.i18n.locale()
    );

    for y in 0..height {
        let mut x = 0;
        while x < width {
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            let w = sym.width().max(1) as u16;
            x += w;
        }
        out.push('\n');
    }

    out.push_str("---\n");
    for y in 0..height {
        let mut x = 0;
        while x < width {
            let cell = &buf[(x, y)];
            let fg = cell.fg;
            let bg = cell.bg;
            let bold = cell.modifier.contains(Modifier::BOLD);
            let start = x;
            while x < width {
                let c = &buf[(x, y)];
                if c.fg != fg || c.bg != bg || c.modifier.contains(Modifier::BOLD) != bold {
                    break;
                }
                x += 1;
            }
            if fg != Color::Reset || bg != Color::Reset || bold {
                out.push_str(&format!(
                    "{y} {start} {} {} {} {}\n",
                    x - start,
                    format_color(fg),
                    format_color(bg),
                    if bold { "bold" } else { "-" }
                ));
            }
        }
    }
    out
}

fn assert_snapshot(name: &str, width: u16, height: u16, app: &mut App) {
    let actual = render_snapshot(name, width, height, app);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tui");
    let path = dir.join(format!("{name}.snap"));
    if std::env::var_os("CQS_UPDATE_SNAPSHOTS").is_some_and(|v| v == "1") {
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing snapshot {} ({e}); run with CQS_UPDATE_SNAPSHOTS=1 to record",
            path.display()
        )
    });
    assert_eq!(
        actual.replace("\r\n", "\n"),
        expected.replace("\r\n", "\n"),
        "snapshot mismatch for {name}"
    );
}

#[test]
fn redesigned_tui_core_snapshot_matrix() {
    // 1. Wide Overview (Obsidian Dark, zh-CN)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    assert_snapshot("overview_obsidian_120x36_zh-CN", 120, 36, &mut app);

    // 2. Wide Overview (Daylight Light, en-US)
    let mut app = fixture_app("en-US", "daylight", false);
    assert_snapshot("overview_daylight_120x36_en-US", 120, 36, &mut app);

    // 3. Compact 80x24 Overview (auto-collapsed sidebar)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    assert_snapshot("overview_compact_80x24_zh-CN", 80, 24, &mut app);

    // 4. Plain ASCII 80x24 Overview
    let mut app = fixture_app("en-US", "plain", false);
    assert_snapshot("overview_plain_80x24_en-US", 80, 24, &mut app);

    // 5. Wide System Workbench (Sidebar + Action List + Inspector)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.goto_nav(NavTarget::Group(0));
    assert_snapshot("workbench_system_120x36_zh-CN", 120, 36, &mut app);

    // 6. Medium 100x30 Workbench (stacked compact inspector)
    let mut app = fixture_app("zh-CN", "catppuccin", true);
    app.goto_nav(NavTarget::Group(0));
    assert_snapshot("workbench_medium_100x30_zh-CN", 100, 30, &mut app);

    // 7. Optimization Checklist + Live Batch Progress + Bottom Log Drawer
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.goto_nav(NavTarget::Optimization);
    app.recommend().unwrap();
    app.logs_open = true;
    app.phase = Phase::Running;
    app.task_ids = vec![
        "system.update".into(),
        "system.clean".into(),
        "tcp.tune".into(),
        "bbr.enable".into(),
    ];
    let now = app.now;
    let mut progress = Progress::new(4, now - Duration::from_secs(25));
    progress.apply(
        &Step {
            index: 0,
            total: 4,
            id: "system.update",
            state: StepState::Finished {
                outcome: Outcome::Done,
                elapsed: Duration::from_secs(18),
            },
        },
        now - Duration::from_secs(7),
    );
    progress.apply(
        &Step {
            index: 1,
            total: 4,
            id: "system.clean",
            state: StepState::Started,
        },
        now - Duration::from_secs(7),
    );
    app.progress = Some(progress);
    app.log_styled("Step 1/4: system.update done", Tone::Success, false);
    app.log_styled(
        "Step 2/4: cleaning apt cache and journals...",
        Tone::Info,
        true,
    );
    assert_snapshot("optimization_progress_120x36_zh-CN", 120, 36, &mut app);

    // 8. Live Appearance & Theme Drawer (`T`) previewing Tokyo Night (Palette tab + HUD)
    let mut app = fixture_app("zh-CN", "obsidian", false);
    app.goto_nav(NavTarget::Group(0));
    app.open_theme_drawer();
    app.preview_theme_cursor(3); // tokyo-night
    assert_snapshot("theme_drawer_tokyo_120x36_zh-CN", 120, 36, &mut app);

    // 8b. Live Appearance Drawer Style tab + Tokyo Night HUD Overview (segmented gauges)
    let mut app = fixture_app("zh-CN", "tokyo-night", true);
    assert_snapshot("overview_tokyo_hud_120x36_zh-CN", 120, 36, &mut app);
    app.open_theme_drawer();
    app.switch_theme_drawer_tab(super::theme_drawer::DrawerTab::Style);
    assert_snapshot("theme_drawer_style_tab_120x36_zh-CN", 120, 36, &mut app);

    // 9. Enhanced Command Palette searching "bbr"
    let mut app = fixture_app("zh-CN", "obsidian", false);
    let mut pal = palette::CommandPalette::default();
    pal.query.replace("bbr".into());
    app.command_palette = Some(pal);
    assert_snapshot("palette_bbr_120x36_zh-CN", 120, 36, &mut app);

    // 10. Parameter Form with interactive Chips (`system.clean`)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    let clean = actions::find("system.clean").unwrap();
    app.form = Some(app.make_form(clean, FormMode::Run).unwrap());
    assert_snapshot("form_clean_chips_120x36_zh-CN", 120, 36, &mut app);

    // 11. High-Risk Confirmation Dialog
    let mut app = fixture_app("zh-CN", "obsidian", true);
    let (tx, _rx) = mpsc::channel();
    app.confirmation = Some(super::state::Confirmation {
        text: "Irreversible cleanup: remove old journals, temp files, and unused Docker data."
            .into(),
        token: "YES".into(),
        input: super::input::InputBuffer::default(),
        reply: tx,
        scroll: 0,
        accept: false,
    });
    assert_snapshot("confirm_modal_120x36_zh-CN", 120, 36, &mut app);

    // 12. Compact 80x24 Floating Sidebar Overlay (`[`)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.sidebar_overlay = true;
    assert_snapshot("sidebar_overlay_80x24_zh-CN", 80, 24, &mut app);

    // 13. Interactive Terminal Prompt Preview Studio (`terminal.preview`)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    let term_group = super::state::GROUPS
        .iter()
        .position(|g| *g == "terminal")
        .unwrap();
    app.goto_nav(NavTarget::Group(term_group));
    app.open_prompt_preview();
    assert_snapshot("prompt_preview_studio_120x36_zh-CN", 120, 36, &mut app);

    // 14. Split-Pane Terminal Beautification Form with Live Preview (`terminal.apply`)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    let term_apply = actions::find("terminal.apply").unwrap();
    let mut form = app.make_form(term_apply, FormMode::Run).unwrap();
    form.values[1].replace("box".into());
    form.values[2].replace("tokyo-night".into());
    form.values[3].replace("on".into());
    app.form = Some(form);
    assert_snapshot("form_terminal_apply_120x36_zh-CN", 120, 36, &mut app);
}

#[test]
fn mobile_tui_snapshot_matrix() {
    use super::docker::fixture::fixture_docker_state;
    use crate::ops::docker::model::DockerTab;

    // 1. iOS Termius 59x24 Overview (stacked metrics + unified quick actions card)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    assert_snapshot("overview_mobile_59x24_zh-CN", 59, 24, &mut app);

    // 2. iOS Termius 59x24 System Workbench (stacked list + section pills + compact inspector)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.goto_nav(NavTarget::Group(0));
    assert_snapshot("workbench_mobile_59x24_zh-CN", 59, 24, &mut app);

    // 3. iOS Termius 59x24 Docker Containers Workspace (stacked list + inspector + quick buttons)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, false, false);
    assert_snapshot("docker_mobile_59x24_zh-CN", 59, 24, &mut app);

    // 4. iOS Termius 59x24 Interactive Prompt Preview Studio (stacked full-width preview)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    let term_group = super::state::GROUPS
        .iter()
        .position(|g| *g == "terminal")
        .unwrap();
    app.goto_nav(NavTarget::Group(term_group));
    app.open_prompt_preview();
    assert_snapshot("prompt_preview_mobile_59x24_zh-CN", 59, 24, &mut app);

    // 5. Minimum Mobile Floor 40x16 Overview (en-US, ultra-compact metrics + unified quick actions)
    let mut app = fixture_app("en-US", "obsidian", false);
    assert_snapshot("overview_mobile_min_40x16_en-US", 40, 16, &mut app);
}

#[test]
fn docker_tui_snapshot_matrix() {
    use super::docker::{fixture::fixture_docker_state, state::ContainerSubTab};
    use crate::ops::docker::model::DockerTab;

    // 1. Wide Embedded Docker Containers Workbench (120x36, Obsidian, zh-CN)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, false, false);
    assert_snapshot("docker_containers_120x36_zh-CN", 120, 36, &mut app);

    // 2. Focus Fullscreen Mode with multi-select + Stats Sparklines & docker top (120x36, zh-CN)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, true, true);
    app.docker.container_sub = ContainerSubTab::Stats;
    app.docker.selected_ids[0].insert("c001nginx0000000000000001".into());
    app.docker.selected_ids[0].insert("c002api000000000000000002".into());
    assert_snapshot("docker_focus_stats_120x36_zh-CN", 120, 36, &mut app);

    // 3. Compact 80x24 Stacked Docker Layout (zh-CN)
    let mut app = fixture_app("zh-CN", "obsidian", true);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, false, false);
    assert_snapshot("docker_compact_80x24_zh-CN", 80, 24, &mut app);

    // 4. Fullscreen Immersive Logs Viewport (120x36, en-US)
    let mut app = fixture_app("en-US", "tokyo-night", true);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, true, true);
    app.docker.container_sub = ContainerSubTab::Logs;
    app.docker.logs.fullscreen = true;
    assert_snapshot("docker_logs_fullscreen_120x36_en-US", 120, 36, &mut app);

    // 5. Pure ASCII Plain Theme at 80x24 (en-US)
    let mut app = fixture_app("en-US", "plain", false);
    app.docker = fixture_docker_state(app.now);
    app.open_docker(DockerTab::Containers, true, true);
    assert_snapshot("docker_plain_80x24_en-US", 80, 24, &mut app);
}
