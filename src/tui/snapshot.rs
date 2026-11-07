//! Snapshot files for TUI regression tests, and the fixed state they render.
//!
//! A snapshot is a rendered buffer in a reviewed text format: a five-line header, the
//! character grid, `---`, then one `row col len fg bg mods` line per run of identically
//! styled cells. Files live in `tests/snapshots/tui/<case>.snap` and are compared byte for
//! byte; they are written only when `CQS_UPDATE_SNAPSHOTS=1` is set.
use super::App;
use crate::{
    dashboard::{LocalSnapshot, PublicSnapshot, Reading, Snapshot, Usage},
    i18n::I18n,
    ops::clash::State,
    runtime::Context,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    style::{Color, Modifier},
};
use std::{
    ffi::OsStr,
    fmt::{self, Write as _},
    fs, io,
    path::Path,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

mod baseline;

const UPDATE_VARIABLE: &str = "CQS_UPDATE_SNAPSHOTS";

/// The terminal background a snapshot was rendered against, as written in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Background {
    Detected(u8, u8, u8),
    Unavailable,
    Disabled,
}
impl fmt::Display for Background {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Detected(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
            Self::Unavailable => f.write_str("unavailable"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

/// Everything a snapshot records besides the buffer. `name` is also the file stem.
pub(super) struct Case {
    pub name: String,
    pub theme: String,
    pub background: Background,
    pub locale: String,
}

/// The style a cell is recorded with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Look {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}
impl Look {
    fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            modifier: cell.modifier,
        }
    }
    fn is_default(self) -> bool {
        self == Self::default()
    }
}

/// Render `buffer` in the snapshot format.
pub(super) fn format(case: &Case, buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut text = format!(
        "# case {}\n# size {}x{}\n# theme {}\n# background {}\n# locale {}\n",
        case.name, area.width, area.height, case.theme, case.background, case.locale
    );
    let mut runs = String::new();
    for row in 0..area.height {
        let mut looks = Vec::with_capacity(usize::from(area.width));
        let mut lead = Look::default();
        let mut hidden = 0;
        for col in 0..area.width {
            let cell = &buffer[(area.x + col, area.y + row)];
            if hidden > 0 {
                // A wide glyph covers this cell on a real terminal, so the cell takes the
                // glyph's style and its own (blank or stale) content is not written.
                hidden -= 1;
            } else {
                text.push_str(cell.symbol());
                hidden = cell.symbol().width().saturating_sub(1);
                lead = Look::of(cell);
            }
            looks.push(lead);
        }
        text.push('\n');
        let mut start = 0;
        while start < looks.len() {
            let look = looks[start];
            let len = looks[start..].iter().take_while(|&&l| l == look).count();
            if !look.is_default() {
                writeln!(
                    runs,
                    "{row} {start} {len} {} {} {}",
                    color(look.fg),
                    color(look.bg),
                    modifiers(look.modifier)
                )
                .expect("writing to a String cannot fail");
            }
            start += len;
        }
    }
    text.push_str("---\n");
    text.push_str(&runs);
    text
}

/// `reset`, an ANSI name or `#rrggbb`. ANSI names follow the theme model: ratatui's `Gray`
/// is ANSI white (index 7) and `White` is bright white (index 15).
fn color(color: Color) -> String {
    let name = match color {
        Color::Reset => "reset",
        Color::Black => "black",
        Color::Red => "red",
        Color::Green => "green",
        Color::Yellow => "yellow",
        Color::Blue => "blue",
        Color::Magenta => "magenta",
        Color::Cyan => "cyan",
        Color::Gray => "white",
        Color::DarkGray => "bright-black",
        Color::LightRed => "bright-red",
        Color::LightGreen => "bright-green",
        Color::LightYellow => "bright-yellow",
        Color::LightBlue => "bright-blue",
        Color::LightMagenta => "bright-magenta",
        Color::LightCyan => "bright-cyan",
        Color::White => "bright-white",
        Color::Rgb(r, g, b) => return format!("#{r:02x}{g:02x}{b:02x}"),
        // Themes only produce the colors above; anything else must fail, not be guessed.
        Color::Indexed(index) => {
            panic!("the snapshot format has no notation for Color::Indexed({index})")
        }
    };
    name.to_owned()
}

const MODIFIERS: [(Modifier, &str); 9] = [
    (Modifier::BOLD, "bold"),
    (Modifier::DIM, "dim"),
    (Modifier::ITALIC, "italic"),
    (Modifier::UNDERLINED, "underlined"),
    (Modifier::SLOW_BLINK, "slow_blink"),
    (Modifier::RAPID_BLINK, "rapid_blink"),
    (Modifier::REVERSED, "reversed"),
    (Modifier::HIDDEN, "hidden"),
    (Modifier::CROSSED_OUT, "crossed_out"),
];

/// Modifier names joined with `+`, or `-` for none.
fn modifiers(modifier: Modifier) -> String {
    let names = MODIFIERS
        .iter()
        .filter(|(flag, _)| modifier.contains(*flag))
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "-".into()
    } else {
        names.join("+")
    }
}

