mod action;
mod api;
mod app;
mod card;
mod catalog;
mod cohort;
mod config;
mod tui;
mod ui;

use clap::Parser;
use color_eyre::eyre::Result;

#[derive(Parser)]
#[command(
    name = "macro-tui",
    version,
    about = "Macro market overview and news for the terminal"
)]
struct Cli {
    /// Starting tab (1=Movers, 2=Board, 3=Mega caps, 4=News)
    #[arg(short, long, value_parser = clap::value_parser!(u8).range(1..=3))]
    tab: Option<u8>,

    /// Print every instrument on the board and exit
    #[arg(long)]
    list_symbols: bool,

    /// Write the given --tab to the config file and exit
    #[arg(long)]
    save_config: bool,

    /// Ignore the config file
    #[arg(long)]
    no_config: bool,
}

/// Writes through `writeln!` rather than `println!` so a closed pipe comes
/// back as an error to handle rather than as a panic. Rust ignores SIGPIPE, so
/// `macro-tui --list-symbols | head` would otherwise abort with a backtrace.
fn list_symbols() -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let mut current = None;
    for instrument in catalog::INSTRUMENTS {
        if current != Some(instrument.group) {
            writeln!(out, "\n{}", instrument.group.as_str())?;
            current = Some(instrument.group);
        }
        writeln!(out, "  {:<16} {}", instrument.name, instrument.cnbc)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_symbols {
        // Piping into `head` closes the pipe early. That is the caller getting
        // what they asked for, not a failure.
        if let Err(e) = list_symbols() {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(e.into());
            }
        }
        return Ok(());
    }

    // Command line beats config file, which beats the built-in default.
    let stored = if cli.no_config {
        config::Config::default()
    } else {
        config::Config::load()?
    };
    let settings = config::Config {
        tab: cli.tab.or(stored.tab),
    };

    if cli.save_config {
        let path = settings.save()?;
        println!("Wrote {}", path.display());
        return Ok(());
    }

    tui_common::terminal::install_hooks()?;

    let tab = usize::from(
        settings
            .tab
            .unwrap_or(1)
            .clamp(1, app::Tab::ALL.len() as u8)
            - 1,
    );
    let mut app = app::App::new(tab);
    let mut tui = tui::Tui::new()?;

    tui.run(&mut app).await
}
