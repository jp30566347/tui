use crate::api::article::Article;
use crate::app::Fetched;
use crate::card::Card;

#[derive(Debug)]
pub enum Action {
    /// Redraw the screen.
    Render,
    /// Fetch whichever feeds have gone stale.
    Refresh,
    /// Fetch every feed regardless of freshness, for an explicit `r`.
    ForceRefresh,
    /// A background fetch finished. Boxed: the payload is large and this
    /// variant would otherwise dominate the size of every `Action`.
    Fetched(Box<Fetched>),
    /// Fetch the story behind a headline, for the reader.
    FetchStory(String),
    /// A story fetch finished, keyed by the link it was asked for.
    StoryFetched(String, Result<Box<Article>, String>),
    /// Render a share card for a story, save it and put it on the clipboard.
    /// The only key with an effect outside the app; it is an action so that
    /// `handle_key` stays IO-free.
    Share(Box<Card>),
    /// The share finished, with a note for the status line either way.
    Shared(Result<String, String>),
}
