//! Offline frame-by-frame TUI recorder for generating README showcase GIFs.
//!
//! Invoked via `scripts/render_gifs.mjs`, or directly with:
//! `CQS_GIF_OUT_DIR=/tmp/cqs-frames cargo test --locked --lib -- --ignored record_readme_gifs --nocapture`

use super::{
    clash::{
        fixture::{sample_probe, sample_service_states, sample_snapshot},
        state::ClashModal,
    },
    docker::{fixture::fixture_docker_state, state::ContainerSubTab},
    layout::UiDensityMode,
    render::tokens,
    snapshot::fixture_snapshot,
    state::{App, NavTarget, Phase, Progress, Tone},
};
use crate::{
    dashboard::{ConnectionCounts, Reading},
    i18n::I18n,
    ops::{
        clash::model::{ClashTab, TrafficSample},
        docker::model::DockerTab,
    },
    runtime::{Context, Outcome, Step, StepState},
    theme::{Background, Role, ThemeSource},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::Rect,
    style::{Color, Modifier},
};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

fn key(code: KeyCode) -> KeyEvent {
    let mods = match code {
        KeyCode::Char(c) if c.is_ascii_uppercase() => KeyModifiers::SHIFT,
        _ => KeyModifiers::NONE,
    };
    KeyEvent::new(code, mods)
}

fn color_to_rgb(c: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => fallback,
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0xf7, 0x76, 0x8e),
        Color::Green => (0x9e, 0xce, 0x6a),
        Color::Yellow => (0xe0, 0xaf, 0x68),
        Color::Blue => (0x7a, 0xa2, 0xf7),
        Color::Magenta => (0xbb, 0x9a, 0xf7),
        Color::Cyan => (0x7d, 0xcf, 0xff),
        Color::Gray => (0xa9, 0xb1, 0xd6),
        Color::DarkGray => (0x56, 0x5f, 0x89),
        Color::LightRed => (0xff, 0x7a, 0x93),
        Color::LightGreen => (0xb9, 0xf2, 0x7c),
        Color::LightYellow => (0xff, 0x9e, 0x64),
        Color::LightBlue => (0x7d, 0xa6, 0xff),
        Color::LightMagenta => (0xbb, 0x9a, 0xf7),
        Color::LightCyan => (0x0d, 0xb9, 0xd7),
        Color::White => (0xff, 0xff, 0xff),
        Color::Indexed(idx) => xterm256(idx),
    }
}

fn xterm256(idx: u8) -> (u8, u8, u8) {
    const ANSI16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    if idx < 16 {
        return ANSI16[idx as usize];
    }
    if idx >= 232 {
        let g = 8 + (idx - 232) * 10;
        return (g, g, g);
    }
    let n = idx - 16;
    let r = n / 36;
    let g = (n / 6) % 6;
    let b = n % 6;
    let map = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
    (map(r), map(g), map(b))
}

