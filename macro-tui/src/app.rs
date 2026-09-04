//! All application state, key handling, and fetch planning.
//!
//! Key handling deliberately does no IO: `handle_key` mutates state and
//! returns an optional follow-up `Action`, which is what makes every binding
//! testable without a terminal or a network.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::api::models::{Quote, Series};
use crate::api::rss::{Headline, Source};
use crate::api::MarketClient;
use crate::catalog::{self, Group, Instrument, INSTRUMENTS};
use tui_common::layout::cycle;

/// How long news stays fresh before a tick will refetch it.
const NEWS_TTL: Duration = Duration::from_secs(300);
/// Daily closes barely move within a session, so the board's sparkline data
/// is refreshed rarely.
const HISTORY_TTL: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Board,
    News,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Board, Tab::News];

    pub fn as_str(self) -> &'static str {
        match self {
            Tab::Board => "Board",
            Tab::News => "News",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn from_index(n: usize) -> Option<Self> {
        Self::ALL.get(n).copied()
    }

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }
}

/// How much history the detail chart shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Range {
    OneMonth,
    SixMonths,
}

impl Range {
    pub const ALL: [Range; 2] = [Range::OneMonth, Range::SixMonths];

    /// The value the history endpoint wants for this range.
    pub fn time_frame(self) -> &'static str {
        match self {
            Range::OneMonth => "P1M",
            Range::SixMonths => "P6M",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Range::OneMonth => "1M",
            Range::SixMonths => "6M",
        }
    }

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }
}

/// The board's history, plus any keys that had to be quarantined to get the
/// batch through.
pub type HistoryBatch = (HashMap<&'static str, Series>, Vec<&'static str>);

/// One round of network results. Boxed inside `Action`, since this is large
/// and would otherwise inflate every variant.
#[derive(Debug)]
pub struct Fetched {
    pub request_id: u64,
    pub quotes: Option<Result<HashMap<String, Quote>, String>>,
    pub news: Option<Vec<Result<Vec<Headline>, String>>>,
    /// The whole board's short history.
    pub history: Option<Result<HistoryBatch, String>>,
    /// One instrument's long history, fetched only when the detail view asks.
    pub long_history: Option<(&'static str, Result<Series, String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    quotes: bool,
    news: bool,
    history: bool,
    long_history: Option<&'static str>,
}

pub struct App {
    pub should_quit: bool,
    pub active_tab: Tab,
    /// `Some(catalog index)` while the detail view is open. The flag and the
    /// selection are one field so they cannot disagree.
    pub detail: Option<usize>,
    pub range: Range,

    /// Parallel to `INSTRUMENTS`. `None` means never fetched, or the endpoint
    /// did not recognise the symbol.
    pub quotes: Vec<Option<Quote>>,
    /// Merged across feeds, deduplicated, newest first.
    pub headlines: Vec<Headline>,
    pub history: HashMap<(Range, &'static str), Series>,
    /// Keys that failed on their own. Excluded from later batches so one
    /// rotted key cannot keep poisoning the whole board request.
    pub history_bad: HashSet<&'static str>,

    pub board_selected: usize,
    pub news_scroll: usize,
    pub rail_scroll: usize,
    pub detail_news_scroll: usize,
    /// `None` shows every source.
    pub news_filter: Option<Source>,
    /// Whether the board's news rail ignores the selection and shows
    /// everything.
    pub rail_all: bool,

    pub show_help: bool,
    /// Rows the content pane last rendered. Written by the UI so page keys
    /// move by an actual screenful; a `Cell` because drawing only borrows.
    pub viewport_rows: Cell<usize>,

    pub loading: bool,
    pub last_updated: Option<DateTime<Local>>,
    pub error: Option<String>,

    news_at: Option<Instant>,
    history_at: Option<Instant>,
    request_id: u64,
    client: MarketClient,
}

impl App {
    pub fn new(tab: usize) -> Self {
        Self {
            should_quit: false,
            active_tab: Tab::from_index(tab).unwrap_or(Tab::Board),
            detail: None,
            range: Range::OneMonth,
            quotes: vec![None; INSTRUMENTS.len()],
            headlines: Vec::new(),
            history: HashMap::new(),
            history_bad: HashSet::new(),
            board_selected: 0,
            news_scroll: 0,
            rail_scroll: 0,
            detail_news_scroll: 0,
            news_filter: None,
            rail_all: false,
            show_help: false,
            viewport_rows: Cell::new(20),
            loading: false,
            last_updated: None,
            error: None,
            news_at: None,
            history_at: None,
            request_id: 0,
            client: MarketClient::new(),
        }
    }

