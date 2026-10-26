//! Export the real TUI render buffer with local read-only measurements for visual QA.
use cqs::{config::Paths, dashboard::Sampler, i18n::I18n, runtime::Context, tui::App};
use ratatui::{Terminal, backend::TestBackend};
use std::{fs, path::PathBuf, time::Duration};

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
    let cells: Vec<_> = terminal.backend().buffer().content.iter().map(|cell| serde_json::json!({
        "symbol": cell.symbol(), "fg": format!("{:?}", cell.fg), "bg": format!("{:?}", cell.bg), "bold": cell.modifier.contains(ratatui::style::Modifier::BOLD),
    })).collect();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output,
        serde_json::to_vec(&serde_json::json!({"width":width,"height":height,"cells":cells}))?,
    )?;
    Ok(())
}