fn rgb_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn dim_rgb(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| ((a as u16 * 62 + b as u16 * 38) / 100) as u8;
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

struct FrameRecord {
    w: u16,
    h: u16,
    mobile: bool,
    title: String,
    bg: String,
    fg: String,
    caption: String,
    key_badge: Option<String>,
    delay_ms: u64,
    cells_json: String,
}

struct Recorder {
    width: u16,
    height: u16,
    mobile: bool,
    title: String,
    now: Instant,
    caption: String,
    key_badge: Option<String>,
    frames: Vec<FrameRecord>,
}

impl Recorder {
    fn new(width: u16, height: u16, mobile: bool, title: &str) -> Self {
        Self {
            width,
            height,
            mobile,
            title: title.into(),
            now: Instant::now() + Duration::from_secs(60),
            caption: String::new(),
            key_badge: None,
            frames: Vec::new(),
        }
    }

    fn set_caption(&mut self, caption: &str, key_badge: Option<&str>) {
        self.caption = caption.into();
        self.key_badge = key_badge.map(str::to_string);
    }

    fn press(&mut self, app: &mut App, code: KeyCode, label: Option<&str>) {
        if let Some(lbl) = label {
            self.key_badge = Some(lbl.into());
        }
        let prev_toast_shown = app.toast.as_ref().map(|t| t.shown);
        let _ = app.handle_key(key(code));
        // Align freshly created toast timestamps with the synthetic recording clock.
        if let Some(toast) = app.toast.as_mut()
            && prev_toast_shown != Some(toast.shown)
        {
            toast.shown = self.now;
        }
    }

    fn capture(&mut self, app: &mut App, delay_ms: u64) {
        self.now += Duration::from_millis(delay_ms);
        app.viewport = Rect::new(0, 0, self.width, self.height);
        app.tick(self.now);

        let mut terminal = Terminal::new(TestBackend::new(self.width, self.height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buf = terminal.backend().buffer();

        let fallback_bg = color_to_rgb(
            tokens::color(&app.theme.active, Role::Canvas),
            (0x1a, 0x1b, 0x26),
        );
        let fallback_fg = color_to_rgb(
            tokens::color(&app.theme.active, Role::Text1),
            (0xc0, 0xca, 0xf5),
        );
        let bg_hex = rgb_hex(fallback_bg);
        let fg_hex = rgb_hex(fallback_fg);

        let mut cells = Vec::with_capacity((self.width as usize * self.height as usize) / 2);
        for y in 0..self.height {
            let mut x = 0u16;
            while x < self.width {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                let span = sym.width().max(1) as u16;

                let mut fg_rgb = color_to_rgb(cell.fg, fallback_fg);
                let mut bg_rgb = color_to_rgb(cell.bg, fallback_bg);
                if cell.modifier.contains(Modifier::REVERSED) {
                    std::mem::swap(&mut fg_rgb, &mut bg_rgb);
                }
                if cell.modifier.contains(Modifier::DIM) {
                    fg_rgb = dim_rgb(fg_rgb, bg_rgb);
                }
                let mut flags = 0u8;
                if cell.modifier.contains(Modifier::BOLD) {
                    flags |= 1;
                }
                if cell.modifier.contains(Modifier::ITALIC) {
                    flags |= 4;
                }
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    flags |= 8;
                }

                let is_empty_canvas = sym == " " && bg_rgb == fallback_bg && flags == 0;
                if !is_empty_canvas {
                    let c_fg = if fg_rgb == fallback_fg {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(rgb_hex(fg_rgb))
                    };
                    let c_bg = if bg_rgb == fallback_bg {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(rgb_hex(bg_rgb))
                    };
                    cells.push(json!([x, y, span, sym, c_fg, c_bg, flags]));
                }
                x = x.saturating_add(span);
            }
        }

        let cells_json = serde_json::to_string(&cells).unwrap();
        if let Some(last) = self.frames.last_mut()
            && last.bg == bg_hex
            && last.fg == fg_hex
            && last.caption == self.caption
            && last.key_badge == self.key_badge
            && last.cells_json == cells_json
        {
            last.delay_ms += delay_ms;
            return;
        }

        self.frames.push(FrameRecord {
            w: self.width,
            h: self.height,
            mobile: self.mobile,
            title: self.title.clone(),
            bg: bg_hex,
            fg: fg_hex,
            caption: self.caption.clone(),
            key_badge: self.key_badge.clone(),
            delay_ms,
            cells_json,
        });
    }

    fn steps(&mut self, app: &mut App, count: usize, step_ms: u64) {
        for _ in 0..count {
            self.capture(app, step_ms);
        }
    }

    fn write_ndjson(&self, path: &Path) {
        let mut out = BufWriter::new(File::create(path).unwrap());
        for f in &self.frames {
            let line = format!(
                "{{\"w\":{},\"h\":{},\"mobile\":{},\"title\":{},\"bg\":{},\"fg\":{},\"caption\":{},\"key\":{},\"delay_ms\":{},\"cells\":{}}}\n",
                f.w,
                f.h,
                f.mobile,
                serde_json::to_string(&f.title).unwrap(),
                serde_json::to_string(&f.bg).unwrap(),
                serde_json::to_string(&f.fg).unwrap(),
                serde_json::to_string(&f.caption).unwrap(),
                serde_json::to_string(&f.key_badge).unwrap(),
                f.delay_ms,
                f.cells_json,
            );
            out.write_all(line.as_bytes()).unwrap();
        }
        out.flush().unwrap();
        let total_ms: u64 = self.frames.iter().map(|f| f.delay_ms).sum();
        eprintln!(
            "Exported {}: {} unique frames ({:.1}s)",
            path.display(),
            self.frames.len(),
            total_ms as f64 / 1000.0
        );
    }
}

fn make_demo_app(theme_id: &str, now: Instant) -> (tempfile::TempDir, App) {
    let temp = tempfile::tempdir().unwrap();
    let paths = crate::tests::context(temp.path()).paths;
    let ctx = Context::new(paths, I18n::load(Some("zh-CN"), None).unwrap(), false);
    let mut app = App::new(ctx);
    app.started = now - Duration::from_secs(12);
    app.now = now;
    app.view_changed = now - Duration::from_secs(5);
    app.root = true;
    app.platform_error = None;
    app.snapshot = fixture_snapshot(now);
    app.snapshot.local.host = Reading::Value("srv-demo-01".into());
    app.snapshot.local.os = Reading::Value("Debian GNU/Linux 13 (trixie)".into());
    app.snapshot.local.mihomo_version = Reading::Value("v1.19.34".into());
    app.theme.reduced_motion = false;
    app.preview_theme(theme_id, Background::Detected((0x0d, 0x11, 0x17)))
        .unwrap();
    app.theme.source = ThemeSource::Default;
    app.palette_morph = None;
    app.tick(now);
    (temp, app)
}

fn future_clash_snapshot() -> crate::ops::clash::model::ClashSnapshot {
    let mut snap = sample_snapshot();
    if let Some(profile) = snap.profiles.first_mut()
        && let Some(userinfo) = profile.userinfo.as_mut()
    {
        userinfo.expire = Some(1_803_600_000);
    }
    snap
}

fn record_desktop_tour(out_dir: &Path) {
    let version = env!("CARGO_PKG_VERSION");
    let title = format!("cqs — srv-demo-01 · Debian 13 · v{version}");
    let mut rec = Recorder::new(116, 34, false, &title);
    let (_tmp, mut app) = make_demo_app("tokyo-night", rec.now);

    rec.set_caption("概览仪表盘 · Bento 指标与宿主机连接数", None);
    let cpu_wave = [14.0, 18.0, 26.0, 32.0, 29.0, 21.0, 17.0, 15.0];
    for (idx, &val) in cpu_wave.iter().enumerate() {
        app.snapshot.local.cpu_usage = Reading::Value(val);
        app.snapshot.local.connections = Reading::Value(ConnectionCounts {
            tcp: 24 + (idx % 4) as u64,
            tcp_time_wait: 5 + (idx % 2) as u64,
            udp: 6,
        });
        app.snapshot.cpu_history.push(val);
        if app.snapshot.cpu_history.len() > 14 {
            app.snapshot.cpu_history.remove(0);
        }
        app.snapshot.local.updated = Some(rec.now);
        rec.steps(&mut app, 3, 75);
    }

    rec.set_caption("实时外观抽屉 · 8 套配色 × 8 套样式平滑渐变", Some("T"));
    rec.press(&mut app, KeyCode::Char('t'), Some("T"));
    rec.steps(&mut app, 4, 65);

    for (cursor, label, badge) in [
        (1usize, "配色预览 · 曜石 Obsidian (Bento)", Some("↓")),
        (
            2usize,
            "配色预览 · 澄明 Daylight (Editorial 浅色)",
            Some("↓"),
        ),
        (4usize, "配色预览 · 摩卡 Catppuccin (Studio)", Some("↓")),
        (3usize, "配色预览 · 东京夜 Tokyo Night (HUD)", Some("↑")),
    ] {
        rec.set_caption(label, badge);
        app.preview_theme_cursor(cursor);
        rec.steps(&mut app, 3, 70);
        rec.steps(&mut app, 6, 100);
    }

    rec.set_caption("样式解耦 · 实时混搭 UI 设计语言", Some("Tab"));
    rec.press(&mut app, KeyCode::Tab, Some("Tab"));
    rec.steps(&mut app, 7, 90);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 6, 90);
    rec.press(&mut app, KeyCode::Esc, Some("Esc"));
    rec.steps(&mut app, 5, 70);

    rec.set_caption("一键优化 · 勾选批处理与实时任务进度", Some("O"));
    rec.press(&mut app, KeyCode::Char('o'), Some("O"));
    rec.steps(&mut app, 6, 85);
    rec.press(&mut app, KeyCode::Char('a'), Some("A"));
    rec.steps(&mut app, 6, 85);

    app.logs_open = true;
    app.phase = Phase::Running;
    app.task_ids = vec![
        "system.update".into(),
        "system.clean".into(),
        "tcp.tune".into(),
        "bbr.enable".into(),
    ];
    let mut progress = Progress::new(4, rec.now - Duration::from_secs(18));
    progress.apply(
        &Step {
            index: 0,
            total: 4,
            id: "system.update",
            state: StepState::Finished {
                outcome: Outcome::Done,
                elapsed: Duration::from_secs(11),
            },
        },
        rec.now - Duration::from_secs(7),
    );
    progress.apply(
        &Step {
            index: 1,
            total: 4,
            id: "system.clean",
            state: StepState::Started,
        },
        rec.now - Duration::from_secs(7),
    );
    app.progress = Some(progress);
    app.log_styled("✓ system.update completed in 11s", Tone::Success, false);
    app.log_styled(
        "• system.clean: pruning apt cache & old journals...",
        Tone::Info,
        true,
    );
    rec.set_caption("批处理执行中 · 事务化进度与实时日志", None);
    rec.steps(&mut app, 14, 80);

    app.phase = Phase::Idle;
    app.progress = None;
    app.logs_open = false;

    rec.set_caption("全局命令面板 · 模糊检索 115 项运维动作", Some("/"));
    rec.press(&mut app, KeyCode::Char('/'), Some("/"));
    rec.steps(&mut app, 5, 70);
    for ch in ['m', 'c', 'p'] {
        rec.press(&mut app, KeyCode::Char(ch), Some(&ch.to_string()));
        rec.steps(&mut app, 3, 85);
    }
    rec.steps(&mut app, 8, 95);
    rec.press(&mut app, KeyCode::Esc, Some("Esc"));
    rec.steps(&mut app, 3, 70);

    rec.set_caption(
        "原生 MCP 接入卡 · 0600 Bearer 遮罩与一键复制",
        Some("Shift+M"),
    );
    rec.press(&mut app, KeyCode::Char('M'), Some("Shift+M"));
    rec.steps(&mut app, 12, 95);

    rec.set_caption("MCP 传输切换 · Streamable HTTP ↔ SSH stdio", Some("Tab"));
    rec.press(&mut app, KeyCode::Tab, Some("Tab"));
    rec.steps(&mut app, 10, 95);

    rec.press(&mut app, KeyCode::Tab, Some("Tab"));
    rec.steps(&mut app, 4, 80);
    rec.set_caption("OSC 52 剪贴板复制 · 零明文密钥泄漏", Some("1"));
    rec.press(&mut app, KeyCode::Char('1'), Some("1"));
    rec.steps(&mut app, 10, 95);

    rec.press(&mut app, KeyCode::Esc, Some("Esc"));
    rec.press(&mut app, KeyCode::Char('h'), Some("H"));
    rec.set_caption("概览仪表盘 · Bento 指标与宿主机连接数", None);
    rec.steps(&mut app, 6, 85);

    rec.write_ndjson(&out_dir.join("desktop-tour.ndjson"));
}

fn record_mihomo_workspace(out_dir: &Path) {
    let mut rec = Recorder::new(116, 34, false, "cqs mihomo — Mihomo 6 标签可视化工作台");
    let (_tmp, mut app) = make_demo_app("obsidian", rec.now);
    app.clash.snapshot = future_clash_snapshot();
    app.clash.exit_probe = Some(sample_probe());
    app.clash.service_states = sample_service_states();
    app.open_clash(ClashTab::Overview, true, true);

    rec.set_caption("Mihomo 仪表盘 · 实时流量图谱与活跃连接", Some("1"));
    let wave_extra: [(u64, u64, u32); 16] = [
        (38_000, 420_000, 4),
        (56_000, 780_000, 5),
        (92_000, 1_240_000, 6),
        (135_000, 1_690_000, 7),
        (180_000, 2_120_000, 8),
        (240_000, 1_960_000, 8),
        (420_000, 1_540_000, 7),
        (680_000, 1_120_000, 6),
        (940_000, 890_000, 6),
        (720_000, 760_000, 5),
        (390_000, 610_000, 5),
        (180_000, 480_000, 4),
        (95_000, 390_000, 4),
        (62_000, 520_000, 5),
        (84_000, 860_000, 6),
        (48_000, 640_000, 5),
    ];
    for (u, d, c) in wave_extra {
        app.clash.snapshot.traffic_history.push_back(TrafficSample {
            up_bps: u,
            down_bps: d,
            active_conns: c,
        });
        if app.clash.snapshot.traffic_history.len() > 60 {
            app.clash.snapshot.traffic_history.pop_front();
        }
        app.clash.snapshot.up_speed = u;
        app.clash.snapshot.down_speed = d;
        app.clash.snapshot.upload_total += u / 4;
        app.clash.snapshot.download_total += d / 4;
        rec.steps(&mut app, 2, 80);
    }

    rec.set_caption("代理策略组 · 多列节点卡片与延迟排序", Some("2"));
    rec.press(&mut app, KeyCode::Char('2'), Some("2"));
    rec.steps(&mut app, 7, 90);
    rec.press(&mut app, KeyCode::Right, Some("→"));
    rec.steps(&mut app, 5, 90);
    rec.set_caption("代理策略组 · 按延迟一键排序", Some("o"));
    rec.press(&mut app, KeyCode::Char('o'), Some("o"));
    rec.steps(&mut app, 9, 95);
    rec.set_caption("切换策略组 · Proxy → Auto → OpenAI", Some("."));
    rec.press(&mut app, KeyCode::Char('.'), Some("."));
    rec.steps(&mut app, 6, 90);
    rec.press(&mut app, KeyCode::Char('.'), Some("."));
    rec.steps(&mut app, 8, 95);

    rec.set_caption("订阅配置 · 流量配额进度与到期倒计时", Some("3"));
    rec.press(&mut app, KeyCode::Char('3'), Some("3"));
    rec.steps(&mut app, 18, 100);

    rec.set_caption("实时请求 · 运行中连接与分流规则筛选", Some("4"));
    rec.press(&mut app, KeyCode::Char('4'), Some("4"));
    rec.steps(&mut app, 8, 95);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 10, 95);

    rec.press(&mut app, KeyCode::Char('1'), Some("1"));
    rec.steps(&mut app, 3, 75);
    rec.set_caption("13 项流媒体与 AI 服务解锁巡检", Some("S"));
    app.clash.modal = Some(ClashModal::ServiceStatus {
        cursor: 0,
        manage: false,
    });
    rec.steps(&mut app, 7, 90);
    for _ in 0..3 {
        rec.press(&mut app, KeyCode::Down, Some("↓"));
        rec.steps(&mut app, 4, 90);
    }
    rec.press(&mut app, KeyCode::Esc, Some("Esc"));
    rec.set_caption("Mihomo 仪表盘 · 实时流量图谱与活跃连接", None);
    rec.steps(&mut app, 6, 85);

    rec.write_ndjson(&out_dir.join("mihomo-workspace.ndjson"));
}

