use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::api::models::*;
use crate::api::NhlClient;
use tui_common::layout::cycle;

const LEADER_LIMIT: u32 = 10;
/// How long standings, schedule, and season leaders stay fresh. They change
/// about once a day; scores are always re-fetched on the live interval.
const SLOW_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Scores,
    Standings,
    Schedule,
    Leaders,
    Goalies,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Scores,
        Tab::Standings,
        Tab::Schedule,
        Tab::Leaders,
        Tab::Goalies,
    ];

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Tab::Scores)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Scores => "[1] Scores",
            Tab::Standings => "[2] Standings",
            Tab::Schedule => "[3] Schedule",
            Tab::Leaders => "[4] Skaters",
            Tab::Goalies => "[5] Goalies",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingsFilter {
    /// Division top threes plus the two wild cards, the way playoff races are
    /// normally read.
    Wildcard,
    Conference,
    Division,
    League,
}

impl StandingsFilter {
    pub const ALL: [StandingsFilter; 4] = [
        StandingsFilter::Wildcard,
        StandingsFilter::Conference,
        StandingsFilter::Division,
        StandingsFilter::League,
    ];

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StandingsFilter::Wildcard => "Wild Card",
            StandingsFilter::Conference => "Conference",
            StandingsFilter::Division => "Division",
            StandingsFilter::League => "League",
        }
    }
}

/// The categories the stats-leaders endpoint actually serves. Anything else
/// comes back as HTTP 400, so this list is the source of truth for both the
/// tab cycle and the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderCategory {
    Points,
    Goals,
    Assists,
    PlusMinus,
    PenaltyMins,
    PowerPlayGoals,
    ShorthandedGoals,
    Faceoffs,
    TimeOnIce,
}

impl LeaderCategory {
    pub const ALL: [LeaderCategory; 9] = [
        LeaderCategory::Points,
        LeaderCategory::Goals,
        LeaderCategory::Assists,
        LeaderCategory::PlusMinus,
        LeaderCategory::PenaltyMins,
        LeaderCategory::PowerPlayGoals,
        LeaderCategory::ShorthandedGoals,
        LeaderCategory::Faceoffs,
        LeaderCategory::TimeOnIce,
    ];

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LeaderCategory::Points => "Points",
            LeaderCategory::Goals => "Goals",
            LeaderCategory::Assists => "Assists",
            LeaderCategory::PlusMinus => "+/-",
            LeaderCategory::PenaltyMins => "PIM",
            LeaderCategory::PowerPlayGoals => "PP Goals",
            LeaderCategory::ShorthandedGoals => "SH Goals",
            LeaderCategory::Faceoffs => "Faceoff %",
            LeaderCategory::TimeOnIce => "TOI/GP",
        }
    }

    pub fn api_key(self) -> &'static str {
        match self {
            LeaderCategory::Points => "points",
            LeaderCategory::Goals => "goals",
            LeaderCategory::Assists => "assists",
            LeaderCategory::PlusMinus => "plusMinus",
            LeaderCategory::PenaltyMins => "penaltyMins",
            LeaderCategory::PowerPlayGoals => "goalsPp",
            LeaderCategory::ShorthandedGoals => "goalsSh",
            LeaderCategory::Faceoffs => "faceoffLeaders",
            LeaderCategory::TimeOnIce => "toi",
        }
    }

    /// Counting stats arrive as whole numbers, faceoffs as a ratio, and time
    /// on ice as seconds.
    pub fn format_value(self, value: f64) -> String {
        match self {
            LeaderCategory::Faceoffs => format!("{:.1}%", value * 100.0),
            LeaderCategory::TimeOnIce => {
                let secs = value.max(0.0).round() as u64;
                format!("{}:{:02}", secs / 60, secs % 60)
            }
            _ => format!("{}", value.round() as i64),
        }
    }
}

/// The four categories the goalie stats-leaders endpoint serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalieCategory {
    Wins,
    GoalsAgainstAverage,
    SavePercentage,
    Shutouts,
}

impl GoalieCategory {
    pub const ALL: [GoalieCategory; 4] = [
        GoalieCategory::Wins,
        GoalieCategory::GoalsAgainstAverage,
        GoalieCategory::SavePercentage,
        GoalieCategory::Shutouts,
    ];

    pub fn next(self) -> Self {
        cycle(&Self::ALL, self, 1)
    }

    pub fn prev(self) -> Self {
        cycle(&Self::ALL, self, -1)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GoalieCategory::Wins => "Wins",
            GoalieCategory::GoalsAgainstAverage => "GAA",
            GoalieCategory::SavePercentage => "Save %",
            GoalieCategory::Shutouts => "Shutouts",
        }
    }

    pub fn api_key(self) -> &'static str {
        match self {
            GoalieCategory::Wins => "wins",
            GoalieCategory::GoalsAgainstAverage => "goalsAgainstAverage",
            GoalieCategory::SavePercentage => "savePctg",
            GoalieCategory::Shutouts => "shutouts",
        }
    }

    pub fn format_value(self, value: f64) -> String {
        match self {
            // Save percentage is conventionally written .921, not 0.921.
            GoalieCategory::SavePercentage => {
                format!("{:.3}", value).trim_start_matches('0').to_string()
            }
            GoalieCategory::GoalsAgainstAverage => format!("{value:.2}"),
            _ => format!("{}", value.round() as i64),
        }
    }
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// One round of network results, handed back to the app from a background task.
#[derive(Debug)]
pub struct Fetched {
    /// Identifies the request that produced this. Results from a superseded
    /// request (the user changed the date mid-flight) are dropped.
    pub request_id: u64,
    /// `None` means the feed was still fresh and was not requested.
    pub scores: Option<Result<ScoreResponse, String>>,
    pub standings: Option<Result<StandingsResponse, String>>,
    pub schedule: Option<Result<ScheduleResponse, String>>,
    pub leaders: Option<Result<HashMap<String, Vec<StatLeader>>, String>>,
    pub goalies: Option<Result<HashMap<String, Vec<StatLeader>>, String>>,
    pub boxscore: Option<Result<BoxscoreResponse, String>>,
    pub game_stats: Option<Result<GameStats, String>>,
}