    /// The instrument the news panes are keyed to: the one under the detail
    /// view when it is open, otherwise the board selection.
    pub fn focused(&self) -> &'static Instrument {
        &INSTRUMENTS[self
            .detail
            .unwrap_or(self.board_selected)
            .min(INSTRUMENTS.len() - 1)]
    }

    // --- fetching --------------------------------------------------------

    fn plan(&self, force: bool) -> Plan {
        let stale = |at: Option<Instant>, ttl: Duration| at.is_none_or(|t| t.elapsed() > ttl);
        // Only the long range is fetched on demand; the short one comes down
        // for the whole board at once.
        let long_history = match (self.range, self.detail) {
            (Range::SixMonths, Some(n)) => INSTRUMENTS[n]
                .history
                .filter(|k| !self.history_bad.contains(k))
                .filter(|k| !self.history.contains_key(&(Range::SixMonths, *k))),
            _ => None,
        };
        Plan {
            quotes: true,
            news: force || self.headlines.is_empty() || stale(self.news_at, NEWS_TTL),
            history: force || self.history.is_empty() || stale(self.history_at, HISTORY_TTL),
            long_history,
        }
    }

    /// Issues whatever the plan calls for, off the UI thread.
    pub fn spawn_fetch(&mut self, tx: UnboundedSender<Action>, force: bool) {
        let plan = self.plan(force);
        self.request_id += 1;
        let request_id = self.request_id;
        self.loading = true;

        let client = self.client.clone();
        let symbols = catalog::all_symbols();
        let keys: Vec<&'static str> = INSTRUMENTS
            .iter()
            .filter_map(|i| i.history)
            .filter(|k| !self.history_bad.contains(k))
            .collect();

        tokio::spawn(async move {
            // Turns "not wanted" into `None` and any error into a string, so
            // one dead feed cannot take the others down with it.
            macro_rules! feed {
                ($want:expr, $call:expr) => {
                    async {
                        if $want {
                            Some($call.await.map_err(|e| e.to_string()))
                        } else {
                            None
                        }
                    }
                };
            }

            let quotes = feed!(plan.quotes, client.get_quotes(&symbols));
            let history = async {
                if plan.history {
                    Some(fetch_history_batch(&client, &keys, Range::OneMonth).await)
                } else {
                    None
                }
            };
            let news = async {
                if !plan.news {
                    return None;
                }
                let mut out = Vec::with_capacity(Source::ALL.len());
                let results =
                    futures::future::join_all(Source::ALL.iter().map(|s| client.get_feed(*s)))
                        .await;
                for r in results {
                    out.push(r.map_err(|e| e.to_string()));
                }
                Some(out)
            };
            let long_history = async {
                let key = plan.long_history?;
                Some((
                    key,
                    client
                        .get_history(&[key], Range::SixMonths.time_frame())
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|mut m| {
                            m.remove(key)
                                .ok_or_else(|| "no series returned".to_string())
                        }),
                ))
            };

            let (quotes, history, news, long_history) =
                tokio::join!(quotes, history, news, long_history);

            let quotes = quotes.map(|r| {
                r.map(|raw| {
                    raw.into_iter()
                        .filter_map(|(sym, q)| q.parse().map(|q| (sym, q)))
                        .collect()
                })
            });

            let _ = tx.send(Action::Fetched(Box::new(Fetched {
                request_id,
                quotes,
                news,
                history,
                long_history,
            })));
        });
    }