fn record_docker_workspace(out_dir: &Path) {
    let mut rec = Recorder::new(116, 34, false, "cqs docker — Docker 6 标签专属工作台");
    let (_tmp, mut app) = make_demo_app("catppuccin", rec.now);
    app.docker = fixture_docker_state(rec.now);
    app.open_docker(DockerTab::Containers, true, true);

    rec.set_caption("Docker 容器 · Space 多选批量操作", Some("1"));
    rec.steps(&mut app, 8, 90);
    rec.press(&mut app, KeyCode::Char(' '), Some("Space"));
    rec.steps(&mut app, 5, 90);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 4, 85);
    rec.press(&mut app, KeyCode::Char(' '), Some("Space"));
    rec.steps(&mut app, 9, 95);

    rec.set_caption("容器实时指标 · CPU / 内存 Sparkline 与进程", Some("m"));
    app.docker.container_sub = ContainerSubTab::Stats;
    rec.steps(&mut app, 18, 100);

    rec.set_caption("实时容器日志 · 级别高亮与脱敏保护", Some("l"));
    app.docker.container_sub = ContainerSubTab::Logs;
    rec.steps(&mut app, 16, 100);

    rec.set_caption("Compose 拓扑 · 项目与关联容器联动", Some("2"));
    rec.press(&mut app, KeyCode::Char('2'), Some("2"));
    rec.steps(&mut app, 16, 100);

    rec.set_caption("镜像管理 · 多源镜像加速预设（g）", Some("3"));
    rec.press(&mut app, KeyCode::Char('3'), Some("3"));
    rec.steps(&mut app, 12, 95);

    rec.set_caption("守护进程 · 事务化镜像源 / IPv6 / Prune", Some("6"));
    rec.press(&mut app, KeyCode::Char('6'), Some("6"));
    rec.steps(&mut app, 14, 95);

    app.docker.selected_ids[0].clear();
    app.docker.container_sub = ContainerSubTab::Overview;
    rec.press(&mut app, KeyCode::Char('1'), Some("1"));
    rec.set_caption("Docker 容器 · Space 多选批量操作", None);
    rec.steps(&mut app, 6, 85);

    rec.write_ndjson(&out_dir.join("docker-workspace.ndjson"));
}