/// Which feeds a given refresh should actually request.
///
/// Scores change minute to minute; standings and season leaders change about
/// once a day. Re-fetching everything on the live interval moved roughly
/// 280 KB every 30 seconds, nearly all of it unchanged.
#[derive(Debug, Clone, Copy)]
struct Plan {
    scores: bool,
    standings: bool,
    schedule: bool,
    leaders: bool,
}

pub struct App {
    pub should_quit: bool,
    pub active_tab: Tab,
    pub current_date: NaiveDate,
    pub favorite_team: Option<String>,

    pub scores: Option<ScoreResponse>,
    pub standings: Option<StandingsResponse>,
    pub schedule: Option<ScheduleResponse>,
    pub leaders: HashMap<String, Vec<StatLeader>>,
    pub goalies: HashMap<String, Vec<StatLeader>>,
    pub boxscore: Option<BoxscoreResponse>,

    pub standings_filter: StandingsFilter,
    pub leader_category: LeaderCategory,
    pub goalie_category: GoalieCategory,
    pub scores_scroll: usize,
    pub standings_scroll: usize,
    pub schedule_scroll: usize,
    pub leaders_scroll: usize,
    pub goalies_scroll: usize,

    pub show_boxscore: bool,
    pub game_stats: Option<GameStats>,
    pub boxscore_scroll: usize,
    /// Text typed into the "go to date" prompt, or `None` when it is closed.
    pub date_input: Option<String>,
    pub show_help: bool,
    pub selected_game_id: Option<u64>,

    /// Rows the content pane last rendered, written by the UI so that page
    /// keys can move by an actual screenful. `Cell` because drawing only
    /// borrows the app immutably.
    pub viewport_rows: Cell<usize>,

    pub loading: bool,
    pub last_updated: Option<chrono::DateTime<Local>>,
    /// Most recent error, shown in the status bar. Printing to stderr would
    /// corrupt the alternate screen we are drawing on.
    pub error: Option<String>,
    /// Last seen combined score per game, used to detect that a favourite
    /// team's game changed while we were away.
    last_scores: HashMap<u64, u32>,

    /// When each slow-moving feed last arrived, for the freshness check.
    standings_at: Option<Instant>,
    schedule_at: Option<Instant>,
    leaders_at: Option<Instant>,
    /// The date the cached schedule covers.
    schedule_for: Option<NaiveDate>,

    request_id: u64,
    client: NhlClient,
}