    /// Folds a finished fetch into state.
    pub fn apply_fetch(&mut self, fetched: Fetched) {
        // A slower earlier request must not overwrite a newer answer.
        if fetched.request_id != self.request_id {
            return;
        }
        self.loading = false;
        let mut errors: Vec<String> = Vec::new();

        if let Some(result) = fetched.quotes {
            match result {
                Ok(quotes) => {
                    // Placed by symbol, never by position.
                    for (n, instrument) in INSTRUMENTS.iter().enumerate() {
                        if let Some(q) = quotes.get(instrument.cnbc) {
                            self.quotes[n] = Some(q.clone());
                        }
                    }
                    self.last_updated = Some(Local::now());
                }
                Err(e) => errors.push(e),
            }
        }

        if let Some(result) = fetched.history {
            match result {
                Ok((series, bad)) => {
                    for key in bad {
                        self.history_bad.insert(key);
                    }
                    for (key, s) in series {
                        self.history.insert((Range::OneMonth, key), s);
                    }
                    self.history_at = Some(Instant::now());
                }
                Err(e) => errors.push(e),
            }
        }

        if let Some((key, result)) = fetched.long_history {
            match result {
                Ok(series) => {
                    self.history.insert((Range::SixMonths, key), series);
                }
                Err(e) => {
                    self.history_bad.insert(key);
                    errors.push(e);
                }
            }
        }

        if let Some(results) = fetched.news {
            let mut merged: Vec<Headline> = Vec::new();
            for r in results {
                match r {
                    Ok(items) => merged.extend(items),
                    Err(e) => errors.push(e),
                }
            }
            // Only replace a good pool when something came back; a total
            // outage should leave the last headlines on screen.
            if !merged.is_empty() {
                self.headlines = dedupe_and_sort(merged);
                self.news_at = Some(Instant::now());
            }
        }

        self.error = errors.into_iter().next();
        self.clamp_scroll();
    }

    // --- news selection --------------------------------------------------

    /// Headlines for the focused instrument, and a label saying which tier of
    /// the fallback produced them, so a fallback is never silently misleading.
    pub fn related_headlines(&self) -> (Vec<&Headline>, String) {
        let instrument = self.focused();
        if !self.rail_all {
            let hits = self.matching(std::iter::once(instrument));
            if !hits.is_empty() {
                return (hits, format!("News \u{00b7} {}", instrument.name));
            }
            let group = INSTRUMENTS.iter().filter(|i| i.group == instrument.group);
            let hits = self.matching(group);
            if !hits.is_empty() {
                return (hits, format!("News \u{00b7} {}", instrument.group.as_str()));
            }
        }
        (self.headlines.iter().collect(), "News \u{00b7} top".into())
    }

