//! Export the real TUI render buffer with local read-only measurements for visual QA.
use cqs::{config::Paths, dashboard::Sampler, i18n::I18n, runtime::Context, tui::App};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::{fs, path::PathBuf, time::Duration};

fn css_color(color: Color, foreground: bool) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#000000".into(),
        Color::White => "#ffffff".into(),
        Color::Red => "#ff5555".into(),
        Color::Green => "#50fa7b".into(),
        Color::Yellow => "#f1fa8c".into(),
        Color::Blue => "#6272a4".into(),
        Color::Magenta => "#ff79c6".into(),
        Color::Cyan => "#8be9fd".into(),
        Color::Gray => "#bfc7d5".into(),
        Color::DarkGray => "#627080".into(),
        Color::LightRed => "#ff6e6e".into(),
        Color::LightGreen => "#69ff94".into(),
        Color::LightYellow => "#ffffa5".into(),
        Color::LightBlue => "#d6acff".into(),
        Color::LightMagenta => "#ff92df".into(),
        Color::LightCyan => "#a4ffff".into(),
        Color::Indexed(index) => format!("var(--ansi-{index})"),
        Color::Reset if foreground => "#e5edf3".into(),
        Color::Reset => "#090e16".into(),
    }
}

fn html_preview(width: u16, cells: &[ratatui::buffer::Cell]) -> String {
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>CqShell TUI Preview</title><style>\
        :root{color-scheme:dark}body{margin:0;background:#090e16;color:#e5edf3;font-family:Segoe UI,sans-serif}header{padding:18px 24px;color:#91a2b0;border-bottom:1px solid #3d5265}main{overflow:auto;padding:24px}pre{width:max-content;min-width:100%;box-sizing:border-box;margin:0;padding:20px;border:1px solid #3d5265;border-radius:6px;background:#090e16;font:14px/1.35 Consolas,\"Cascadia Mono\",monospace;letter-spacing:0}span{white-space:pre}</style><header>CqShell · Windows TUI preview</header><main><pre>",
    );
    for (index, cell) in cells.iter().enumerate() {
        let bold = if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
            "font-weight:700;"
        } else {
            ""
        };
        html.push_str(&format!(
            "<span style=\"color:{};background:{};{}\">",
            css_color(cell.fg, true),
            css_color(cell.bg, false),
            bold
        ));
        for character in cell.symbol().chars() {
            html.push_str(match character {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                _ => {
                    html.push(character);
                    continue;
                }
            });
        }
        html.push_str("</span>");
        if (index + 1) % width as usize == 0 {
            html.push('\n');
        }
    }
    html.push_str("</pre></main></html>");
    html
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = PathBuf::from(
        args.next()
            .ok_or("usage: tui_snapshot output.json [width] [height] [locale]")?,
    );
    let width: u16 = args.next().unwrap_or_else(|| "120".into()).parse()?;
    let height: u16 = args.next().unwrap_or_else(|| "36".into()).parse()?;
    let locale = args.next().unwrap_or_else(|| "zh-CN".into());
    let paths = Paths::from_env()?;
    let mut sampler = Sampler::new();
    sampler.sample(&paths);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));
    let local = sampler.sample(&paths);
    let mut app = App::new(Context::new(paths, I18n::load(Some(&locale), None)?, false));
    app.snapshot.local = local;
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| app.draw(frame))?;
    let buffer = terminal.backend().buffer();
    let cells: Vec<_> = buffer.content.iter().map(|cell| serde_json::json!({
        "symbol": cell.symbol(), "fg": format!("{:?}", cell.fg), "bg": format!("{:?}", cell.bg), "bold": cell.modifier.contains(ratatui::style::Modifier::BOLD),
    })).collect();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output.with_extension("json"),
        serde_json::to_vec(&serde_json::json!({"width":width,"height":height,"cells":cells}))?,
    )?;
    fs::write(
        output.with_extension("html"),
        html_preview(width, &buffer.content),
    )?;
    Ok(())
}