impl App {
    pub fn new(favorite_team: Option<String>, start_tab: usize) -> Self {
        Self {
            should_quit: false,
            active_tab: Tab::from_index(start_tab),
            current_date: Local::now().date_naive(),
            favorite_team: favorite_team.map(|t| t.to_uppercase()),
            scores: None,
            standings: None,
            schedule: None,
            leaders: HashMap::new(),
            goalies: HashMap::new(),
            boxscore: None,
            standings_filter: StandingsFilter::Wildcard,
            leader_category: LeaderCategory::Points,
            goalie_category: GoalieCategory::Wins,
            scores_scroll: 0,
            standings_scroll: 0,
            schedule_scroll: 0,
            leaders_scroll: 0,
            goalies_scroll: 0,
            show_boxscore: false,
            game_stats: None,
            boxscore_scroll: 0,
            date_input: None,
            show_help: false,
            selected_game_id: None,
            viewport_rows: Cell::new(20),
            loading: false,
            last_updated: None,
            error: None,
            last_scores: HashMap::new(),
            standings_at: None,
            schedule_at: None,
            leaders_at: None,
            schedule_for: None,
            request_id: 0,
            client: NhlClient::new(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Ctrl-C quits from anywhere, including out of an overlay.
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return None;
        }

        // The date prompt is a text field, so it must claim ordinary
        // characters before any of the single-key bindings below.
        if let Some(input) = self.date_input.as_mut() {
            match key.code {
                KeyCode::Esc => self.date_input = None,
                KeyCode::Enter => {
                    let parsed = NaiveDate::parse_from_str(input, "%Y-%m-%d");
                    self.date_input = None;
                    if let Ok(date) = parsed {
                        self.current_date = date;
                        self.reset_scroll();
                        return Some(Action::Refresh);
                    }
                    self.error = Some("Could not read that date (expected YYYY-MM-DD)".into());
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                // Length caps at "YYYY-MM-DD".
                KeyCode::Char(c) if (c.is_ascii_digit() || c == '-') && input.len() < 10 => {
                    input.push(c)
                }
                _ => {}
            }
            return None;
        }

        // Overlays swallow input: Esc/q backs out of them rather than
        // quitting, which is what Esc means in most TUIs.
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter
            ) {
                self.show_help = false;
            }
            return None;
        }
        if self.show_boxscore {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                    self.show_boxscore = false;
                    self.boxscore = None;
                    self.game_stats = None;
                    self.boxscore_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => self.boxscore_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.boxscore_scroll = self.boxscore_scroll.saturating_sub(1)
                }
                KeyCode::Home | KeyCode::Char('g') => self.boxscore_scroll = 0,
                KeyCode::Char('?') => self.show_help = true,
                _ => {}
            }
            return None;
        }

        match key.code {
            // Vim's half-page scroll. These are matched before the bare
            // character bindings, or `d` would shadow Ctrl-D.
            KeyCode::Char('u') if ctrl => self.move_selection(-(self.page() as isize) / 2),
            KeyCode::Char('d') if ctrl => self.move_selection(self.page() as isize / 2),

            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => return Some(Action::ForceRefresh),

            // Tab cycling, by number and by Tab/Shift-Tab.
            KeyCode::Char(c @ '1'..='5') => {
                self.active_tab = Tab::from_index(c as usize - '1' as usize)
            }
            KeyCode::Tab => self.active_tab = self.active_tab.next(),
            KeyCode::BackTab => self.active_tab = self.active_tab.prev(),

            // `h`/`l` step the date on date-driven tabs and cycle the
            // grouping everywhere else.
            KeyCode::Left | KeyCode::Char('h') => match self.active_tab {
                Tab::Scores | Tab::Schedule => return self.shift_date(-1),
                Tab::Standings => self.standings_filter = self.standings_filter.prev(),
                Tab::Leaders => {
                    self.leader_category = self.leader_category.prev();
                    self.leaders_scroll = 0;
                }
                Tab::Goalies => {
                    self.goalie_category = self.goalie_category.prev();
                    self.goalies_scroll = 0;
                }
            },
            KeyCode::Right | KeyCode::Char('l') => match self.active_tab {
                Tab::Scores | Tab::Schedule => return self.shift_date(1),
                Tab::Standings => self.standings_filter = self.standings_filter.next(),
                Tab::Leaders => {
                    self.leader_category = self.leader_category.next();
                    self.leaders_scroll = 0;
                }
                Tab::Goalies => {
                    self.goalie_category = self.goalie_category.next();
                    self.goalies_scroll = 0;
                }
            },
            // Shift jumps a week, so browsing to a distant date does not mean
            // holding a key down; `t` returns to today.
            KeyCode::Char('H') => return self.shift_date(-7),
            KeyCode::Char('L') => return self.shift_date(7),
            // Jump to the nearest date that actually has games, which is what
            // makes the app usable in the offseason.
            KeyCode::Char('n') => {
                if let Some(date) = self.next_game_day() {
                    return self.goto(date);
                }
            }
            KeyCode::Char('p') => {
                if let Some(date) = self.previous_game_day() {
                    return self.goto(date);
                }
            }
            KeyCode::Char('d') => self.date_input = Some(String::new()),
            KeyCode::Char('t') => {
                let today = Local::now().date_naive();
                if self.current_date != today {
                    self.current_date = today;
                    self.reset_scroll();
                    return Some(Action::Refresh);
                }
            }

            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-(self.page() as isize)),
            KeyCode::PageDown => self.move_selection(self.page() as isize),
            KeyCode::Home | KeyCode::Char('g') => *self.scroll_mut() = 0,
            KeyCode::End | KeyCode::Char('G') => {
                let last = self.row_count().saturating_sub(1);
                *self.scroll_mut() = last;
            }

            KeyCode::Enter if self.active_tab == Tab::Scores => {
                if let Some(game) = self.selected_game() {
                    self.selected_game_id = Some(game.id);
                    self.show_boxscore = true;
                    self.boxscore_scroll = 0;
                    // Fetch the boxscore now rather than waiting out the
                    // refresh interval.
                    return Some(Action::Refresh);
                }
            }
            _ => {}
        }
        None
    }

    /// The nearest day on either side that actually has games.
    ///
    /// The week we already hold is checked first, because the endpoint's
    /// `nextStartDate` is the start of the next week that contains games, not
    /// the first day with games in it. In September it points at a Friday
    /// with nothing scheduled while the games start on the Saturday, and
    /// in-season it skips right over the rest of the current week.
    /// The final filter matters while a jump is still loading: the schedule
    /// on hand is the old one, and its pointer is the date already on screen.
    /// Offering it would render "Next game day" as the current day and make
    /// the key a no-op.
    pub fn next_game_day(&self) -> Option<NaiveDate> {
        let schedule = self.schedule.as_ref()?;
        self.week_day_with_games(schedule, false)
            .or_else(|| parse_date(schedule.next_start_date.as_deref()?))
            .filter(|date| *date > self.current_date)
    }

    pub fn previous_game_day(&self) -> Option<NaiveDate> {
        let schedule = self.schedule.as_ref()?;
        self.week_day_with_games(schedule, true)
            .or_else(|| parse_date(schedule.previous_start_date.as_deref()?))
            .filter(|date| *date < self.current_date)
    }

    /// The nearest day in the fetched week that has games, before or after
    /// the date on screen. `None` when the week holds nothing on that side,
    /// which is when the endpoint's own answer is the best available.
    fn week_day_with_games(
        &self,
        schedule: &crate::api::models::ScheduleResponse,
        before: bool,
    ) -> Option<NaiveDate> {
        let mut days: Vec<NaiveDate> = schedule
            .game_week
            .iter()
            .filter(|day| !day.games.is_empty())
            .filter_map(|day| parse_date(&day.date))
            .filter(|date| {
                if before {
                    *date < self.current_date
                } else {
                    *date > self.current_date
                }
            })
            .collect();
        days.sort_unstable();
        if before {
            days.pop()
        } else {
            days.into_iter().next()
        }
    }

    /// First day of the regular season, for the offseason placeholder.
    pub fn regular_season_start(&self) -> Option<NaiveDate> {
        parse_date(
            self.schedule
                .as_ref()?
                .regular_season_start_date
                .as_deref()?,
        )
    }

    fn goto(&mut self, date: NaiveDate) -> Option<Action> {
        self.current_date = date;
        self.reset_scroll();
        Some(Action::Refresh)
    }

    /// Moves the current date and asks for the data that goes with it.
    fn shift_date(&mut self, days: i64) -> Option<Action> {
        self.current_date += chrono::Duration::days(days);
        self.reset_scroll();
        Some(Action::Refresh)
    }

    /// One screenful of rows, as last rendered.
    fn page(&self) -> usize {
        self.viewport_rows.get().max(1)
    }

    fn move_selection(&mut self, delta: isize) {
        let max = self.row_count();
        if max == 0 {
            return;
        }
        let scroll = self.scroll_mut();
        let target = (*scroll as isize + delta).clamp(0, max as isize - 1);
        *scroll = target as usize;
    }

    /// Number of selectable rows on the active tab.
    fn row_count(&self) -> usize {
        match self.active_tab {
            Tab::Scores => self.scores.as_ref().map_or(0, |s| s.games.len()),
            Tab::Standings => self.filtered_standings().len(),
            Tab::Schedule => self
                .schedule
                .as_ref()
                .map_or(0, |s| s.game_week.iter().map(|d| d.games.len()).sum()),
            Tab::Leaders | Tab::Goalies => self.leader_entries().len(),
        }
    }

    fn scroll_mut(&mut self) -> &mut usize {
        match self.active_tab {
            Tab::Scores => &mut self.scores_scroll,
            Tab::Standings => &mut self.standings_scroll,
            Tab::Schedule => &mut self.schedule_scroll,
            Tab::Leaders => &mut self.leaders_scroll,
            Tab::Goalies => &mut self.goalies_scroll,
        }
    }

    fn reset_scroll(&mut self) {
        self.scores_scroll = 0;
        self.schedule_scroll = 0;
    }

    /// Keeps selections pointing at a row that still exists after a refresh
    /// returns a different number of games.
    fn clamp_scroll(&mut self) {
        // Lengths are read up front: each getter borrows `self` immutably.
        let leaders = self
            .leaders
            .get(self.leader_category.api_key())
            .map_or(0, Vec::len);
        let goalies = self
            .goalies
            .get(self.goalie_category.api_key())
            .map_or(0, Vec::len);
        let lengths = [
            self.scores.as_ref().map_or(0, |s| s.games.len()),
            self.filtered_standings().len(),
            self.schedule
                .as_ref()
                .map_or(0, |s| s.game_week.iter().map(|d| d.games.len()).sum()),
            leaders,
            goalies,
        ];
        let scrolls = [
            &mut self.scores_scroll,
            &mut self.standings_scroll,
            &mut self.schedule_scroll,
            &mut self.leaders_scroll,
            &mut self.goalies_scroll,
        ];
        for (scroll, len) in scrolls.into_iter().zip(lengths) {
            *scroll = (*scroll).min(len.saturating_sub(1));
        }
    }

    pub fn selected_game(&self) -> Option<&Game> {
        self.scores
            .as_ref()
            .and_then(|s| s.games.get(self.scores_scroll))
    }

    pub fn date_str(&self) -> String {
        self.current_date.format("%Y-%m-%d").to_string()
    }

    pub fn is_today(&self) -> bool {
        self.current_date == Local::now().date_naive()
    }

    pub fn filtered_standings(&self) -> Vec<&Standing> {
        let Some(standings) = self.standings.as_ref().map(|s| &s.standings) else {
            return Vec::new();
        };
        let mut rows: Vec<&Standing> = standings.iter().collect();
        match self.standings_filter {
            // Conference, then the two divisions' top threes, then that
            // conference's wild card race in order.
            StandingsFilter::Wildcard => {
                rows.sort_by(|a, b| {
                    let key = |s: &Standing| {
                        let wildcard = s.wildcard_sequence.unwrap_or(u32::MAX);
                        // In a division top three, wildcard_sequence is 0, so
                        // those group under the division; everyone else falls
                        // into the shared wild card block.
                        let in_division = wildcard == 0;
                        (
                            s.conference_name.clone(),
                            !in_division,
                            if in_division {
                                s.division_name.clone()
                            } else {
                                String::new()
                            },
                            if in_division {
                                s.division_sequence.unwrap_or(u32::MAX)
                            } else {
                                wildcard
                            },
                        )
                    };
                    key(a).cmp(&key(b))
                });
            }
            StandingsFilter::League => {
                rows.sort_by_key(|s| s.league_sequence.unwrap_or(u32::MAX));
            }
            // `sort_by`, not `sort_by_key`: the key is evaluated on every
            // comparison, so returning an owned String there allocated
            // hundreds of times per sort, and this runs on every frame.
            StandingsFilter::Conference => {
                // Eastern before Western, each in conference order.
                rows.sort_by(|a, b| {
                    a.conference_name.cmp(&b.conference_name).then_with(|| {
                        a.conference_sequence
                            .unwrap_or(u32::MAX)
                            .cmp(&b.conference_sequence.unwrap_or(u32::MAX))
                    })
                });
            }
            StandingsFilter::Division => {
                rows.sort_by(|a, b| {
                    a.division_name.cmp(&b.division_name).then_with(|| {
                        a.division_sequence
                            .unwrap_or(u32::MAX)
                            .cmp(&b.division_sequence.unwrap_or(u32::MAX))
                    })
                });
            }
        }
        rows
    }

    /// The heading a row sits under, or `None` when the active filter is flat.
    pub fn standings_group<'a>(&self, standing: &'a Standing) -> Option<&'a str> {
        match self.standings_filter {
            StandingsFilter::Conference => Some(&standing.conference_name),
            StandingsFilter::Division => Some(&standing.division_name),
            StandingsFilter::League => None,
            StandingsFilter::Wildcard => Some(if standing.wildcard_sequence == Some(0) {
                &standing.division_name
            } else {
                "Wild Card"
            }),
        }
    }

    /// True when a horizontal rule belongs under this row: the playoff cut,
    /// after the second wild card in each conference.
    pub fn is_playoff_cut(&self, standing: &Standing) -> bool {
        self.standings_filter == StandingsFilter::Wildcard && standing.wildcard_sequence == Some(2)
    }

    /// The leaderboard for whichever leaders tab is active. Skaters and
    /// goalies share one table; only the source and the value format differ.
    pub fn leader_entries(&self) -> &[StatLeader] {
        let (table, key) = match self.active_tab {
            Tab::Goalies => (&self.goalies, self.goalie_category.api_key()),
            _ => (&self.leaders, self.leader_category.api_key()),
        };
        table.get(key).map_or(&[], |v| v.as_slice())
    }

    pub fn leader_category_label(&self) -> &'static str {
        match self.active_tab {
            Tab::Goalies => self.goalie_category.as_str(),
            _ => self.leader_category.as_str(),
        }
    }

    pub fn format_leader_value(&self, value: f64) -> String {
        match self.active_tab {
            Tab::Goalies => self.goalie_category.format_value(value),
            _ => self.leader_category.format_value(value),
        }
    }

    pub fn is_favorite_team(&self, abbrev: &str) -> bool {
        self.favorite_team.as_deref() == Some(abbrev)
    }

    /// True when a favourite team's game has scored since the last refresh.
    ///
    /// A game we have not seen before never alerts, so opening the app during
    /// a 3-1 game is silent; only a change from a known score rings.
    fn check_score_alerts(&mut self) -> bool {
        let (Some(scores), Some(fav)) = (&self.scores, &self.favorite_team) else {
            return false;
        };
        let mut alert = false;
        let mut seen = HashMap::new();
        for game in &scores.games {
            if game.home_team.abbrev != *fav && game.away_team.abbrev != *fav {
                continue;
            }
            let total = game.home_team.score.unwrap_or(0) + game.away_team.score.unwrap_or(0);
            if self
                .last_scores
                .get(&game.id)
                .is_some_and(|prev| total > *prev)
            {
                alert = true;
            }
            seen.insert(game.id, total);
        }
        self.last_scores = seen;
        alert
    }

    /// Starts a fetch in the background and returns immediately, so the UI
    /// keeps responding to keys while the network call is in flight.
    /// Decides which feeds are stale enough to be worth requesting.
    fn plan(&self, force: bool) -> Plan {
        let stale = |at: Option<Instant>| match at {
            None => true,
            Some(at) => at.elapsed() >= SLOW_INTERVAL,
        };
        Plan {
            // Always: this is the live data, and it is keyed by date.
            scores: true,
            standings: force || self.standings.is_none() || stale(self.standings_at),
            // The schedule is a week around `current_date`, so a date change
            // invalidates it regardless of age.
            schedule: force
                || self.schedule_for != Some(self.current_date)
                || stale(self.schedule_at),
            leaders: force || stale(self.leaders_at),
        }
    }

    /// Starts a fetch in the background and returns immediately, so the UI
    /// keeps responding to keys while the network call is in flight.
    ///
    /// `force` bypasses the freshness check, for an explicit refresh.
    pub fn spawn_fetch(&mut self, tx: UnboundedSender<Action>, force: bool) {
        self.request_id += 1;
        self.loading = true;

        let plan = self.plan(force);
        let request_id = self.request_id;
        let client = self.client.clone();
        let date = self.date_str();
        let boxscore_id = self
            .show_boxscore
            .then_some(self.selected_game_id)
            .flatten();

        // Remember what this request covers, so a later plan sees it as fresh.
        if plan.schedule {
            self.schedule_for = Some(self.current_date);
        }

        tokio::spawn(async move {
            // Each arm resolves to a different payload type, so the
            // error mapping is written out rather than shared via a closure.
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
            let (scores, standings, schedule, leaders, goalies) = tokio::join!(
                feed!(plan.scores, client.get_scores(&date)),
                feed!(plan.standings, client.get_standings()),
                feed!(plan.schedule, client.get_schedule(&date)),
                feed!(plan.leaders, client.get_skater_leaders(LEADER_LIMIT)),
                feed!(plan.leaders, client.get_goalie_leaders(LEADER_LIMIT)),
            );
            // Per-period goals and shots come from a second, smaller
            // endpoint, and only while the overlay is actually open.
            let (boxscore, game_stats) = match boxscore_id {
                Some(id) => {
                    let (b, g) = tokio::join!(client.get_boxscore(id), client.get_game_stats(id));
                    (
                        Some(b.map_err(|e| e.to_string())),
                        Some(g.map_err(|e| e.to_string())),
                    )
                }
                None => (None, None),
            };
            let _ = tx.send(Action::Fetched(Box::new(Fetched {
                request_id,
                scores,
                standings,
                schedule,
                leaders,
                goalies,
                boxscore,
                game_stats,
            })));
        });
    }

    /// Folds a completed fetch into the app. Returns true if the favourite
    /// team scored.
    pub fn apply_fetch(&mut self, fetched: Fetched) -> bool {
        if fetched.request_id != self.request_id {
            return false; // Superseded by a later request.
        }
        self.loading = false;
        self.last_updated = Some(Local::now());

        let mut errors = Vec::new();
        // A closure would be monomorphic over one payload type; each slot
        // holds a different one. `None` means the feed was not requested.
        macro_rules! take {
            ($slot:expr, $result:expr, $stamp:expr) => {
                match $result {
                    Some(Ok(value)) => {
                        $slot = Some(value);
                        $stamp = Some(Instant::now());
                    }
                    Some(Err(e)) => errors.push(e),
                    None => {}
                }
            };
        }
        let mut ignored = None;
        take!(self.scores, fetched.scores, ignored);
        take!(self.standings, fetched.standings, self.standings_at);
        take!(self.schedule, fetched.schedule, self.schedule_at);
        let _ = ignored;

        match fetched.leaders {
            Some(Ok(leaders)) => {
                self.leaders = leaders;
                self.leaders_at = Some(Instant::now());
            }
            Some(Err(e)) => errors.push(e),
            None => {}
        }
        match fetched.goalies {
            Some(Ok(goalies)) => self.goalies = goalies,
            Some(Err(e)) => errors.push(e),
            None => {}
        }
        match fetched.boxscore {
            Some(Ok(boxscore)) => self.boxscore = Some(boxscore),
            Some(Err(e)) => errors.push(e),
            None => {}
        }
        match fetched.game_stats {
            Some(Ok(stats)) => self.game_stats = Some(stats),
            Some(Err(e)) => errors.push(e),
            None => {}
        }

        self.error = errors.first().cloned();
        self.clamp_scroll();
        self.check_score_alerts()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(Some("tor".into()), 0)
    }

    /// A schedule whose week holds games only on the given dates, plus the
    /// endpoint's own next/previous pointers.
    fn schedule_week(
        days_with_games: &[&str],
        empty_days: &[&str],
        next: Option<&str>,
        previous: Option<&str>,
    ) -> crate::api::models::ScheduleResponse {
        use crate::api::models::{GameDay, ScheduleResponse};
        let day = |date: &str, games: usize| GameDay {
            date: date.to_string(),
            day_abbrev: "Fri".into(),
            games: (0..games).map(|_| schedule_game()).collect(),
        };
        let mut game_week: Vec<GameDay> = empty_days.iter().map(|d| day(d, 0)).collect();
        game_week.extend(days_with_games.iter().map(|d| day(d, 2)));
        game_week.sort_by(|a, b| a.date.cmp(&b.date));
        ScheduleResponse {
            game_week,
            next_start_date: next.map(str::to_string),
            previous_start_date: previous.map(str::to_string),
            regular_season_start_date: None,
        }
    }

    fn schedule_game() -> crate::api::models::ScheduleGame {
        serde_json::from_str(
            r#"{"startTimeUTC":"2026-09-17T23:00:00Z",
                "awayTeam":{"abbrev":"TOR","placeName":{"default":"Toronto"}},
                "homeTeam":{"abbrev":"MTL","placeName":{"default":"Montreal"}}}"#,
        )
        .expect("schedule game fixture should parse")
    }

    /// The endpoint's nextStartDate is the start of the next week that has
    /// games, not the first day with games. Following it blindly skipped the
    /// rest of the current week.
    #[test]
    fn the_next_game_day_prefers_a_day_in_the_week_we_already_have() {
        let mut a = app();
        a.current_date = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        a.schedule = Some(schedule_week(
            &["2026-09-17"],
            &["2026-09-15", "2026-09-16"],
            Some("2026-09-25"),
            None,
        ));
        assert_eq!(
            a.next_game_day(),
            Some(NaiveDate::from_ymd_opt(2026, 9, 17).unwrap()),
            "should not skip past the 17th to the endpoint's own answer"
        );
    }

    #[test]
    fn the_previous_game_day_prefers_the_latest_earlier_day_in_the_week() {
        let mut a = app();
        a.current_date = NaiveDate::from_ymd_opt(2026, 9, 18).unwrap();
        a.schedule = Some(schedule_week(
            &["2026-09-15", "2026-09-17"],
            &["2026-09-18"],
            None,
            Some("2026-06-12"),
        ));
        assert_eq!(
            a.previous_game_day(),
            Some(NaiveDate::from_ymd_opt(2026, 9, 17).unwrap())
        );
    }

    /// The offseason: the whole week is empty, so the endpoint's pointer is
    /// the only thing to go on.
    #[test]
    fn an_empty_week_falls_back_to_the_endpoints_own_pointers() {
        let mut a = app();
        a.current_date = NaiveDate::from_ymd_opt(2026, 9, 4).unwrap();
        a.schedule = Some(schedule_week(
            &[],
            &["2026-09-04", "2026-09-05"],
            Some("2026-09-18"),
            Some("2026-06-12"),
        ));
        assert_eq!(
            a.next_game_day(),
            Some(NaiveDate::from_ymd_opt(2026, 9, 18).unwrap())
        );
        assert_eq!(
            a.previous_game_day(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap())
        );
    }

    /// While a jump is loading, the schedule on hand is the previous one and
    /// its pointer is the date already on screen. Offering it would print
    /// "Next game day" as today and leave the key doing nothing.
    #[test]
    fn a_stale_pointer_at_the_current_date_is_not_offered() {
        let mut a = app();
        a.current_date = NaiveDate::from_ymd_opt(2026, 9, 18).unwrap();
        a.schedule = Some(schedule_week(
            &[],
            &["2026-09-04"],
            Some("2026-09-18"),
            Some("2026-09-18"),
        ));
        assert_eq!(a.next_game_day(), None);
        assert_eq!(a.previous_game_day(), None);
    }

    /// A day with games on the far side must not be offered as the near one.
    #[test]
    fn the_current_date_is_never_offered_as_its_own_next_or_previous() {
        let mut a = app();
        a.current_date = NaiveDate::from_ymd_opt(2026, 9, 17).unwrap();
        a.schedule = Some(schedule_week(&["2026-09-17"], &[], None, None));
        assert_eq!(a.next_game_day(), None);
        assert_eq!(a.previous_game_day(), None);
    }

    fn game(id: u64, away: (&str, u32), home: (&str, u32)) -> Game {
        let team = |(abbrev, score): (&str, u32)| TeamScore {
            abbrev: abbrev.to_string(),
            score: Some(score),
        };
        Game {
            id,
            start_time_utc: None,
            game_state: "LIVE".into(),
            away_team: team(away),
            home_team: team(home),
            game_outcome: None,
            period: None,
            period_descriptor: None,
            clock: None,
        }
    }

    #[test]
    fn favorite_team_is_normalized_to_uppercase() {
        assert!(app().is_favorite_team("TOR"));
        assert!(!app().is_favorite_team("tor"));
    }

    #[test]
    fn categories_cycle_in_both_directions() {
        let first = LeaderCategory::ALL[0];
        let last = LeaderCategory::ALL[LeaderCategory::ALL.len() - 1];
        assert_eq!(first.prev(), last);
        assert_eq!(last.next(), first);

        let mut c = first;
        for _ in 0..LeaderCategory::ALL.len() {
            c = c.next();
        }
        assert_eq!(c, first, "a full cycle returns to the start");
    }

    #[test]
    fn standings_filters_cycle() {
        assert_eq!(StandingsFilter::Wildcard.prev(), StandingsFilter::League);
        assert_eq!(StandingsFilter::League.next(), StandingsFilter::Wildcard);
    }

    #[test]
    fn the_date_prompt_captures_typing_and_jumps() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::Char('d')));
        for c in "2026-03-10".chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        // Digits typed into the prompt must not be read as tab switches.
        assert_eq!(app.active_tab, Tab::Scores);

        let action = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, Some(Action::Refresh)));
        assert_eq!(app.date_str(), "2026-03-10");
        assert!(app.date_input.is_none());
    }

    #[test]
    fn the_date_prompt_rejects_nonsense_without_moving() {
        let mut app = app();
        let before = app.current_date;
        app.handle_key(KeyEvent::from(KeyCode::Char('d')));
        for c in "2026-99-99".chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.current_date, before);
        assert!(app.error.is_some(), "the user is told why nothing happened");
    }

    #[test]
    fn esc_cancels_the_date_prompt() {
        let mut app = app();
        let before = app.current_date;
        app.handle_key(KeyEvent::from(KeyCode::Char('d')));
        app.handle_key(KeyEvent::from(KeyCode::Char('1')));
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.date_input.is_none());
        assert_eq!(app.current_date, before);
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_d_still_pages_now_that_d_opens_the_prompt() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: (0..30).map(|i| game(i, ("TOR", 0), ("MTL", 0))).collect(),
        });
        app.viewport_rows.set(10);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.scores_scroll, 5);
        assert!(app.date_input.is_none(), "Ctrl-D must not open the prompt");
    }

    #[test]
    fn playoff_spots_follow_the_wildcard_sequence() {
        let team = |wildcard: Option<u32>| Standing {
            team_name: TeamName {
                default: "T".into(),
            },
            team_abbrev: TeamAbbrev {
                default: "T".into(),
            },
            conference_name: "Eastern".into(),
            division_name: "Atlantic".into(),
            games_played: 0,
            wins: 0,
            losses: 0,
            ot_losses: 0,
            points: 0,
            goal_for: 0,
            goal_against: 0,
            goal_differential: 0,
            streak_code: None,
            streak_count: None,
            division_sequence: None,
            conference_sequence: None,
            league_sequence: None,
            point_pctg: None,
            l10_wins: Some(7),
            l10_losses: Some(2),
            l10_ot_losses: Some(1),
            wildcard_sequence: wildcard,
        };
        // 0 is a division top three; 1 and 2 are the wild cards.
        assert!(team(Some(0)).in_playoff_spot());
        assert!(team(Some(2)).in_playoff_spot());
        assert!(!team(Some(3)).in_playoff_spot());
        assert!(!team(None).in_playoff_spot());
        assert_eq!(team(None).last_ten().as_deref(), Some("7-2-1"));
    }

    #[test]
    fn leader_values_are_formatted_per_category() {
        assert_eq!(LeaderCategory::Points.format_value(138.0), "138");
        assert_eq!(LeaderCategory::Faceoffs.format_value(0.630788), "63.1%");
        assert_eq!(LeaderCategory::TimeOnIce.format_value(1664.2568), "27:44");
    }

    #[test]
    fn period_labels_cover_overtime_and_shootout() {
        let pd = |number, period_type: Option<&str>| PeriodDescriptor {
            number,
            period_type: period_type.map(str::to_string),
        };
        assert_eq!(pd(1, Some("REG")).label(), "1st");
        assert_eq!(pd(4, Some("OT")).label(), "OT");
        assert_eq!(pd(5, Some("OT")).label(), "2OT");
        assert_eq!(pd(5, Some("SO")).label(), "SO");
    }

    #[test]
    fn first_sighting_of_a_game_does_not_alert() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: vec![game(1, ("TOR", 3), ("MTL", 1))],
        });
        assert!(!app.check_score_alerts(), "opening mid-game must be silent");
    }

    #[test]
    fn a_goal_by_a_tracked_game_alerts_once() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: vec![game(1, ("TOR", 3), ("MTL", 1))],
        });
        app.check_score_alerts();

        app.scores = Some(ScoreResponse {
            games: vec![game(1, ("TOR", 4), ("MTL", 1))],
        });
        assert!(app.check_score_alerts());
        assert!(!app.check_score_alerts(), "same score must not re-alert");
    }

    #[test]
    fn games_without_the_favorite_team_never_alert() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: vec![game(1, ("BOS", 1), ("MTL", 1))],
        });
        app.check_score_alerts();
        app.scores = Some(ScoreResponse {
            games: vec![game(1, ("BOS", 2), ("MTL", 1))],
        });
        assert!(!app.check_score_alerts());
    }

    #[test]
    fn selection_is_clamped_when_a_refresh_returns_fewer_games() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: (0..5).map(|i| game(i, ("TOR", 0), ("MTL", 0))).collect(),
        });
        app.scores_scroll = 4;

        app.scores = Some(ScoreResponse {
            games: vec![game(0, ("TOR", 0), ("MTL", 0))],
        });
        app.clamp_scroll();
        assert_eq!(app.scores_scroll, 0);
        assert!(app.selected_game().is_some());
    }

    #[test]
    fn stale_results_are_discarded() {
        let mut app = app();
        app.request_id = 7;
        let stale = Fetched {
            request_id: 6,
            scores: Some(Ok(ScoreResponse {
                games: vec![game(1, ("TOR", 9), ("MTL", 0))],
            })),
            standings: None,
            schedule: None,
            leaders: None,
            goalies: None,
            boxscore: None,
            game_stats: None,
        };
        app.apply_fetch(stale);
        assert!(app.scores.is_none(), "a superseded response must not land");
    }

    #[test]
    fn changing_date_resets_selection_and_requests_a_refresh() {
        let mut app = app();
        app.scores_scroll = 3;
        let start = app.current_date;
        let action = app.handle_key(KeyEvent::from(KeyCode::Char('l')));
        assert!(matches!(action, Some(Action::Refresh)));
        assert_eq!(app.current_date, start + chrono::Duration::days(1));
        assert_eq!(app.scores_scroll, 0);
    }
    #[test]
    fn goalie_values_use_hockey_conventions() {
        assert_eq!(GoalieCategory::SavePercentage.format_value(0.921), ".921");
        assert_eq!(
            GoalieCategory::GoalsAgainstAverage.format_value(2.1534),
            "2.15"
        );
        assert_eq!(GoalieCategory::Wins.format_value(39.0), "39");
    }

    #[test]
    fn tabs_cycle_with_tab_and_shift_tab() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::Goalies, "Shift-Tab wraps backwards");
        app.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::Scores);
    }

    #[test]
    fn esc_closes_overlays_but_does_not_quit() {
        let mut app = app();
        app.show_help = true;
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!app.show_help);
        assert!(!app.should_quit, "Esc must not quit from an overlay");

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!app.should_quit, "Esc is not a quit key at the top level");

        app.handle_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits_from_inside_an_overlay() {
        let mut app = app();
        app.show_boxscore = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn page_keys_move_by_a_screenful_and_stop_at_the_ends() {
        let mut app = app();
        app.scores = Some(ScoreResponse {
            games: (0..30).map(|i| game(i, ("TOR", 0), ("MTL", 0))).collect(),
        });
        app.viewport_rows.set(10);

        app.handle_key(KeyEvent::from(KeyCode::PageDown));
        assert_eq!(app.scores_scroll, 10);
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.scores_scroll, 5, "Ctrl-U is a half page");

        app.handle_key(KeyEvent::from(KeyCode::PageUp));
        assert_eq!(app.scores_scroll, 0, "clamped at the top");
        for _ in 0..10 {
            app.handle_key(KeyEvent::from(KeyCode::PageDown));
        }
        assert_eq!(app.scores_scroll, 29, "clamped at the bottom");
    }

    #[test]
    fn t_returns_to_today_from_a_browsed_date() {
        let mut app = app();
        app.handle_key(KeyEvent::from(KeyCode::Char('H')));
        assert_eq!(
            app.current_date,
            Local::now().date_naive() - chrono::Duration::days(7)
        );

        let action = app.handle_key(KeyEvent::from(KeyCode::Char('t')));
        assert!(matches!(action, Some(Action::Refresh)));
        assert!(app.is_today());
    }

    #[test]
    fn fresh_feeds_are_not_refetched_but_a_forced_refresh_takes_everything() {
        let mut app = app();
        app.standings_at = Some(Instant::now());
        app.leaders_at = Some(Instant::now());
        app.schedule_at = Some(Instant::now());
        app.schedule_for = Some(app.current_date);
        app.standings = Some(StandingsResponse { standings: vec![] });

        let plan = app.plan(false);
        assert!(plan.scores, "scores are always live");
        assert!(!plan.standings && !plan.leaders && !plan.schedule);

        let forced = app.plan(true);
        assert!(forced.standings && forced.leaders && forced.schedule);
    }

    #[test]
    fn changing_date_invalidates_the_cached_schedule() {
        let mut app = app();
        app.schedule_at = Some(Instant::now());
        app.schedule_for = Some(app.current_date);
        assert!(!app.plan(false).schedule);

        app.handle_key(KeyEvent::from(KeyCode::Char('l')));
        assert!(app.plan(false).schedule, "a new date needs a new game week");
    }
}
