//! Terminal setup, restore, and the two side effects that cannot live in a
//! cell buffer.

use color_eyre::eyre::Result;

/// Rendering goes to stderr, which leaves stdout free for piping.
pub type Backend = ratatui::backend::CrosstermBackend<std::io::Stderr>;
pub type Terminal = ratatui::Terminal<Backend>;

/// Puts the terminal back the way we found it.
///
/// Safe to call more than once, and callable from a panic hook, which is why
/// it is a free function rather than a method.
pub fn restore() -> Result<()> {
    crossterm::execute!(
        std::io::stderr(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

pub fn enter(terminal: &mut Terminal) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    // Mouse capture is deliberately not enabled: nothing here handles mouse
    // events, and turning it on breaks click-to-select in the host terminal.
    crossterm::execute!(
        std::io::stderr(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    terminal.clear()?;
    Ok(())
}

pub fn new_terminal() -> Result<Terminal> {
    Ok(ratatui::Terminal::new(
        ratatui::backend::CrosstermBackend::new(std::io::stderr()),
    )?)
}

/// Restores the terminal before a panic or error report is printed, so the
/// message lands on a usable screen instead of the alternate one.
pub fn install_hooks() -> Result<()> {
    let (panic_hook, eyre_hook) = color_eyre::config::HookBuilder::default().into_hooks();

    let eyre_hook = eyre_hook.into_eyre_hook();
    color_eyre::eyre::set_hook(Box::new(move |error| {
        let _ = restore();
        eyre_hook(error)
    }))?;

    let panic_hook = panic_hook.into_panic_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        panic_hook(info);
    }));

    Ok(())
}

/// Rings the terminal bell. Written straight to the terminal because it is
/// not something that can live in a ratatui cell buffer.
pub fn bell() {
    use std::io::Write;
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(b"\x07");
    let _ = stderr.flush();
}

/// Hands a URL to the platform's browser.
///
/// Spawned detached with both streams sent to null: a chatty opener writing to
/// stderr would draw straight onto the alternate screen, which is where these
/// apps render.
pub fn open_url(url: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open a browser with {opener}: {e}"))
}
