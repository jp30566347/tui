//! The terminal lifecycle and this app's action loop.
//!
//! Setup, restore and event reading come from `tui_common`; what stays here
//! is the loop that maps this app's own actions onto its state.

use std::time::Duration;

use color_eyre::eyre::Result;
use tokio::sync::mpsc;
use tui_common::events::{Event, EventPump};
use tui_common::terminal;

use crate::action::Action;
use crate::app::App;
use crate::{card, ui};

/// Quotes are one cheap request, so the board can be near-live. What the other
/// feeds cost is decided by `App::plan`, not here.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

pub use tui_common::terminal::restore;

pub struct Tui {
    terminal: terminal::Terminal,
    action_rx: mpsc::UnboundedReceiver<Action>,
    action_tx: mpsc::UnboundedSender<Action>,
}

impl Tui {
    pub fn new() -> Result<Self> {
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        Ok(Self {
            terminal: terminal::new_terminal()?,
            action_rx,
            action_tx,
        })
    }

    pub async fn run(&mut self, app: &mut App) -> Result<()> {
        terminal::enter(&mut self.terminal)?;
        let mut pump = EventPump::start(REFRESH_INTERVAL);

        // Restore the terminal even if the loop fails, so an error is
        // readable instead of being printed into the alternate screen.
        let result = self.event_loop(app, &mut pump).await;
        pump.cancel();
        restore()?;
        result
    }

    async fn event_loop(&mut self, app: &mut App, pump: &mut EventPump) -> Result<()> {
        app.spawn_fetch(self.action_tx.clone(), true);
        self.draw(app)?;

        loop {
            // Terminal input and the refresh clock arrive on one channel;
            // finished fetches arrive on the other.
            let action = tokio::select! {
                event = pump.next() => match event {
                    Some(Event::Key(key)) => app.handle_key(key),
                    Some(Event::Tick) => Some(Action::Refresh),
                    Some(Event::Resize) => Some(Action::Render),
                    None => break,
                },
                action = self.action_rx.recv() => match action {
                    Some(action) => Some(action),
                    None => break,
                },
            };

            if let Some(action) = action {
                match action {
                    Action::Render => {}
                    Action::Refresh => app.spawn_fetch(self.action_tx.clone(), false),
                    Action::ForceRefresh => app.spawn_fetch(self.action_tx.clone(), true),
                    Action::Fetched(fetched) => app.apply_fetch(*fetched),
                    Action::FetchStory(link) => app.spawn_story(self.action_tx.clone(), link),
                    Action::StoryFetched(link, result) => app.apply_story(link, result),
                    // Rasterising and encoding the card takes a noticeable
                    // fraction of a second, so it runs off the UI thread.
                    Action::Share(card) => {
                        let tx = self.action_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            let _ = tx.send(Action::Shared(card::share(&card)));
                        });
                    }
                    Action::Shared(result) => app.apply_shared(result),
                }
            }

            if app.should_quit {
                break;
            }
            self.draw(app)?;
        }
        Ok(())
    }

    fn draw(&mut self, app: &App) -> Result<()> {
        self.terminal.draw(|frame| ui::draw(frame, app))?;
        Ok(())
    }
}
