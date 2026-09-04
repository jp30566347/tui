//! The background task that turns terminal input and a refresh clock into a
//! single stream of events.
//!
//! Kept separate from each app's action loop: what an app does with a key is
//! its own business, but reading the keys is not.

use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEvent, KeyEventKind};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// What the pump produces. Deliberately not an app's own action type: the
/// mapping from event to action is where the two apps differ.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    Key(KeyEvent),
    /// The refresh interval elapsed.
    Tick,
    /// The terminal was resized, so the whole screen is invalid.
    Resize,
}

pub struct EventPump {
    task: tokio::task::JoinHandle<()>,
    cancellation_token: CancellationToken,
    rx: mpsc::UnboundedReceiver<Event>,
}

impl EventPump {
    /// Starts reading the terminal and ticking at `interval`.
    ///
    /// The first tick of an interval completes immediately; it is consumed
    /// here so callers can issue their own initial fetch without racing it.
    pub fn start(interval: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let cancellation_token = CancellationToken::new();
        let token = cancellation_token.clone();

        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut refresh = tokio::time::interval(interval);
            refresh.tick().await;

            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    maybe_event = reader.next().fuse() => {
                        match maybe_event {
                            // Only presses: this also dedupes key repeat on
                            // Windows, which reports press and release.
                            Some(Ok(CrosstermEvent::Key(key)))
                                if key.kind == KeyEventKind::Press =>
                            {
                                if tx.send(Event::Key(key)).is_err() {
                                    break;
                                }
                            }
                            Some(Ok(CrosstermEvent::Resize(_, _))) => {
                                if tx.send(Event::Resize).is_err() {
                                    break;
                                }
                            }
                            Some(Ok(_)) => {}
                            // stdin closed or broke: nothing more will arrive.
                            Some(Err(_)) | None => break,
                        }
                    }
                    _ = refresh.tick() => {
                        if tx.send(Event::Tick).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            task,
            cancellation_token,
            rx,
        }
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }
}

impl Drop for EventPump {
    fn drop(&mut self) {
        self.cancel();
        self.task.abort();
    }
}
