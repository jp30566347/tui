//! All application state, key handling, and fetch planning.
//!
//! Key handling deliberately does no IO: `handle_key` mutates state and
//! returns an optional follow-up `Action`, which is what makes every binding
//! testable without a terminal or a network.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::api::article::{Article, Block};
use crate::api::models::{Quote, Series};
use crate::api::rss::{Headline, Source};
use crate::api::MarketClient;
use crate::card::{self, Card, Ticker};
use crate::catalog::{self, format_percent, Group, Instrument, DOW_30, INSTRUMENTS};
use crate::dow;
use tui_common::layout::cycle;

/// How long news stays fresh before a tick will refetch it.
const NEWS_TTL: Duration = Duration::from_secs(300);
/// Daily closes barely move within a session, so the board's sparkline data
/// is refreshed rarely.
const HISTORY_TTL: Duration = Duration::from_secs(900);

/// How far an instrument has to move, in percent, before the Movers tab
/// carries it. A day's ordinary drift is a few tenths; a percent is the point
/// at which a move is worth a card of its own.
pub const MOVER_THRESHOLD: f64 = 1.0;

/// At most this many macro stories sit above the cards. Two is what a session
/// has: the data release, and whatever the policy story of the day is.
pub const MACRO_HEADLINES: usize = 2;

/// Terms that mark a story as macro: the releases and policy events that move
/// the whole board rather than one row of it.
const MACRO_TERMS: &[&str] = &[
    "payrolls",
    "nonfarm",
    "jobs report",
    "jobless claims",
    "unemployment",
    "job openings",
    "hiring",
    "cpi",
    "inflation",
    "ppi",
    "pce",
    "gdp",
    "retail sales",
    "fed",
    "fomc",
    "powell",
    "rate cut",
    "rate hike",
    "interest rates",
    "ecb",
    "boj",
    "tariff",
    "tariffs",
    "recession",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Movers,
    Board,
    Dow,
    News,
}

/// How the Dow tab orders its rows.
///
/// Four orders because each answers a different question: what moved the
/// index, what can move it most, where a name sits in its own year, and what
/// it did in its own terms.
/// The board row the Dow tab reconciles against. The average's own quote is
/// what makes the divisor recoverable and the contributions checkable.
pub const DOW_INDEX_SYMBOL: &str = ".DJI";

