use crate::app::Fetched;

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
}
