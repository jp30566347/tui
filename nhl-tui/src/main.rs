mod action;
mod api;
mod app;
mod config;
mod tui;
mod ui;

use clap::Parser;
use color_eyre::eyre::Result;

#[derive(Parser)]
#[command(
    name = "nhl-tui",
    version,
    about = "NHL scores dashboard for the terminal"
)]
struct Cli {
    /// Favorite team abbreviation (e.g., TOR, EDM, BOS)
    #[arg(short, long)]
    team: Option<String>,

    /// Starting tab (1=Scores, 2=Standings, 3=Schedule, 4=Skaters, 5=Goalies)
    ///
    /// No short form: `-t` belongs to `--team`.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=5))]
    tab: Option<u8>,

    /// Write the given --team and --tab to the config file and exit
    #[arg(long)]
    save_config: bool,

    /// Ignore the config file
    #[arg(long)]
    no_config: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Command line beats config file, which beats the built-in default.
    let stored = if cli.no_config {
        config::Config::default()
    } else {
        config::Config::load()?
    };
    let settings = config::Config {
        team: cli.team.clone().or(stored.team),
        tab: cli.tab.or(stored.tab),
    };

    if cli.save_config {
        let path = settings.save()?;
        println!("Wrote {}", path.display());
        return Ok(());
    }

    tui_common::terminal::install_hooks()?;

    let tab = usize::from(settings.tab.unwrap_or(1).clamp(1, 5) - 1);
    let mut app = app::App::new(settings.team, tab);
    let mut tui = tui::Tui::new()?;

    tui.run(&mut app).await
}