    fn matching<'a>(
        &'a self,
        instruments: impl Iterator<Item = &'a Instrument>,
    ) -> Vec<&'a Headline> {
        let needles: Vec<String> = instruments
            .flat_map(|i| {
                std::iter::once(i.name.to_lowercase())
                    .chain(i.aliases.iter().map(|a| a.to_string()))
            })
            .collect();
        self.headlines
            .iter()
            .filter(|h| needles.iter().any(|n| contains_word(&h.haystack, n)))
            .collect()
    }

    /// Headlines for the News tab, after the source filter.
    pub fn filtered_headlines(&self) -> Vec<&Headline> {
        self.headlines
            .iter()
            .filter(|h| self.news_filter.is_none_or(|s| h.source == s))
            .collect()
    }

    // --- key handling ----------------------------------------------------

    /// Handles one key press. Never performs IO; anything that must touch the
    /// outside world comes back as an `Action` for the event loop to run.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        // Checked first so it works from inside every overlay and view.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }

        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                self.show_help = false;
            }
            return None;
        }

        // Matched before the bare characters below, or `d` would shadow the
        // half-page chord.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let page = (self.page() / 2).max(1) as isize;
            match key.code {
                KeyCode::Char('d') => {
                    self.move_selection(page);
                    return None;
                }
                KeyCode::Char('u') => {
                    self.move_selection(-page);
                    return None;
                }
                _ => {}
            }
        }

        if self.detail.is_some() {
            return self.handle_detail_key(key);
        }

        let page = self.page() as isize;
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => return Some(Action::ForceRefresh),

            KeyCode::Char('1') => self.active_tab = Tab::Board,
            KeyCode::Char('2') => self.active_tab = Tab::News,
            KeyCode::Tab => self.active_tab = self.active_tab.next(),
            KeyCode::BackTab => self.active_tab = self.active_tab.prev(),

            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Char('g') | KeyCode::Home => self.set_selection(0),
            KeyCode::Char('G') | KeyCode::End => self.set_selection(isize::MAX),

            // Context-dependent, the way the reference app steps dates on one
            // tab and cycles filters on another.
            KeyCode::Char('l') | KeyCode::Right => match self.active_tab {
                Tab::Board => self.jump_group(1),
                Tab::News => self.cycle_news_filter(1),
            },
            KeyCode::Char('h') | KeyCode::Left => match self.active_tab {
                Tab::Board => self.jump_group(-1),
                Tab::News => self.cycle_news_filter(-1),
            },

            KeyCode::Char('n') => self.rail_scroll = self.rail_scroll.saturating_add(1),
            KeyCode::Char('N') => self.rail_scroll = self.rail_scroll.saturating_sub(1),
            KeyCode::Char('f') if self.active_tab == Tab::Board => {
                self.rail_all = !self.rail_all;
                self.rail_scroll = 0;
            }

            KeyCode::Enter => match self.active_tab {
                Tab::Board => {
                    self.detail = Some(self.board_selected);
                    self.detail_news_scroll = 0;
                    // The long range is only fetched when it is asked for.
                    return Some(Action::Refresh);
                }
                Tab::News => return self.open_selected_news(),
            },
            KeyCode::Char('o') => return self.open_selected_news(),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.detail = None;
                // Coming back to the board should not leave the rail scrolled
                // to where the detail's news list was.
                self.detail_news_scroll = 0;
            }
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => return Some(Action::ForceRefresh),
            KeyCode::Char('l') | KeyCode::Right => {
                self.range = self.range.next();
                return Some(Action::Refresh);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.range = self.range.prev();
                return Some(Action::Refresh);
            }
            KeyCode::Char('1') => {
                self.range = Range::OneMonth;
                return Some(Action::Refresh);
            }
            KeyCode::Char('6') => {
                self.range = Range::SixMonths;
                return Some(Action::Refresh);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.detail_news_scroll = self.detail_news_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.detail_news_scroll = self.detail_news_scroll.saturating_sub(1)
            }
            KeyCode::Char('f') => {
                self.rail_all = !self.rail_all;
                self.detail_news_scroll = 0;
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                let (hits, _) = self.related_headlines();
                let url = hits.get(self.detail_news_scroll).map(|h| h.link.clone());
                return url.map(Action::OpenUrl);
            }
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn open_selected_news(&self) -> Option<Action> {
        let (list, scroll) = match self.active_tab {
            Tab::News => (self.filtered_headlines(), self.news_scroll),
            Tab::Board => (self.related_headlines().0, self.rail_scroll),
        };
        list.get(scroll).map(|h| Action::OpenUrl(h.link.clone()))
    }

    /// One screenful of rows, as the UI last drew it.
    fn page(&self) -> usize {
        self.viewport_rows.get().max(1)
    }

    fn move_selection(&mut self, delta: isize) {
        match self.active_tab {
            Tab::Board => self.set_selection(self.board_selected as isize + delta),
            Tab::News => {
                let max = self.filtered_headlines().len().saturating_sub(1) as isize;
                self.news_scroll =
                    (self.news_scroll as isize + delta).clamp(0, max.max(0)) as usize;
            }
        }
    }

    fn set_selection(&mut self, to: isize) {
        let max = INSTRUMENTS.len() as isize - 1;
        self.board_selected = to.clamp(0, max) as usize;
        // The rail is keyed to the selection, so a new selection starts at the
        // top of its own headlines rather than mid-list.
        self.rail_scroll = 0;
    }

    /// Moves to the first instrument of the neighbouring group, which is how
    /// you cross a 26-row board without holding `j`.
    fn jump_group(&mut self, step: isize) {
        let current = INSTRUMENTS[self.board_selected].group;
        let at = Group::ALL.iter().position(|g| *g == current).unwrap_or(0);
        let target =
            Group::ALL[(at as isize + step).rem_euclid(Group::ALL.len() as isize) as usize];
        if let Some(n) = INSTRUMENTS.iter().position(|i| i.group == target) {
            self.set_selection(n as isize);
        }
    }

    fn cycle_news_filter(&mut self, step: isize) {
        // `None` is the "all sources" entry, so it sits at index 0 of a list
        // one longer than the sources themselves.
        let options: Vec<Option<Source>> = std::iter::once(None)
            .chain(Source::ALL.iter().map(|s| Some(*s)))
            .collect();
        self.news_filter = cycle(&options, self.news_filter, step);
        self.news_scroll = 0;
    }

    /// Keeps every cursor on a row that still exists.
    fn clamp_scroll(&mut self) {
        self.board_selected = self.board_selected.min(INSTRUMENTS.len().saturating_sub(1));
        self.news_scroll = self
            .news_scroll
            .min(self.filtered_headlines().len().saturating_sub(1));
        let related = self.related_headlines().0.len();
        self.rail_scroll = self.rail_scroll.min(related.saturating_sub(1));
        self.detail_news_scroll = self.detail_news_scroll.min(related.saturating_sub(1));
    }
}