/// Compare `buffer` with `tests/snapshots/tui/<case>.snap`, or record it there when
/// `CQS_UPDATE_SNAPSHOTS=1` is set.
pub(super) fn assert_snapshot(case: &Case, buffer: &Buffer) {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tui");
    let update = update_requested(&case.name, std::env::var_os(UPDATE_VARIABLE).as_deref());
    if let Err(message) = check(&directory, case, buffer, update) {
        panic!("{message}");
    }
}

fn update_requested(name: &str, value: Option<&OsStr>) -> bool {
    !name.starts_with("baseline_") && value == Some(OsStr::new("1"))
}

/// Compare with, or with `update` write, `<directory>/<case>.snap`. Without `update` no file
/// is ever created or changed.
fn check(directory: &Path, case: &Case, buffer: &Buffer, update: bool) -> Result<(), String> {
    assert!(
        !case.name.is_empty()
            && case
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "a case name is a plain file stem: {:?}",
        case.name
    );
    let path = directory.join(format!("{}.snap", case.name));
    let actual = format(case, buffer);
    let expected = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if expected.as_deref() == Some(actual.as_bytes()) {
        return Ok(());
    }
    if update {
        return fs::create_dir_all(directory)
            .and_then(|()| fs::write(&path, &actual))
            .map_err(|error| format!("{}: {error}", path.display()));
    }
    Err(match expected {
        None => format!(
            "{}: snapshot missing; review the output and record it with {UPDATE_VARIABLE}=1",
            path.display()
        ),
        Some(expected) => format!(
            "{}: snapshot differs; {}\nreview the change and rewrite it with {UPDATE_VARIABLE}=1",
            path.display(),
            first_difference(&expected, &actual)
        ),
    })
}

fn first_difference(expected: &[u8], actual: &str) -> String {
    let expected = String::from_utf8_lossy(expected);
    let (mut want, mut got) = (expected.split('\n'), actual.split('\n'));
    let shown =
        |line: Option<&str>| line.map_or_else(|| "<end of file>".into(), |l| format!("{l:?}"));
    let mut line = 1;
    loop {
        match (want.next(), got.next()) {
            (None, None) => return "the files differ only in bytes that are not UTF-8".into(),
            (a, b) if a == b => line += 1,
            (a, b) => {
                return format!(
                    "first difference at line {line}:\n  expected: {}\n  actual:   {}",
                    shown(a),
                    shown(b)
                );
            }
        }
    }
}

/// Distance from the fixture App's start to its frame clock. Fixed, so spinner frames and
/// data ages never depend on when a test runs.
const FIXTURE_CLOCK: Duration = Duration::from_secs(60);
const LOCAL_AGE: Duration = Duration::from_secs(2);
const PUBLIC_AGE: Duration = Duration::from_secs(12);

/// Fixed readings for snapshots; nothing is sampled live. Ages are measured from `now`.
pub(super) fn fixture_snapshot(now: Instant) -> Snapshot {
    const GIB: u64 = 1 << 30;
    let text = |value: &str| Reading::Value(value.to_owned());
    Snapshot {
        local: LocalSnapshot {
            host: text("vps-sg-01"),
            os: text("Debian GNU/Linux 12 (bookworm)"),
            kernel: text("6.12.8-joeyblog-bbrv3"),
            arch: "x86_64".into(),
            cpu: text("AMD EPYC 7B13 × 2"),
            cpu_usage: Reading::Value(12.0),
            cpu_count: Reading::Value(2),
            terminal_proxy: Reading::Value(true),
            mihomo_state: Reading::Value(State::Running(4242)),
            mihomo_version: text("v1.19.2"),
            // 1.2 of 1.9 GiB; the total is the 1 992 294 KiB of effective memory.
            memory: Reading::Value(Usage {
                used: GIB * 12 / 10,
                total: 1_992_294 * 1024,
            }),
            effective_memory_kb: Reading::Value(1_992_294),
            swap: Reading::Value(Usage {
                used: GIB * 2 / 10,
                total: 2 * GIB,
            }),
            disk: Reading::Value(Usage {
                used: GIB * 164 / 10,
                total: 40 * GIB,
            }),
            disk_name: "/".into(),
            // 3 days 04 hours.
            uptime: Reading::Value(3 * 86_400 + 4 * 3_600),
            timezone: text("Asia/Shanghai"),
            congestion: text("cubic"),
            qdisc: text("fq_codel"),
            bbr_available: Reading::Value(true),
            bbr_version: text("v3"),
            tcp_configured: Reading::Value(false),
            // 6 MiB; tcp.tune with bandwidth 1000 and rtt 50 would raise it to 12 MiB.
            tcp_rmem_max: Reading::Value(6_291_456),
            updated: Some(now - LOCAL_AGE),
        },
        // Documentation address ranges and AS number.
        public: PublicSnapshot {
            ipv4: text("203.0.113.24"),
            ipv6: text("2001:db8:3c4d:15::1a2f"),
            isp: text("AS64500 Example Net"),
            updated: Some(now - PUBLIC_AGE),
        },
        // Oldest first; the newest is the current 12 %.
        cpu_history: vec![
            22.0, 35.0, 61.0, 48.0, 30.0, 18.0, 25.0, 12.0, 9.0, 14.0, 20.0, 12.0,
        ],
    }
}