fn record_mobile_touch(out_dir: &Path) {
    let version = env!("CARGO_PKG_VERSION");
    let title = format!("Termius · cqs v{version} (59×34)");
    let mut rec = Recorder::new(59, 34, true, &title);
    let (_tmp, mut app) = make_demo_app("obsidian", rec.now);
    app.ui_density = UiDensityMode::Auto;
    app.docker = fixture_docker_state(rec.now);
    app.clash.snapshot = future_clash_snapshot();
    app.clash.exit_probe = Some(sample_probe());
    app.clash.service_states = sample_service_states();

    rec.set_caption("窄屏堆叠概览 · 底部拇指栏", None);
    let cpu_wave = [15.0, 24.0, 19.0, 14.0];
    for &val in &cpu_wave {
        app.snapshot.local.cpu_usage = Reading::Value(val);
        app.snapshot.cpu_history.push(val);
        if app.snapshot.cpu_history.len() > 14 {
            app.snapshot.cpu_history.remove(0);
        }
        rec.steps(&mut app, 5, 95);
    }

    rec.set_caption("浮层导航抽屉", Some("["));
    rec.press(&mut app, KeyCode::Char('['), Some("["));
    rec.steps(&mut app, 5, 65);
    rec.steps(&mut app, 5, 90);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 4, 90);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 6, 95);
    rec.press(&mut app, KeyCode::Esc, Some("Esc"));
    rec.steps(&mut app, 3, 75);

    rec.set_caption("移动端 Mihomo 工作台", None);
    app.open_clash(ClashTab::Overview, false, false);
    rec.steps(&mut app, 11, 95);
    rec.set_caption("移动端代理节点切换", Some("→"));
    rec.press(&mut app, KeyCode::Right, Some("→"));
    rec.steps(&mut app, 12, 95);

    rec.set_caption("移动端 Docker 容器管理", None);
    app.open_docker(DockerTab::Containers, false, false);
    rec.steps(&mut app, 10, 95);
    rec.press(&mut app, KeyCode::Down, Some("↓"));
    rec.steps(&mut app, 10, 95);

    app.goto_nav(NavTarget::Overview);
    rec.set_caption("窄屏堆叠概览 · 底部拇指栏", Some("H"));
    rec.steps(&mut app, 6, 85);

    rec.write_ndjson(&out_dir.join("mobile-touch.ndjson"));
}

#[test]
#[ignore = "offline README GIF frame recorder; invoke via scripts/render_gifs.mjs"]
fn record_readme_gifs() {
    let out_dir = PathBuf::from(
        std::env::var("CQS_GIF_OUT_DIR").expect("set CQS_GIF_OUT_DIR to target frames directory"),
    );
    fs::create_dir_all(&out_dir).unwrap();
    record_desktop_tour(&out_dir);
    record_mihomo_workspace(&out_dir);
    record_docker_workspace(&out_dir);
    record_mobile_touch(&out_dir);
}