/// Fetches the board's history, falling back to probing keys one at a time
/// when the batch is rejected.
///
/// One unrecognised key fails the whole batch, so a single rotted key would
/// otherwise cost every sparkline on the board. Probing individually finds the
/// culprit in one extra round, and the caller quarantines it so later batches
/// go straight through.
async fn fetch_history_batch(
    client: &MarketClient,
    keys: &[&'static str],
    range: Range,
) -> Result<HistoryBatch, String> {
    match client.get_history(keys, range.time_frame()).await {
        Ok(series) => Ok((series, Vec::new())),
        Err(batch_error) => {
            let results =
                futures::future::join_all(keys.iter().map(|k| async move {
                    (*k, client.get_history(&[*k], range.time_frame()).await)
                }))
                .await;

            let mut good = HashMap::new();
            let mut bad = Vec::new();
            for (key, result) in results {
                match result {
                    Ok(mut m) => match m.remove(key) {
                        Some(series) => {
                            good.insert(key, series);
                        }
                        None => bad.push(key),
                    },
                    Err(_) => bad.push(key),
                }
            }
            if good.is_empty() {
                Err(batch_error.to_string())
            } else {
                Ok((good, bad))
            }
        }
    }
}

/// Merges the feeds into one pool: same story from two sources appears once,
/// newest first, undated last.
fn dedupe_and_sort(mut headlines: Vec<Headline>) -> Vec<Headline> {
    headlines.sort_by(|a, b| match (a.published, b.published) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let mut seen = HashSet::new();
    headlines.retain(|h| seen.insert(canonical_link(&h.link)));
    headlines
}

/// A link without its tracking query, so the same story syndicated twice is
/// recognised as one.
fn canonical_link(link: &str) -> String {
    link.split(['?', '#']).next().unwrap_or(link).to_lowercase()
}

/// Substring match that will not fire inside a longer word.
///
/// A plain `contains` would match "cac" in "vacation" and "eth" in "whether",
/// which is how a currency row ends up showing weather news.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    let mut at = 0;
    while let Some(found) = haystack[at..].find(needle) {
        let start = at + found;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        if boundary(before) && boundary(after) {
            return true;
        }
        // Advance by one character, not one byte, or a multi-byte haystack
        // would panic on the next slice.
        at = start + haystack[start..].chars().next().map_or(1, |c| c.len_utf8());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(0)
    }

    fn headline(title: &str, link: &str, ts: &str, source: Source) -> Headline {
        Headline {
            title: title.into(),
            link: link.into(),
            published: DateTime::parse_from_rfc2822(ts)
                .ok()
                .map(|d| d.with_timezone(&Utc)),
            source,
            haystack: title.to_lowercase(),
        }
    }

    #[test]
    fn enter_opens_the_detail_view_for_the_selected_instrument() {
        let mut a = app();
        a.handle_key(key('j'));
        a.handle_key(code(KeyCode::Enter));
        assert_eq!(a.detail, Some(1));
    }

    #[test]
    fn esc_closes_the_detail_view_without_quitting() {
        let mut a = app();
        a.detail = Some(3);
        a.handle_key(code(KeyCode::Esc));
        assert_eq!(a.detail, None);
        assert!(!a.should_quit);
    }

    /// Esc is deliberately not a quit key anywhere.
    #[test]
    fn esc_on_the_board_does_not_quit() {
        let mut a = app();
        a.handle_key(code(KeyCode::Esc));
        assert!(!a.should_quit);
    }

    #[test]
    fn ctrl_c_quits_from_inside_the_detail_view_and_the_help_overlay() {
        let mut a = app();
        a.detail = Some(0);
        a.handle_key(ctrl('c'));
        assert!(a.should_quit);

        let mut b = app();
        b.show_help = true;
        b.handle_key(ctrl('c'));
        assert!(b.should_quit);
    }

    /// `d` opens nothing here, but the chord must still win over any future
    /// bare binding, which is why it is matched first.
    #[test]
    fn ctrl_d_pages_rather_than_being_read_as_a_bare_d() {
        let mut a = app();
        a.viewport_rows.set(10);
        a.handle_key(ctrl('d'));
        assert_eq!(a.board_selected, 5);
        a.handle_key(ctrl('u'));
        assert_eq!(a.board_selected, 0);
    }

    #[test]
    fn the_help_overlay_swallows_keys_until_it_is_closed() {
        let mut a = app();
        a.handle_key(key('?'));
        assert!(a.show_help);
        a.handle_key(key('j'));
        assert_eq!(a.board_selected, 0, "j should not have moved the selection");
        a.handle_key(code(KeyCode::Esc));
        assert!(!a.show_help);
    }

    #[test]
    fn the_selection_stops_at_both_ends_of_the_catalog() {
        let mut a = app();
        a.handle_key(key('k'));
        assert_eq!(a.board_selected, 0);
        a.handle_key(key('G'));
        assert_eq!(a.board_selected, INSTRUMENTS.len() - 1);
        a.handle_key(key('j'));
        assert_eq!(a.board_selected, INSTRUMENTS.len() - 1);
        a.handle_key(key('g'));
        assert_eq!(a.board_selected, 0);
    }

    #[test]
    fn l_and_h_jump_between_groups_on_the_board() {
        let mut a = app();
        assert_eq!(a.focused().group, Group::UsEquity);
        a.handle_key(key('l'));
        assert_eq!(a.focused().group, Group::Rates);
        a.handle_key(key('l'));
        assert_eq!(a.focused().group, Group::Commodities);
        a.handle_key(key('h'));
        assert_eq!(a.focused().group, Group::Rates);
    }

    #[test]
    fn l_and_h_switch_the_chart_range_inside_the_detail_view() {
        let mut a = app();
        a.detail = Some(0);
        assert_eq!(a.range, Range::OneMonth);
        a.handle_key(key('l'));
        assert_eq!(a.range, Range::SixMonths);
        a.handle_key(key('h'));
        assert_eq!(a.range, Range::OneMonth);
    }

    #[test]
    fn switching_to_the_long_range_asks_for_history_it_does_not_have() {
        let mut a = app();
        a.detail = Some(0);
        a.range = Range::SixMonths;
        assert_eq!(a.plan(false).long_history, INSTRUMENTS[0].history);
    }

    #[test]
    fn a_cached_long_range_asks_for_nothing() {
        let mut a = app();
        a.detail = Some(0);
        a.range = Range::SixMonths;
        a.history
            .insert((Range::SixMonths, INSTRUMENTS[0].history.unwrap()), vec![]);
        assert_eq!(a.plan(false).long_history, None);
    }

    /// The board's own history is batched, so the detail view must not also
    /// fetch it one instrument at a time.
    #[test]
    fn the_short_range_is_never_fetched_per_instrument() {
        let mut a = app();
        a.detail = Some(0);
        a.range = Range::OneMonth;
        assert_eq!(a.plan(false).long_history, None);
    }

    #[test]
    fn quotes_are_always_planned_but_news_and_history_wait_for_their_interval() {
        let mut a = app();
        let first = a.plan(false);
        assert!(first.quotes && first.news && first.history);

        a.news_at = Some(Instant::now());
        a.history_at = Some(Instant::now());
        a.headlines.push(headline(
            "x",
            "https://e.com/1",
            "Fri, 04 Sep 2026 12:00:00 GMT",
            Source::CnbcTop,
        ));
        a.history.insert((Range::OneMonth, "k"), vec![]);

        let second = a.plan(false);
        assert!(second.quotes);
        assert!(!second.news && !second.history);

        let forced = a.plan(true);
        assert!(forced.quotes && forced.news && forced.history);
    }

    #[test]
    fn a_quarantined_history_key_is_dropped_from_the_next_plan() {
        let mut a = app();
        a.detail = Some(0);
        a.range = Range::SixMonths;
        a.history_bad.insert(INSTRUMENTS[0].history.unwrap());
        assert_eq!(a.plan(false).long_history, None);
    }

    /// A slow earlier response arriving after a newer one must not win.
    #[test]
    fn a_superseded_fetch_is_discarded() {
        let mut a = app();
        a.request_id = 7;
        a.apply_fetch(Fetched {
            request_id: 3,
            quotes: Some(Ok(HashMap::from([(
                ".SPX".to_string(),
                Quote {
                    last: 1.0,
                    change: 0.0,
                    change_pct: 0.0,
                    open: None,
                    high: None,
                    low: None,
                    prev_close: None,
                    year_high: None,
                    year_low: None,
                    market_status: None,
                },
            )]))),
            news: None,
            history: None,
            long_history: None,
        });
        assert!(a.quotes[0].is_none());
    }

    /// Rows are matched by symbol, so a reordered response cannot put one
    /// instrument's price on another's row.
    #[test]
    fn quotes_are_placed_by_symbol_not_by_response_position() {
        let mut a = app();
        let quote = |last| Quote {
            last,
            change: 0.0,
            change_pct: 0.0,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            year_high: None,
            year_low: None,
            market_status: None,
        };
        // Deliberately not in catalog order.
        a.apply_fetch(Fetched {
            request_id: a.request_id,
            quotes: Some(Ok(HashMap::from([
                (".FTMIB".to_string(), quote(52173.59)),
                (".SPX".to_string(), quote(7738.79)),
            ]))),
            news: None,
            history: None,
            long_history: None,
        });
        let spx = INSTRUMENTS.iter().position(|i| i.cnbc == ".SPX").unwrap();
        let mib = INSTRUMENTS.iter().position(|i| i.cnbc == ".FTMIB").unwrap();
        assert_eq!(a.quotes[spx].as_ref().unwrap().last, 7738.79);
        assert_eq!(a.quotes[mib].as_ref().unwrap().last, 52173.59);
    }

    /// A failed refresh must never blank a board that had good numbers.
    #[test]
    fn a_failed_quote_fetch_leaves_the_last_good_prices_on_screen() {
        let mut a = app();
        a.quotes[0] = Some(Quote {
            last: 7738.79,
            change: 0.0,
            change_pct: 0.0,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            year_high: None,
            year_low: None,
            market_status: None,
        });
        a.apply_fetch(Fetched {
            request_id: a.request_id,
            quotes: Some(Err("network down".into())),
            news: None,
            history: None,
            long_history: None,
        });
        assert_eq!(a.quotes[0].as_ref().unwrap().last, 7738.79);
        assert_eq!(a.error.as_deref(), Some("network down"));
    }

    #[test]
    fn headlines_are_deduplicated_across_feeds_and_sorted_newest_first() {
        let merged = dedupe_and_sort(vec![
            headline(
                "old",
                "https://e.com/a",
                "Fri, 04 Sep 2026 10:00:00 GMT",
                Source::CnbcTop,
            ),
            headline(
                "new",
                "https://e.com/b",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::MarketWatch,
            ),
            // Same story as the first, with a tracking query appended.
            headline(
                "old syndicated",
                "https://e.com/a?syn=1",
                "Fri, 04 Sep 2026 10:00:00 GMT",
                Source::FinancialTimes,
            ),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "new");
        assert_eq!(merged[1].title, "old");
    }

    #[test]
    fn undated_headlines_sort_last_rather_than_first() {
        let merged = dedupe_and_sort(vec![
            headline("undated", "https://e.com/a", "not a date", Source::CnbcTop),
            headline(
                "dated",
                "https://e.com/b",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::CnbcTop,
            ),
        ]);
        assert_eq!(merged[0].title, "dated");
    }

    /// The reason matching is not a plain `contains`.
    #[test]
    fn a_headline_matches_an_alias_only_on_a_word_boundary() {
        assert!(contains_word("gold hits a record", "gold"));
        assert!(contains_word("the price of gold", "gold"));
        assert!(contains_word("eur/usd slips", "eur/usd"));
        assert!(!contains_word("book a vacation", "cac"));
        assert!(!contains_word("whether it rallies", "eth"));
        assert!(!contains_word("goldman sachs hires", "gold"));
    }

    #[test]
    fn word_matching_handles_multibyte_text_without_panicking() {
        assert!(contains_word("japan\u{2019}s yen weakens", "yen"));
        assert!(!contains_word("caf\u{e9} culture", "eth"));
    }

    #[test]
    fn an_instrument_with_no_matching_headlines_falls_back_to_its_group() {
        let mut a = app();
        a.headlines = vec![headline(
            "Copper hits a record",
            "https://e.com/c",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::CnbcTop,
        )];
        // Select silver, which no headline mentions, but copper shares its
        // group.
        a.board_selected = INSTRUMENTS.iter().position(|i| i.name == "Silver").unwrap();
        let (hits, label) = a.related_headlines();
        assert_eq!(hits.len(), 1);
        assert!(label.contains("Commodities"), "got {label}");
    }

    #[test]
    fn an_instrument_matched_directly_is_labelled_with_its_own_name() {
        let mut a = app();
        a.headlines = vec![headline(
            "Gold hits a record",
            "https://e.com/g",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::CnbcTop,
        )];
        a.board_selected = INSTRUMENTS.iter().position(|i| i.name == "Gold").unwrap();
        let (hits, label) = a.related_headlines();
        assert_eq!(hits.len(), 1);
        assert!(label.contains("Gold"), "got {label}");
    }

    #[test]
    fn with_no_matches_at_all_the_rail_shows_the_whole_pool() {
        let mut a = app();
        a.headlines = vec![headline(
            "An unrelated story",
            "https://e.com/u",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::CnbcTop,
        )];
        let (hits, label) = a.related_headlines();
        assert_eq!(hits.len(), 1);
        assert!(label.contains("top"), "got {label}");
    }

    #[test]
    fn the_news_filter_cycles_through_all_sources_and_back_to_everything() {
        let mut a = app();
        a.active_tab = Tab::News;
        assert_eq!(a.news_filter, None);
        for source in Source::ALL {
            a.handle_key(key('l'));
            assert_eq!(a.news_filter, Some(source));
        }
        a.handle_key(key('l'));
        assert_eq!(a.news_filter, None, "should wrap back to all sources");
    }

    #[test]
    fn opening_a_story_with_an_empty_news_pool_does_nothing() {
        let mut a = app();
        a.active_tab = Tab::News;
        assert!(a.handle_key(key('o')).is_none());
    }

    #[test]
    fn o_returns_the_url_of_the_selected_story_rather_than_opening_it() {
        let mut a = app();
        a.active_tab = Tab::News;
        a.headlines = vec![headline(
            "Story",
            "https://e.com/s",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::CnbcTop,
        )];
        match a.handle_key(key('o')) {
            Some(Action::OpenUrl(url)) => assert_eq!(url, "https://e.com/s"),
            other => panic!("expected an OpenUrl action, got {other:?}"),
        }
    }

    /// A shrinking news pool must not leave the cursor past the end.
    #[test]
    fn cursors_are_clamped_when_the_pool_shrinks() {
        let mut a = app();
        a.active_tab = Tab::News;
        a.headlines = (0..5)
            .map(|n| {
                headline(
                    "x",
                    &format!("https://e.com/{n}"),
                    "Fri, 04 Sep 2026 13:00:00 GMT",
                    Source::CnbcTop,
                )
            })
            .collect();
        a.news_scroll = 4;
        a.headlines.truncate(2);
        a.clamp_scroll();
        assert_eq!(a.news_scroll, 1);
    }

    #[test]
    fn tabs_cycle_in_both_directions() {
        assert_eq!(Tab::Board.next(), Tab::News);
        assert_eq!(Tab::News.next(), Tab::Board);
        assert_eq!(Tab::Board.prev(), Tab::News);
    }
}