/// The App state snapshots start from: the fixture readings, local data 2 s and public data
/// 12 s old, and a non-root user on a supported platform, so the screen depends on neither
/// the clock, the test process's privileges nor the host OS.
pub(super) fn fixture_app(locale: &str) -> App {
    // The paths name a directory that does not exist; drawing reads no files.
    let paths = crate::tests::context(Path::new("/nonexistent/cqs-snapshot")).paths;
    let i18n = I18n::load(Some(locale), None).expect("bundled locale");
    let mut app = App::new(Context::new(paths, i18n, false));
    app.theme.active = crate::theme::resolve(
        &crate::theme::builtin::classic(),
        &crate::theme::Background::Unavailable("snapshot fixture".into()),
    );
    app.now = app.started + FIXTURE_CLOCK;
    app.root = false;
    app.platform_error = None;
    app.snapshot = fixture_snapshot(app.now);
    app
}

/// Draw `app` once on a fresh `width`×`height` test terminal.
pub(super) fn render(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draw");
    terminal.backend().buffer().clone()
}

mod matrix;
mod stage2;

mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Style};

    fn case(name: &str) -> Case {
        Case {
            name: name.into(),
            theme: "classic".into(),
            background: Background::Detected(0x0b, 0x11, 0x19),
            locale: "zh-CN".into(),
        }
    }
    /// Grid rows and style lines of a formatted snapshot.
    fn body(text: &str) -> (Vec<&str>, Vec<&str>) {
        let lines = text.lines().skip(5).collect::<Vec<_>>();
        let rule = lines.iter().position(|line| *line == "---").expect("rule");
        (lines[..rule].to_vec(), lines[rule + 1..].to_vec())
    }

    #[test]
    fn format_writes_header_grid_and_style_runs() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 6, 2));
        let accent = Style::new()
            .fg(Color::Rgb(0x4f, 0xd1, 0xbd))
            .add_modifier(Modifier::BOLD);
        buffer.set_string(0, 0, "中", accent);
        buffer.set_string(2, 0, "ab", Style::new().fg(Color::Red));
        buffer.set_string(0, 1, "xyz", Style::new().bg(Color::Rgb(11, 17, 25)));
        assert_eq!(
            format(&case("unit"), &buffer),
            "# case unit\n# size 6x2\n# theme classic\n# background #0b1119\n# locale zh-CN\n\
             中ab  \nxyz   \n---\n\
             0 0 2 #4fd1bd reset bold\n0 2 2 red reset -\n1 0 3 reset #0b1119 -\n"
        );
        assert_eq!(Background::Unavailable.to_string(), "unavailable");
        assert_eq!(Background::Disabled.to_string(), "disabled");
    }

    #[test]
    fn continuation_cells_are_not_written_and_take_the_wide_glyph_style() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        buffer.set_string(0, 0, "中", Style::new().bg(Color::Rgb(1, 2, 3)));
        // An overdrawn continuation cell stays hidden behind the glyph on a real terminal.
        buffer[(1, 0)].set_symbol("X").set_fg(Color::Green);
        // A wide glyph in the last column must not swallow the start of the next row.
        buffer[(3, 0)].set_symbol("宽");
        buffer.set_string(0, 1, "abcd", Style::new());
        let text = format(&case("wide"), &buffer);
        let (grid, runs) = body(&text);
        assert_eq!(grid, ["中 宽", "abcd"]);
        assert_eq!(runs, ["0 0 2 reset #010203 -"]);
    }

    #[test]
    fn equal_neighbours_merge_and_default_runs_are_omitted() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 1));
        let red = Style::new().fg(Color::Red);
        buffer.set_string(0, 0, "ab", red);
        buffer.set_string(3, 0, "cd", red);
        buffer.set_string(5, 0, "e", red.add_modifier(Modifier::BOLD));
        // Explicitly reset styling is indistinguishable from untouched cells.
        buffer.set_string(6, 0, "f", Style::reset());
        let text = format(&case("runs"), &buffer);
        let (grid, runs) = body(&text);
        assert_eq!(grid, ["ab cdef "]);
        assert_eq!(
            runs,
            [
                "0 0 2 red reset -",
                "0 3 2 red reset -",
                "0 5 1 red reset bold"
            ]
        );
    }

    #[test]
    fn colors_and_modifiers_use_the_snapshot_notation() {
        for (value, name) in [
            (Color::Reset, "reset"),
            (Color::Black, "black"),
            (Color::Red, "red"),
            (Color::Green, "green"),
            (Color::Yellow, "yellow"),
            (Color::Blue, "blue"),
            (Color::Magenta, "magenta"),
            (Color::Cyan, "cyan"),
            (Color::Gray, "white"),
            (Color::DarkGray, "bright-black"),
            (Color::LightRed, "bright-red"),
            (Color::LightGreen, "bright-green"),
            (Color::LightYellow, "bright-yellow"),
            (Color::LightBlue, "bright-blue"),
            (Color::LightMagenta, "bright-magenta"),
            (Color::LightCyan, "bright-cyan"),
            (Color::White, "bright-white"),
            (Color::Rgb(0, 10, 255), "#000aff"),
        ] {
            assert_eq!(color(value), name);
        }
        assert_eq!(modifiers(Modifier::empty()), "-");
        assert_eq!(modifiers(Modifier::DIM | Modifier::BOLD), "bold+dim");
        assert_eq!(
            modifiers(Modifier::all()),
            "bold+dim+italic+underlined+slow_blink+rapid_blink+reversed+hidden+crossed_out"
        );
    }

    #[test]
    #[should_panic(expected = "Color::Indexed(42)")]
    fn colors_outside_the_format_fail_loudly() {
        color(Color::Indexed(42));
    }

    #[test]
    fn snapshot_files_are_written_only_on_request() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("tui");
        let path = directory.join("files.snap");
        let case = case("files");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer.set_string(0, 0, "abc", Style::new());

        let missing = check(&directory, &case, &buffer, false).unwrap_err();
        assert!(missing.contains(UPDATE_VARIABLE), "{missing}");
        assert!(!directory.exists(), "a plain check never creates files");

        check(&directory, &case, &buffer, true).unwrap();
        let recorded = fs::read(&path).unwrap();
        assert_eq!(recorded, format(&case, &buffer).as_bytes());
        assert!(!recorded.contains(&b'\r'), "snapshots use LF line endings");
        check(&directory, &case, &buffer, false).unwrap();

        buffer.set_string(1, 0, "X", Style::new());
        let changed = check(&directory, &case, &buffer, false).unwrap_err();
        assert!(
            changed.contains("line 6")
                && changed.contains("\"abc\"")
                && changed.contains("\"aXc\""),
            "{changed}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            recorded,
            "a mismatch is not written"
        );

        assert!(!update_requested("classic_main", None));
        assert!(!update_requested("classic_main", Some(OsStr::new(""))));
        assert!(!update_requested("classic_main", Some(OsStr::new("0"))));
        assert!(update_requested("classic_main", Some(OsStr::new("1"))));
        assert!(!update_requested("baseline_main", Some(OsStr::new("1"))));
    }

    #[test]
    fn fixture_app_has_fixed_ages_privileges_and_readings() {
        let compact = |buffer: &Buffer| {
            buffer
                .content
                .iter()
                .map(Cell::symbol)
                .collect::<String>()
                .replace(' ', "")
        };
        for locale in ["zh-CN", "en-US"] {
            let mut app = fixture_app(locale);
            assert_eq!(app.ctx.i18n.locale(), locale);
            assert!(!app.root && app.platform_error.is_none());
            assert_eq!(app.now - app.started, FIXTURE_CLOCK);
            let age = |at: Option<Instant>| app.now.duration_since(at.unwrap());
            assert_eq!(age(app.snapshot.local.updated), LOCAL_AGE);
            assert_eq!(age(app.snapshot.public.updated), PUBLIC_AGE);

            let i = &app.ctx.i18n;
            let screen = compact(&render(&app, 120, 36));
            let local = i.format("dash.age", &[("age", "2s")]).replace(' ', "");
            assert!(screen.contains("vps-sg-01"), "{screen}");
            assert!(screen.contains(&local), "{local} in {screen}");

            app.overview = true;
            let overview = compact(&render(&app, 120, 36));
            let public = app.ctx.i18n.text("dash.public_age").replace(' ', "");
            assert!(overview.contains(&format!("{public}12s")), "{overview}");
        }
    }
}