/// How far a quote's price moved today in absolute dollars. Ranking by this
/// ranks by index points, since every member shares one divisor.
fn swing(quote: &Quote) -> f64 {
    quote
        .prev_close
        .map_or(0.0, |prev| (quote.last - prev).abs())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DowSort {
    /// Biggest contribution to today's index move first, by absolute points.
    /// The default, because it is the tab's whole subject.
    Points,
    /// Highest share price first, which in a price-weighted average is the
    /// same thing as most index weight.
    Weight,
    /// Highest in its own 52-week band first.
    Range,
    /// Biggest percentage move today first.
    Move,
}

impl DowSort {
    pub const ALL: [DowSort; 4] = [
        DowSort::Points,
        DowSort::Weight,
        DowSort::Range,
        DowSort::Move,
    ];

    /// The panel's bottom hint, naming the order currently in force.
    ///
    /// One string per variant rather than a `format!`, because the block's
    /// hint borrows for the program's life.
    pub fn hint(self) -> &'static str {
        match self {
            DowSort::Points => {
                " j/k \u{2195} \u{00b7} h/l sort: points \u{00b7} r refresh \u{00b7} ? help "
            }
            DowSort::Weight => {
                " j/k \u{2195} \u{00b7} h/l sort: weight \u{00b7} r refresh \u{00b7} ? help "
            }
            DowSort::Range => {
                " j/k \u{2195} \u{00b7} h/l sort: range \u{00b7} r refresh \u{00b7} ? help "
            }
            DowSort::Move => {
                " j/k \u{2195} \u{00b7} h/l sort: move \u{00b7} r refresh \u{00b7} ? help "
            }
        }
    }

    fn step(self, delta: isize) -> Self {
        cycle(&Self::ALL, self, delta)
    }
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Movers, Tab::Board, Tab::Dow, Tab::News];

    pub fn as_str(self) -> &'static str {
        match self {
            Tab::Movers => "Movers",
            Tab::Board => "Board",
            Tab::Dow => "Dow 30",
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

/// What the reader knows about a story, keyed by its link.
#[derive(Debug)]
pub enum Story {
    Loading,
    Ready(Box<Article>),
    Failed(String),
}

/// The reader, open on one headline. The story itself lives in the cache so
/// that closing and reopening the reader costs nothing.
#[derive(Debug, Clone)]
pub struct Reader {
    pub headline: Headline,
    pub scroll: usize,
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
    /// `Some` while a story is open in the reader, over whichever view opened
    /// it.
    pub reader: Option<Reader>,
    /// Stories fetched this session, pruned to the headlines still in the
    /// pool whenever the pool is replaced.
    pub stories: HashMap<String, Story>,

    /// Parallel to `INSTRUMENTS`. `None` means never fetched, or the endpoint
    /// did not recognise the symbol.
    pub quotes: Vec<Option<Quote>>,
    /// Parallel to `DOW_30`, filled from the same quote response.
    pub dow_quotes: Vec<Option<Quote>>,
    /// Merged across feeds, deduplicated, newest first.
    pub headlines: Vec<Headline>,
    pub history: HashMap<(Range, &'static str), Series>,
    /// Keys that failed on their own. Excluded from later batches so one
    /// rotted key cannot keep poisoning the whole board request.
    pub history_bad: HashSet<&'static str>,

    pub board_selected: usize,
    /// Which cohort row the cursor is on, as an index into the sorted order
    /// the tab renders rather than into `DOW_30`.
    pub dow_selected: usize,
    pub dow_sort: DowSort,
    /// Which mover card the cursor is on, as an index into `movers()` rather
    /// than into the catalog: the list is reordered by every refresh.
    pub movers_selected: usize,
    /// Which of the macro stories above the cards is picked.
    pub movers_news_scroll: usize,
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
    /// Cards the mover grid last fitted across the pane, so `j` and `k` move
    /// by a row of a grid whose width only the renderer knows.
    pub grid_columns: Cell<usize>,
    /// Macro stories the Movers tab last had room for. The cursor cannot
    /// leave what is actually on screen.
    pub news_slots: Cell<usize>,
    /// How far the reader can scroll, as the UI last wrapped the story. The
    /// count depends on the pane width, which only the renderer knows.
    pub reader_max_scroll: Cell<usize>,
    /// The outcome of the last share, shown in the status line until the
    /// next key press. `Err` for a share that only half worked.
    pub notice: Option<Result<String, String>>,

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
            active_tab: Tab::from_index(tab).unwrap_or(Tab::Movers),
            detail: None,
            range: Range::OneMonth,
            reader: None,
            stories: HashMap::new(),
            quotes: vec![None; INSTRUMENTS.len()],
            dow_quotes: vec![None; DOW_30.len()],
            headlines: Vec::new(),
            history: HashMap::new(),
            history_bad: HashSet::new(),
            board_selected: 0,
            dow_selected: 0,
            dow_sort: DowSort::Points,
            movers_selected: 0,
            movers_news_scroll: 0,
            news_scroll: 0,
            rail_scroll: 0,
            detail_news_scroll: 0,
            news_filter: None,
            rail_all: false,
            show_help: false,
            viewport_rows: Cell::new(20),
            grid_columns: Cell::new(1),
            news_slots: Cell::new(MACRO_HEADLINES),
            reader_max_scroll: Cell::new(0),
            notice: None,
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
    /// view when it is open, otherwise whatever the front tab has selected.
    pub fn focused(&self) -> &'static Instrument {
        let at = match (self.detail, self.active_tab) {
            (Some(n), _) => n,
            (None, Tab::Movers) => self.selected_mover().unwrap_or(self.board_selected),
            (None, _) => self.board_selected,
        };
        &INSTRUMENTS[at.min(INSTRUMENTS.len() - 1)]
    }

    // --- movers ----------------------------------------------------------

    /// The board rows whose move clears the threshold, biggest first.
    pub fn movers(&self) -> Vec<usize> {
        self.ranked()
            .into_iter()
            .take_while(|n| self.change_pct(*n).abs() > MOVER_THRESHOLD)
            .collect()
    }

    // --- mega-cap cohort -------------------------------------------------

    /// Cohort rows in the order the tab draws them, as indices into
    /// `DOW_30`.
    ///
    /// Unpriced names sink to the bottom in catalog order rather than being
    /// dropped: the tab's shape has to be stable across refreshes or the
    /// cursor would wander onto a different company mid-session.
    pub fn dow_order(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..DOW_30.len()).collect();
        let sort = self.dow_sort;
        rows.sort_by(|a, b| {
            let (qa, qb) = (self.dow_quotes[*a].as_ref(), self.dow_quotes[*b].as_ref());
            match (qa, qb) {
                (Some(qa), Some(qb)) => match sort {
                    // The divisor is the same for every row, so ranking by
                    // price change ranks by index points without needing it.
                    DowSort::Points => swing(qb).total_cmp(&swing(qa)),
                    DowSort::Weight => qb.last.total_cmp(&qa.last),
                    DowSort::Range => dow::range_position(qb)
                        .unwrap_or(-1.0)
                        .total_cmp(&dow::range_position(qa).unwrap_or(-1.0)),
                    DowSort::Move => qb.change_pct.abs().total_cmp(&qa.change_pct.abs()),
                },
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then(a.cmp(b))
        });
        rows
    }

    /// The Dow's own quote, off the board, for the divisor and the header.
    ///
    /// Looked up by symbol rather than by a remembered index, so reordering
    /// the catalog cannot silently point this at a different instrument.
    pub fn dow_index_quote(&self) -> Option<&Quote> {
        let n = INSTRUMENTS
            .iter()
            .position(|i| i.cnbc == DOW_INDEX_SYMBOL)?;
        self.quotes[n].as_ref()
    }

    /// The divisor that turns member prices into index points, or `None`
    /// until every one of the thirty has a previous close.
    pub fn dow_divisor(&self) -> Option<f64> {
        dow::divisor(&self.dow_quotes, self.dow_index_quote()?)
    }

    /// Whether any headline in the pool names this company.
    ///
    /// The tab has no room for a news pane, so this is the whole of the "why"
    /// it can offer: a marker saying the session has a story about this name,
    /// and the News tab is where to read it.
    pub fn dow_in_the_news(&self, index: usize) -> bool {
        let mega = &DOW_30[index];
        self.headlines
            .iter()
            .any(|h| named(&h.haystack, mega.name, mega.aliases))
    }

    /// The cohort taken as one group, or `None` before any of it is priced.
    pub fn dow_session(&self) -> Option<dow::Session> {
        dow::session(&self.dow_quotes)
    }

    fn set_dow_selection(&mut self, to: isize) {
        let max = DOW_30.len().saturating_sub(1) as isize;
        self.dow_selected = to.clamp(0, max.max(0)) as usize;
    }

    fn cycle_dow_sort(&mut self, delta: isize) {
        self.dow_sort = self.dow_sort.step(delta);
        // The row under the cursor has moved, so the cursor goes back to the
        // top rather than following a name it was never pointing at.
        self.dow_selected = 0;
    }

    /// Every priced row by the size of its move, biggest first. Ties keep
    /// catalog order, so a quiet board still reads in its usual grouping.
    fn ranked(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..INSTRUMENTS.len())
            .filter(|n| self.quotes[*n].is_some())
            .collect();
        rows.sort_by(|a, b| {
            self.change_pct(*b)
                .abs()
                .total_cmp(&self.change_pct(*a).abs())
        });
        rows
    }

    fn change_pct(&self, n: usize) -> f64 {
        self.quotes[n].as_ref().map_or(0.0, |q| q.change_pct)
    }

    /// The largest move on the board whatever its size. A day with nothing
    /// over the threshold is itself worth saying, and saying where it was
    /// closest beats an empty pane.
    pub fn biggest_move(&self) -> Option<usize> {
        self.ranked().into_iter().next()
    }

    /// The catalog row the selected card points at.
    pub fn selected_mover(&self) -> Option<usize> {
        self.movers().get(self.movers_selected).copied()
    }

    /// The one or two stories that move the whole board rather than one row
    /// of it, with a label saying whether they are really macro.
    ///
    /// Ranked rather than filtered, so a session whose pool holds one
    /// payrolls story and nothing else macro still fills the second slot
    /// instead of leaving half the strip blank. The sort is stable, so the
    /// newest story wins inside a tier.
    pub fn macro_headlines(&self) -> (Vec<&Headline>, &'static str) {
        let mut ranked: Vec<(u8, &Headline)> =
            self.headlines.iter().map(|h| (macro_rank(h), h)).collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.truncate(MACRO_HEADLINES);
        let label = match ranked.first() {
            Some((rank, _)) if *rank < NOT_MACRO => "Macro",
            _ => "Top news",
        };
        (ranked.into_iter().map(|(_, h)| h).collect(), label)
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
            .chain(DOW_30.iter().map(|m| m.history))
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
                    for (n, mega) in DOW_30.iter().enumerate() {
                        if let Some(q) = quotes.get(mega.cnbc) {
                            self.dow_quotes[n] = Some(q.clone());
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
                self.prune_stories();
            }
        }

        self.error = errors.into_iter().next();
        self.clamp_scroll();
    }

    /// Fetches one story for the reader, off the UI thread. Separate from the
    /// feed fetch so a slow page cannot hold up the quotes, and so a refresh
    /// issued meanwhile cannot supersede it.
    pub fn spawn_story(&self, tx: UnboundedSender<Action>, link: String) {
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .get_article(&link)
                .await
                .map(Box::new)
                .map_err(|e| e.to_string());
            let _ = tx.send(Action::StoryFetched(link, result));
        });
    }

    pub fn apply_story(&mut self, link: String, result: Result<Box<Article>, String>) {
        let story = match result {
            Ok(article) => Story::Ready(article),
            Err(e) => Story::Failed(e),
        };
        self.stories.insert(link, story);
    }

    /// Drops cached stories whose headlines have left the pool, so a session
    /// left running for days does not accumulate every story it ever showed.
    fn prune_stories(&mut self) {
        let keep: HashSet<&str> = self
            .headlines
            .iter()
            .map(|h| h.link.as_str())
            .chain(self.reader.iter().map(|r| r.headline.link.as_str()))
            .collect();
        let dropped: Vec<String> = self
            .stories
            .keys()
            .filter(|k| !keep.contains(k.as_str()))
            .cloned()
            .collect();
        for link in dropped {
            self.stories.remove(&link);
        }
    }

    /// The reader's story, if it has arrived.
    pub fn story(&self) -> Option<&Story> {
        self.stories.get(&self.reader.as_ref()?.headline.link)
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
        let instruments: Vec<&Instrument> = instruments.collect();
        self.headlines
            .iter()
            .filter(|h| instruments.iter().any(|i| mentions(&h.haystack, i)))
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
        // A share's outcome stays up until the user does something else.
        self.notice = None;

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
                    self.scroll_by(page);
                    return None;
                }
                KeyCode::Char('u') => {
                    self.scroll_by(-page);
                    return None;
                }
                _ => {}
            }
        }

        if self.reader.is_some() {
            return self.handle_reader_key(key);
        }
        if self.detail.is_some() {
            return self.handle_detail_key(key);
        }

        let page = self.page() as isize;
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => return Some(Action::ForceRefresh),

            KeyCode::Char('1') => self.active_tab = Tab::Movers,
            KeyCode::Char('2') => self.active_tab = Tab::Board,
            KeyCode::Char('3') => self.active_tab = Tab::Dow,
            KeyCode::Char('4') => self.active_tab = Tab::News,
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
                Tab::Movers => self.move_card(1),
                Tab::Board => self.jump_group(1),
                Tab::Dow => self.cycle_dow_sort(1),
                Tab::News => self.cycle_news_filter(1),
            },
            KeyCode::Char('h') | KeyCode::Left => match self.active_tab {
                Tab::Movers => self.move_card(-1),
                Tab::Board => self.jump_group(-1),
                Tab::Dow => self.cycle_dow_sort(-1),
                Tab::News => self.cycle_news_filter(-1),
            },

            KeyCode::Char('n') => self.cycle_story(1),
            KeyCode::Char('N') => self.cycle_story(-1),
            KeyCode::Char('f') if self.active_tab == Tab::Board => {
                self.rail_all = !self.rail_all;
                self.rail_scroll = 0;
            }

            KeyCode::Enter => match self.active_tab {
                Tab::Movers => {
                    if let Some(n) = self.selected_mover() {
                        return self.open_detail(n);
                    }
                }
                Tab::Board => return self.open_detail(self.board_selected),
                // The detail view charts a catalog row, and a cohort row is
                // not one. Nothing to open rather than a view that would have
                // to be half built.
                Tab::Dow => {}
                Tab::News => return self.open_selected_story(),
            },
            KeyCode::Char('o') => return self.open_selected_story(),
            KeyCode::Char('c') => return self.share_selected_story(),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn handle_reader_key(&mut self, key: KeyEvent) -> Option<Action> {
        let page = self.page() as isize;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.reader = None,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_by(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.scroll_by(isize::MAX / 2),
            KeyCode::Char('c') => {
                let headline = self.reader.as_ref()?.headline.clone();
                return Some(Action::Share(Box::new(self.card_for(&headline))));
            }
            // A story that failed to load can be asked for again.
            KeyCode::Char('r') => {
                let link = self.reader.as_ref()?.headline.link.clone();
                if matches!(self.stories.get(&link), Some(Story::Failed(_)) | None) {
                    self.stories.insert(link.clone(), Story::Loading);
                    return Some(Action::FetchStory(link));
                }
            }
            _ => {}
        }
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
            KeyCode::Enter | KeyCode::Char('o') => return self.open_selected_story(),
            KeyCode::Char('c') => return self.share_selected_story(),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    /// The headline under the cursor of whichever list is in front: the
    /// detail view's, the News tab's, the Movers tab's macro strip, or the
    /// board's rail.
    fn selected_headline(&self) -> Option<Headline> {
        let (list, at) = if self.detail.is_some() {
            (self.related_headlines().0, self.detail_news_scroll)
        } else {
            match self.active_tab {
                Tab::Movers => (self.macro_headlines().0, self.movers_news_scroll),
                Tab::News => (self.filtered_headlines(), self.news_scroll),
                Tab::Board => (self.related_headlines().0, self.rail_scroll),
                // No news pane on the cohort tab, so nothing is under the
                // cursor for `o` or `c` to act on.
                Tab::Dow => (Vec::new(), 0),
            }
        };
        list.get(at).map(|h| (*h).clone())
    }

    /// Opens the detail view on a catalog row. The long chart range is only
    /// fetched once something asks for it, which is here.
    fn open_detail(&mut self, index: usize) -> Option<Action> {
        self.detail = Some(index);
        self.detail_news_scroll = 0;
        Some(Action::Refresh)
    }

    fn open_selected_story(&mut self) -> Option<Action> {
        let headline = self.selected_headline()?;
        self.open_story(headline)
    }

    /// Opens the reader on a headline, fetching the story unless it is
    /// already cached. A story that failed before is tried again.
    fn open_story(&mut self, headline: Headline) -> Option<Action> {
        let link = headline.link.clone();
        self.reader = Some(Reader {
            headline,
            scroll: 0,
        });
        self.reader_max_scroll.set(0);
        match self.stories.get(&link) {
            Some(Story::Ready(_)) | Some(Story::Loading) => None,
            Some(Story::Failed(_)) | None => {
                self.stories.insert(link.clone(), Story::Loading);
                Some(Action::FetchStory(link))
            }
        }
    }

    fn share_selected_story(&self) -> Option<Action> {
        let headline = self.selected_headline()?;
        Some(Action::Share(Box::new(self.card_for(&headline))))
    }

    /// The share card for a headline: the story's own key points and section
    /// when it has loaded, the feed's summary otherwise.
    fn card_for(&self, headline: &Headline) -> Card {
        let story = match self.stories.get(&headline.link) {
            Some(Story::Ready(article)) => Some(article.as_ref()),
            _ => None,
        };
        let points = story.map(|a| a.key_points.clone()).unwrap_or_default();
        let summary = if !headline.description.is_empty() {
            headline.description.clone()
        } else {
            story
                .and_then(|a| {
                    a.body.iter().find_map(|b| match b {
                        Block::Paragraph(p) => Some(p.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_default()
        };
        let section = story
            .and_then(|a| a.section.clone())
            .unwrap_or_else(|| headline.source.name().to_string());
        Card {
            title: headline.title.clone(),
            kicker: format!("{} \u{b7} {section}", headline.source.publisher()),
            published: story.and_then(|a| a.published).or(headline.published),
            points,
            summary,
            domain: card::domain(&headline.link),
            ticker: self.ticker_for(headline),
        }
    }

    /// The board row a story mentions, with its quote and month of closes,
    /// for the card's ticker strip. The focused instrument wins when the
    /// story mentions it, so a story opened from a row is tied to that row;
    /// otherwise the first mentioned instrument in board order. A story that
    /// mentions nothing on the board, or a row without a quote, gets none.
    fn ticker_for(&self, headline: &Headline) -> Option<Ticker> {
        let instrument = std::iter::once(self.focused())
            .chain(INSTRUMENTS.iter())
            .find(|i| mentions(&headline.haystack, i))?;
        let n = INSTRUMENTS.iter().position(|i| i.cnbc == instrument.cnbc)?;
        let quote = self.quotes[n].as_ref()?;
        let closes = instrument
            .history
            .and_then(|key| self.history.get(&(Range::OneMonth, key)))
            .map(|series| series.iter().map(|(_, v)| *v).collect())
            .unwrap_or_default();
        Some(Ticker {
            name: instrument.name.to_string(),
            level: instrument.level(quote.last),
            change: instrument.change(quote.change),
            percent: format_percent(quote.change_pct),
            up: quote.change >= 0.0,
            closes,
        })
    }

    pub fn apply_shared(&mut self, result: Result<String, String>) {
        self.notice = Some(result);
    }

    /// Scrolls whatever is in front: the reader when it is open, the list
    /// otherwise.
    fn scroll_by(&mut self, delta: isize) {
        if let Some(reader) = self.reader.as_mut() {
            let max = self.reader_max_scroll.get() as isize;
            reader.scroll = (reader.scroll as isize).saturating_add(delta).clamp(0, max) as usize;
        } else {
            self.move_selection(delta);
        }
    }

    /// One screenful of rows, as the UI last drew it.
    fn page(&self) -> usize {
        self.viewport_rows.get().max(1)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.detail.is_some() {
            let max = self.related_headlines().0.len().saturating_sub(1) as isize;
            self.detail_news_scroll =
                (self.detail_news_scroll as isize + delta).clamp(0, max.max(0)) as usize;
            return;
        }
        match self.active_tab {
            // A step down the grid is a whole row of cards, so the cursor
            // lands under the one it left rather than beside it.
            Tab::Movers => self.move_card(delta * self.grid_columns.get().max(1) as isize),
            Tab::Board => self.set_board_selection(self.board_selected as isize + delta),
            Tab::Dow => self.set_dow_selection(self.dow_selected as isize + delta),
            Tab::News => {
                let max = self.filtered_headlines().len().saturating_sub(1) as isize;
                self.news_scroll =
                    (self.news_scroll as isize + delta).clamp(0, max.max(0)) as usize;
            }
        }
    }

    /// One card left or right, which on the grid's edges is also the step
    /// from the end of a row to the start of the next.
    fn move_card(&mut self, delta: isize) {
        let max = self.movers().len().saturating_sub(1) as isize;
        self.movers_selected = (self.movers_selected as isize)
            .saturating_add(delta)
            .clamp(0, max.max(0)) as usize;
    }

    /// `n` steps the story cursor of whichever news pane is showing: the
    /// Movers tab's macro strip, or the board's rail.
    fn cycle_story(&mut self, step: isize) {
        if self.active_tab == Tab::Movers && self.detail.is_none() {
            let max = self.shown_macro_headlines().saturating_sub(1) as isize;
            self.movers_news_scroll =
                (self.movers_news_scroll as isize + step).clamp(0, max.max(0)) as usize;
        } else {
            self.rail_scroll = (self.rail_scroll as isize + step).max(0) as usize;
        }
    }

    /// Macro stories the last frame actually drew. A short terminal gets one
    /// where a tall one gets two, and the cursor may not point past it.
    fn shown_macro_headlines(&self) -> usize {
        self.news_slots.get().min(self.macro_headlines().0.len())
    }

    /// `g` and `G`: the ends of whichever list is in front.
    fn set_selection(&mut self, to: isize) {
        match self.active_tab {
            Tab::Movers => {
                let max = self.movers().len().saturating_sub(1) as isize;
                self.movers_selected = to.clamp(0, max.max(0)) as usize;
            }
            Tab::Board => self.set_board_selection(to),
            Tab::Dow => self.set_dow_selection(to),
            Tab::News => {
                let max = self.filtered_headlines().len().saturating_sub(1) as isize;
                self.news_scroll = to.clamp(0, max.max(0)) as usize;
            }
        }
    }

    fn set_board_selection(&mut self, to: isize) {
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
            self.set_board_selection(n as isize);
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
        // The mover list is rebuilt by every refresh and can shrink to
        // nothing between two frames.
        self.movers_selected = self
            .movers_selected
            .min(self.movers().len().saturating_sub(1));
        self.movers_news_scroll = self
            .movers_news_scroll
            .min(self.shown_macro_headlines().saturating_sub(1));
        self.news_scroll = self
            .news_scroll
            .min(self.filtered_headlines().len().saturating_sub(1));
        let related = self.related_headlines().0.len();
        self.rail_scroll = self.rail_scroll.min(related.saturating_sub(1));
        self.detail_news_scroll = self.detail_news_scroll.min(related.saturating_sub(1));
        if let Some(reader) = self.reader.as_mut() {
            reader.scroll = reader.scroll.min(self.reader_max_scroll.get());
        }
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

/// The rank of a story that is about nothing on the macro list.
const NOT_MACRO: u8 = 2;

/// How squarely a story is about the macro picture: the headline itself says
/// so, only its summary does, or neither.
///
/// The tiers are the difference between a story about the payrolls number and
/// a story about one borrower that mentions the Fed in passing. Both belong
/// in the news pool; only the first belongs over the day's movers.
fn macro_rank(headline: &Headline) -> u8 {
    let is_macro = |text: &str| MACRO_TERMS.iter().any(|t| contains_word(text, t));
    if is_macro(&headline.title.to_lowercase()) {
        0
    } else if is_macro(&headline.haystack) {
        1
    } else {
        NOT_MACRO
    }
}

/// Whether a headline's haystack mentions an instrument by name or alias.
fn mentions(haystack: &str, instrument: &Instrument) -> bool {
    named(haystack, instrument.name, instrument.aliases)
}

/// The same test against a bare name and alias list, so the cohort table can
/// use it without inventing a second matcher that could drift from this one.
fn named(haystack: &str, name: &str, aliases: &[&str]) -> bool {
    contains_word(haystack, &name.to_lowercase())
        || aliases.iter().any(|alias| contains_word(haystack, alias))
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

    /// The board, which most of these tests are about. The app itself opens
    /// on the Movers tab.
    fn app() -> App {
        App::new(Tab::Board.index())
    }

    /// Puts a percent move on a named instrument and returns its catalog row.
    fn with_move(a: &mut App, name: &str, pct: f64) -> usize {
        let n = INSTRUMENTS.iter().position(|i| i.name == name).unwrap();
        a.quotes[n] = Some(quote(100.0 + pct, pct));
        n
    }

    /// The Movers tab over a board with four moves on it, two of them big
    /// enough to get a card.
    fn movers_app() -> App {
        let mut a = App::new(Tab::Movers.index());
        with_move(&mut a, "Gold", 2.4);
        with_move(&mut a, "Silver", -3.1);
        with_move(&mut a, "VIX", 1.9);
        with_move(&mut a, "Copper", -1.2);
        with_move(&mut a, "S&P 500", 0.4);
        a
    }

    fn headline(title: &str, link: &str, ts: &str, source: Source) -> Headline {
        Headline {
            title: title.into(),
            link: link.into(),
            description: format!("About {title}."),
            published: DateTime::parse_from_rfc2822(ts)
                .ok()
                .map(|d| d.with_timezone(&Utc)),
            source,
            haystack: title.to_lowercase(),
        }
    }

    /// An app on the News tab with one story in the pool.
    fn news_app() -> App {
        let mut a = app();
        a.active_tab = Tab::News;
        a.headlines = vec![headline(
            "Story",
            "https://www.cnbc.com/2026/09/09/s.html",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::Economy,
        )];
        a
    }

    fn article() -> Box<Article> {
        Box::new(Article {
            title: "Story".into(),
            section: Some("Markets".into()),
            key_points: vec!["One.".into(), "Two.".into()],
            body: vec![Block::Paragraph("The body.".into())],
            ..Article::default()
        })
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
            Source::Top,
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
                    market_cap: None,
                    beta: None,
                    vol_ratio: None,
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
            market_cap: None,
            beta: None,
            vol_ratio: None,
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
            market_cap: None,
            beta: None,
            vol_ratio: None,
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
                Source::Top,
            ),
            headline(
                "new",
                "https://e.com/b",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::Economy,
            ),
            // Same story as the first, with a tracking query appended.
            headline(
                "old syndicated",
                "https://e.com/a?syn=1",
                "Fri, 04 Sep 2026 10:00:00 GMT",
                Source::Finance,
            ),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "new");
        assert_eq!(merged[1].title, "old");
    }

    #[test]
    fn undated_headlines_sort_last_rather_than_first() {
        let merged = dedupe_and_sort(vec![
            headline("undated", "https://e.com/a", "not a date", Source::Top),
            headline(
                "dated",
                "https://e.com/b",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::Top,
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
            Source::Top,
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
            Source::Top,
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
            Source::Top,
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
    fn enter_on_the_news_tab_opens_the_reader_and_asks_for_the_story() {
        let mut a = news_app();
        match a.handle_key(code(KeyCode::Enter)) {
            Some(Action::FetchStory(link)) => {
                assert_eq!(link, "https://www.cnbc.com/2026/09/09/s.html")
            }
            other => panic!("expected a FetchStory action, got {other:?}"),
        }
        assert_eq!(a.reader.as_ref().unwrap().headline.title, "Story");
        assert!(matches!(a.story(), Some(Story::Loading)));
    }

    #[test]
    fn o_opens_the_reader_from_the_board_rail_and_the_detail_view() {
        let mut a = news_app();
        a.active_tab = Tab::Board;
        a.rail_all = true;
        assert!(matches!(
            a.handle_key(key('o')),
            Some(Action::FetchStory(_))
        ));
        a.reader = None;
        a.detail = Some(0);
        assert!(
            a.handle_key(code(KeyCode::Enter)).is_none(),
            "already cached"
        );
        assert!(a.reader.is_some());
    }

    #[test]
    fn a_cached_story_opens_without_a_fetch() {
        let mut a = news_app();
        a.stories
            .insert(a.headlines[0].link.clone(), Story::Ready(article()));
        assert!(a.handle_key(code(KeyCode::Enter)).is_none());
        assert!(matches!(a.story(), Some(Story::Ready(_))));
    }

    #[test]
    fn a_story_that_failed_is_fetched_again_when_reopened_or_on_r() {
        let mut a = news_app();
        let link = a.headlines[0].link.clone();
        a.stories
            .insert(link.clone(), Story::Failed("timed out".into()));
        assert!(matches!(
            a.handle_key(code(KeyCode::Enter)),
            Some(Action::FetchStory(_))
        ));
        a.stories.insert(link, Story::Failed("timed out".into()));
        assert!(matches!(
            a.handle_key(key('r')),
            Some(Action::FetchStory(_))
        ));
        assert!(matches!(a.story(), Some(Story::Loading)));
        assert!(a.handle_key(key('r')).is_none(), "already loading");
    }

    #[test]
    fn esc_closes_the_reader_and_leaves_the_view_beneath_it_alone() {
        let mut a = news_app();
        a.handle_key(code(KeyCode::Enter));
        a.handle_key(code(KeyCode::Esc));
        assert!(a.reader.is_none());
        assert_eq!(a.active_tab, Tab::News);
        assert!(!a.should_quit);
    }

    #[test]
    fn the_reader_swallows_list_keys_and_scrolls_within_the_lines_the_ui_reported() {
        let mut a = news_app();
        a.handle_key(code(KeyCode::Enter));
        a.reader_max_scroll.set(30);
        a.viewport_rows.set(10);
        a.handle_key(key('j'));
        assert_eq!(a.reader.as_ref().unwrap().scroll, 1);
        assert_eq!(a.news_scroll, 0, "j should not have moved the list");
        a.handle_key(ctrl('d'));
        assert_eq!(a.reader.as_ref().unwrap().scroll, 6);
        a.handle_key(key('G'));
        assert_eq!(a.reader.as_ref().unwrap().scroll, 30);
        a.handle_key(key('g'));
        assert_eq!(a.reader.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn c_builds_a_card_from_the_feed_when_the_story_has_not_loaded() {
        let mut a = news_app();
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => {
                assert_eq!(card.title, "Story");
                assert_eq!(card.kicker, "CNBC \u{b7} Economy");
                assert!(card.points.is_empty());
                assert_eq!(card.summary, "About Story.");
                assert_eq!(card.domain, "cnbc.com");
            }
            other => panic!("expected a Share action, got {other:?}"),
        }
    }

    #[test]
    fn c_uses_the_story_key_points_and_section_once_it_has_loaded() {
        let mut a = news_app();
        a.stories
            .insert(a.headlines[0].link.clone(), Story::Ready(article()));
        a.handle_key(code(KeyCode::Enter));
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => {
                assert_eq!(card.points, vec!["One.", "Two."]);
                assert_eq!(card.kicker, "CNBC \u{b7} Markets");
            }
            other => panic!("expected a Share action, got {other:?}"),
        }
    }

    fn quote(last: f64, change: f64) -> Quote {
        Quote {
            last,
            change,
            change_pct: change / (last - change) * 100.0,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            year_high: None,
            year_low: None,
            market_status: None,
            market_cap: None,
            beta: None,
            vol_ratio: None,
        }
    }

    #[test]
    fn the_card_carries_the_row_the_story_mentions_with_its_quote_and_closes() {
        let mut a = news_app();
        a.headlines[0] = headline(
            "Gold hits a record as the dollar slips",
            "https://www.cnbc.com/2026/09/09/g.html",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::Top,
        );
        let gold = INSTRUMENTS.iter().position(|i| i.name == "Gold").unwrap();
        a.quotes[gold] = Some(quote(4465.70, 26.70));
        a.history.insert(
            (Range::OneMonth, INSTRUMENTS[gold].history.unwrap()),
            vec![(1, 4400.0), (2, 4420.0), (3, 4465.7)],
        );
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => {
                let ticker = card.ticker.expect("a ticker");
                assert_eq!(ticker.name, "Gold");
                assert_eq!(ticker.level, "4,465.70");
                assert!(ticker.up);
                assert_eq!(ticker.closes, vec![4400.0, 4420.0, 4465.7]);
            }
            other => panic!("expected a Share action, got {other:?}"),
        }
    }

    /// The story also mentions the dollar, but it was opened from the gold
    /// row, so the card is tied to gold.
    #[test]
    fn the_focused_row_wins_when_the_story_mentions_it() {
        let mut a = news_app();
        a.headlines[0] = headline(
            "Dollar slips as gold hits a record",
            "https://www.cnbc.com/2026/09/09/g.html",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::Top,
        );
        for (n, i) in INSTRUMENTS.iter().enumerate() {
            if i.name == "Gold" || i.name == "Dollar index" {
                a.quotes[n] = Some(quote(100.0, -1.0));
            }
        }
        a.active_tab = Tab::Board;
        a.rail_all = true;
        a.board_selected = INSTRUMENTS.iter().position(|i| i.name == "Gold").unwrap();
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => assert_eq!(card.ticker.unwrap().name, "Gold"),
            other => panic!("expected a Share action, got {other:?}"),
        }
    }

    #[test]
    fn a_story_that_mentions_no_row_or_a_row_without_a_quote_gets_no_ticker() {
        let mut a = news_app();
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => assert!(card.ticker.is_none()),
            other => panic!("expected a Share action, got {other:?}"),
        }
        a.headlines[0] = headline(
            "Gold hits a record",
            "https://www.cnbc.com/2026/09/09/g.html",
            "Fri, 04 Sep 2026 13:00:00 GMT",
            Source::Top,
        );
        match a.handle_key(key('c')) {
            Some(Action::Share(card)) => assert!(card.ticker.is_none(), "no quote yet"),
            other => panic!("expected a Share action, got {other:?}"),
        }
    }

    #[test]
    fn a_share_notice_stays_until_the_next_key() {
        let mut a = news_app();
        a.apply_shared(Ok("Copied".into()));
        assert!(a.notice.is_some());
        a.handle_key(key('j'));
        assert!(a.notice.is_none());
    }

    #[test]
    fn stories_whose_headlines_left_the_pool_are_dropped_on_refresh() {
        let mut a = news_app();
        a.stories.insert(
            "https://www.cnbc.com/gone.html".into(),
            Story::Ready(article()),
        );
        a.stories
            .insert(a.headlines[0].link.clone(), Story::Ready(article()));
        a.apply_fetch(Fetched {
            request_id: a.request_id,
            quotes: None,
            news: Some(vec![Ok(a.headlines.clone())]),
            history: None,
            long_history: None,
        });
        assert_eq!(a.stories.len(), 1);
        assert!(a
            .stories
            .contains_key("https://www.cnbc.com/2026/09/09/s.html"));
    }

    #[test]
    fn a_fetched_story_lands_in_the_cache() {
        let mut a = news_app();
        a.apply_story(
            "https://www.cnbc.com/2026/09/09/s.html".into(),
            Ok(article()),
        );
        assert!(matches!(
            a.stories.get("https://www.cnbc.com/2026/09/09/s.html"),
            Some(Story::Ready(_))
        ));
        a.apply_story("x".into(), Err("boom".into()));
        assert!(matches!(a.stories.get("x"), Some(Story::Failed(e)) if e == "boom"));
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
                    Source::Top,
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
        assert_eq!(Tab::Movers.next(), Tab::Board);
        assert_eq!(Tab::Board.next(), Tab::Dow);
        assert_eq!(Tab::Dow.next(), Tab::News);
        assert_eq!(Tab::News.next(), Tab::Movers);
        assert_eq!(Tab::Movers.prev(), Tab::News);
    }

    #[test]
    fn the_app_opens_on_the_movers_tab() {
        assert_eq!(App::new(0).active_tab, Tab::Movers);
    }

    #[test]
    fn the_number_keys_reach_every_tab() {
        let mut a = app();
        for (k, tab) in [
            ('4', Tab::News),
            ('1', Tab::Movers),
            ('3', Tab::Dow),
            ('2', Tab::Board),
        ] {
            a.handle_key(key(k));
            assert_eq!(a.active_tab, tab, "key {k}");
        }
    }

    /// The whole point of the tab: a quiet row does not get a card, and a row
    /// sitting exactly on the threshold has not cleared it.
    #[test]
    fn only_moves_over_the_threshold_get_a_card() {
        let mut a = movers_app();
        with_move(&mut a, "Brent crude", MOVER_THRESHOLD);
        let names: Vec<&str> = a.movers().iter().map(|n| INSTRUMENTS[*n].name).collect();
        assert_eq!(names, vec!["Silver", "Gold", "VIX", "Copper"]);
    }

    #[test]
    fn cards_are_ordered_by_the_size_of_the_move_whichever_way_it_went() {
        let a = movers_app();
        let first = INSTRUMENTS[a.movers()[0]].name;
        assert_eq!(first, "Silver", "the biggest move is a fall");
        assert_eq!(
            a.biggest_move().map(|n| INSTRUMENTS[n].name),
            Some("Silver")
        );
    }

    /// With nothing over the threshold the tab still has something to say.
    #[test]
    fn a_quiet_board_has_no_cards_but_still_has_a_biggest_move() {
        let mut a = App::new(Tab::Movers.index());
        with_move(&mut a, "Gold", 0.42);
        with_move(&mut a, "S&P 500", -0.1);
        assert!(a.movers().is_empty());
        assert_eq!(a.biggest_move().map(|n| INSTRUMENTS[n].name), Some("Gold"));
    }

    #[test]
    fn an_unpriced_board_has_no_biggest_move_at_all() {
        assert_eq!(App::new(Tab::Movers.index()).biggest_move(), None);
    }

    #[test]
    fn h_and_l_step_one_card_and_j_and_k_step_a_whole_grid_row() {
        let mut a = movers_app();
        a.grid_columns.set(2);
        a.handle_key(key('l'));
        assert_eq!(a.movers_selected, 1);
        a.handle_key(key('j'));
        assert_eq!(a.movers_selected, 3);
        a.handle_key(key('k'));
        assert_eq!(a.movers_selected, 1);
        a.handle_key(key('h'));
        assert_eq!(a.movers_selected, 0);
        a.handle_key(key('h'));
        assert_eq!(a.movers_selected, 0, "the cursor stops at the first card");
        a.handle_key(key('G'));
        assert_eq!(a.movers_selected, 3);
        a.handle_key(key('j'));
        assert_eq!(a.movers_selected, 3, "and at the last");
        a.handle_key(key('g'));
        assert_eq!(a.movers_selected, 0);
    }

    #[test]
    fn enter_on_a_card_opens_that_instruments_detail_view() {
        let mut a = movers_app();
        a.handle_key(key('l'));
        assert!(matches!(
            a.handle_key(code(KeyCode::Enter)),
            Some(Action::Refresh)
        ));
        assert_eq!(a.detail, INSTRUMENTS.iter().position(|i| i.name == "Gold"));
        a.handle_key(code(KeyCode::Esc));
        assert_eq!(a.active_tab, Tab::Movers);
    }

    #[test]
    fn the_selected_card_is_the_instrument_the_movers_tab_calls_focused() {
        let mut a = movers_app();
        assert_eq!(a.focused().name, "Silver");
        a.handle_key(key('l'));
        assert_eq!(a.focused().name, "Gold");
    }

    /// A refresh rebuilds the list, and a card the cursor was on can leave it.
    #[test]
    fn the_card_cursor_is_clamped_when_a_refresh_leaves_fewer_movers() {
        let mut a = movers_app();
        a.movers_selected = 3;
        with_move(&mut a, "VIX", 0.2);
        with_move(&mut a, "Copper", -0.3);
        a.clamp_scroll();
        assert_eq!(a.movers_selected, 1);
    }

    fn macro_pool() -> Vec<Headline> {
        vec![
            headline(
                "Chipmaker beats estimates",
                "https://e.com/chips",
                "Fri, 04 Sep 2026 14:00:00 GMT",
                Source::Finance,
            ),
            headline(
                "U.S. payrolls rose 162,000 in August",
                "https://e.com/jobs",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::Economy,
            ),
            headline(
                "Fed holds rates steady",
                "https://e.com/fed",
                "Fri, 04 Sep 2026 12:00:00 GMT",
                Source::Top,
            ),
            headline(
                "Inflation cooled in August",
                "https://e.com/cpi",
                "Fri, 04 Sep 2026 11:00:00 GMT",
                Source::Economy,
            ),
        ]
    }

    /// The strip is for the releases that move the whole board, not for the
    /// newest thing in the pool.
    #[test]
    fn the_macro_strip_prefers_a_data_release_over_a_company_story() {
        let mut a = App::new(Tab::Movers.index());
        a.headlines = macro_pool();
        let (picked, label) = a.macro_headlines();
        assert_eq!(label, "Macro");
        assert_eq!(picked.len(), MACRO_HEADLINES);
        assert!(
            picked[0].title.contains("payrolls"),
            "got {:?}",
            picked[0].title
        );
        assert!(picked[1].title.contains("Fed"), "got {:?}", picked[1].title);
    }

    /// A story about one borrower that nods at the Fed in its summary is not
    /// what the strip is for, however new it is.
    #[test]
    fn a_story_that_only_mentions_the_data_in_passing_ranks_below_one_about_it() {
        let mut a = App::new(Tab::Movers.index());
        let passing = Headline {
            title: "Private credit borrowers are feeling the squeeze".into(),
            link: "https://e.com/credit".into(),
            description: "Investors weigh what the Fed does next.".into(),
            published: DateTime::parse_from_rfc2822("Fri, 04 Sep 2026 15:00:00 GMT")
                .ok()
                .map(|d| d.with_timezone(&Utc)),
            source: Source::Finance,
            haystack: "private credit borrowers are feeling the squeeze investors weigh what the fed does next.".into(),
        };
        a.headlines = vec![
            passing,
            headline(
                "U.S. payrolls rose 162,000 in August",
                "https://e.com/jobs",
                "Fri, 04 Sep 2026 13:00:00 GMT",
                Source::Economy,
            ),
        ];
        let (picked, label) = a.macro_headlines();
        assert_eq!(label, "Macro");
        assert!(
            picked[0].title.contains("payrolls"),
            "got {:?}",
            picked[0].title
        );
        assert!(
            picked[1].title.contains("credit"),
            "the second slot is filled rather than left blank"
        );
    }

    #[test]
    fn a_pool_with_nothing_macro_in_it_falls_back_to_the_newest_headlines() {
        let mut a = App::new(Tab::Movers.index());
        a.headlines = vec![headline(
            "Chipmaker beats estimates",
            "https://e.com/chips",
            "Fri, 04 Sep 2026 14:00:00 GMT",
            Source::Finance,
        )];
        let (picked, label) = a.macro_headlines();
        assert_eq!(label, "Top news");
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn n_picks_a_macro_story_and_o_reads_the_one_it_is_on() {
        let mut a = App::new(Tab::Movers.index());
        a.headlines = macro_pool();
        a.news_slots.set(2);
        a.handle_key(key('n'));
        assert_eq!(a.movers_news_scroll, 1);
        match a.handle_key(key('o')) {
            Some(Action::FetchStory(link)) => assert_eq!(link, "https://e.com/fed"),
            other => panic!("expected a FetchStory action, got {other:?}"),
        }
        a.reader = None;
        a.handle_key(key('N'));
        assert_eq!(a.movers_news_scroll, 0);
    }

    /// A short terminal draws one story, so the cursor may not point at a
    /// second one the reader cannot see.
    #[test]
    fn the_story_cursor_stays_inside_what_the_terminal_had_room_for() {
        let mut a = App::new(Tab::Movers.index());
        a.headlines = macro_pool();
        a.news_slots.set(1);
        a.handle_key(key('n'));
        assert_eq!(a.movers_news_scroll, 0);
    }

    /// `n` still belongs to the board's rail everywhere else.
    #[test]
    fn n_scrolls_the_rail_on_the_board() {
        let mut a = app();
        a.headlines = macro_pool();
        a.rail_all = true;
        a.handle_key(key('n'));
        assert_eq!(a.rail_scroll, 1);
        a.handle_key(key('N'));
        assert_eq!(a.rail_scroll, 0);
        a.handle_key(key('N'));
        assert_eq!(a.rail_scroll, 0);
    }
}
