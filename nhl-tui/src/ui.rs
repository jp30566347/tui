use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Tabs,
    },
    Frame,
};

use crate::api::models::PeriodDescriptor;
use crate::app::{App, StandingsFilter, Tab};
use tui_common::layout::{centered_rect, centered_size, pad_to_width, panel, scroll_offset};

const SELECTED_BG: Color = Color::DarkGray;
const SELECTED_STYLE: Style = Style::new()
    .bg(SELECTED_BG)
    .fg(Color::White)
    .add_modifier(Modifier::BOLD);
const HEADING: Style = Style::new().fg(Color::Cyan);
const MUTED: Style = Style::new().fg(Color::DarkGray);
const FAVORITE: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(f, app, chunks[0]);
    // Page keys move by a screenful, so the app needs to know how tall the
    // content pane actually is. Minus two for the panel's top and bottom border.
    app.viewport_rows
        .set(chunks[1].height.saturating_sub(2).max(1) as usize);
    match app.active_tab {
        Tab::Scores => draw_scores(f, app, chunks[1]),
        Tab::Standings => draw_standings(f, app, chunks[1]),
        Tab::Schedule => draw_schedule(f, app, chunks[1]),
        Tab::Leaders | Tab::Goalies => draw_leaders(f, app, chunks[1]),
    }
    draw_status(f, app, chunks[2]);

    if app.show_boxscore {
        draw_boxscore_overlay(f, app, area);
    }
    if app.show_help {
        draw_help_overlay(f, area);
    }
    if let Some(input) = &app.date_input {
        draw_date_prompt(f, input, area);
    }
}

/// A one-line text field for jumping to an arbitrary date. Stepping a day or
/// a week at a time cannot cross the offseason in any reasonable number of
/// keystrokes.
fn draw_date_prompt(f: &mut Frame, input: &str, area: Rect) {
    let overlay = centered_size(40, 3, area);
    f.render_widget(Clear, overlay);
    let block = Block::default()
        .title(" Go to date ")
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(overlay);
    f.render_widget(block, overlay);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(input.to_string(), Style::new().bold()),
            // A block cursor, since the real one is hidden.
            Span::styled("\u{2588}", Style::new().fg(Color::Yellow)),
            Span::styled(
                format!(
                    "{}  YYYY-MM-DD",
                    " ".repeat(10usize.saturating_sub(input.len()))
                ),
                MUTED,
            ),
        ])),
        inner,
    );
}

/// What to show when a day has no games: where the season is, and the keys
/// that get there. Without this the offseason is a blank pane.
fn no_games_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("No games on this date.", MUTED), Line::raw("")];
    if let Some(next) = app.next_game_day() {
        lines.push(Line::from(vec![
            Span::styled("Next game day: ", MUTED),
            Span::styled(next.format("%a %-d %b %Y").to_string(), FAVORITE),
            Span::styled("   press n", MUTED),
        ]));
    }
    if let Some(previous) = app.previous_game_day() {
        lines.push(Line::from(vec![
            Span::styled("Previous:      ", MUTED),
            Span::styled(previous.format("%a %-d %b %Y").to_string(), FAVORITE),
            Span::styled("   press p", MUTED),
        ]));
    }
    if let Some(start) = app.regular_season_start() {
        if app.current_date < start {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Regular season opens {}", start.format("%-d %B %Y")),
                MUTED,
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("d  jump to any date", MUTED));
    lines
}

fn draw_centered_lines(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let height = lines.len() as u16;
    let top = area.y + area.height.saturating_sub(height) / 2;
    let inner = Rect {
        y: top.min(area.y + area.height),
        height: height.min(area.height),
        ..area
    };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// "Tampa Bay Lightning (TBL)" when it fits, else "Tampa Bay Lightning", else
/// the name truncated.
///
/// The table widget cuts a cell at the column edge, which left rows reading
/// "Tampa Bay Lightning (T" once the pane narrowed. Dropping the whole
/// bracketed abbreviation reads as a choice; cutting into it reads as a bug.
fn team_label(name: &str, abbrev: &str, width: u16) -> String {
    let width = width as usize;
    let full = format!("{name} ({abbrev})");
    if full.chars().count() <= width {
        return full;
    }
    if name.chars().count() <= width {
        return name.to_string();
    }
    let mut out: String = name.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

fn placeholder(f: &mut Frame, area: Rect, text: &str, style: Style) {
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
}

fn date_title(app: &App, name: &str) -> String {
    if app.is_today() {
        format!(" {} {} (Today) ", name, app.date_str())
    } else {
        format!(" {} {} ", name, app.date_str())
    }
}

fn header_row(labels: &[&str]) -> Row<'static> {
    Row::new(
        labels
            .iter()
            .map(|l| Cell::from(l.to_string()).style(Style::new().bold())),
    )
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let tabs = Tabs::new(Tab::ALL.map(Tab::title).to_vec())
        .block(
            Block::default()
                .title(" NHL Dashboard ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(HEADING),
        )
        .select(app.active_tab.index())
        .style(Style::new().fg(Color::White))
        .highlight_style(FAVORITE);
    f.render_widget(tabs, area);
}

fn draw_scores(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        date_title(app, "Scores"),
        " \u{25C4} h/Left  |  l/Right \u{25BA}  |  j/k \u{2195}  |  Enter: details ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(scores) = &app.scores else {
        return placeholder(f, inner, "Loading\u{2026}", Style::new());
    };
    if scores.games.is_empty() {
        return draw_centered_lines(f, inner, no_games_lines(app));
    }

    let lines: Vec<Line> = scores
        .games
        .iter()
        .enumerate()
        .map(|(i, game)| {
            let selected = i == app.scores_scroll;
            let (state, state_style) = game_state_label(game);

            let team_style = |abbrev: &str| {
                if app.is_favorite_team(abbrev) {
                    FAVORITE
                } else {
                    Style::new()
                }
            };
            let score = Style::new().bold();

            let mut spans = vec![
                Span::raw(if selected { "\u{25B8} " } else { "  " }),
                Span::styled(
                    format!("{:>3} ", game.away_team.abbrev),
                    team_style(&game.away_team.abbrev),
                ),
                Span::styled(format!("{:>2}", game.away_team.score.unwrap_or(0)), score),
                Span::raw(" - "),
                Span::styled(format!("{:<2}", game.home_team.score.unwrap_or(0)), score),
                Span::styled(
                    format!("{:<3}", game.home_team.abbrev),
                    team_style(&game.home_team.abbrev),
                ),
                Span::raw("  "),
                Span::styled(state, state_style),
            ];
            if selected {
                spans.push(Span::styled("  \u{21B5} Enter", FAVORITE));
            }
            // The line style is the base that span styles patch, so the
            // selection highlight is set once here instead of on every span.
            let base = if selected {
                Style::new().bg(SELECTED_BG)
            } else {
                Style::new()
            };
            pad_to_width(Line::from(spans).style(base), inner.width)
        })
        .collect();

    let offset = scroll_offset(app.scores_scroll, inner.height as usize, lines.len());
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn game_state_label(game: &crate::api::models::Game) -> (String, Style) {
    match game.game_state.as_str() {
        // CRIT is a late, close game; the API treats it as live.
        "LIVE" | "CRIT" => {
            let period = game
                .period_descriptor
                .as_ref()
                .map(PeriodDescriptor::label)
                .unwrap_or_else(|| game.period.unwrap_or(0).to_string());
            let clock = game
                .clock
                .as_ref()
                .and_then(|c| c.time_remaining.as_deref())
                .unwrap_or("");
            (
                format!("LIVE {period} {clock}").trim_end().to_string(),
                Style::new().fg(Color::Green),
            )
        }
        "FINAL" | "OFF" => {
            let label = match game
                .game_outcome
                .as_ref()
                .and_then(|o| o.last_period_type.as_deref())
            {
                Some("OT") => "FINAL/OT",
                Some("SO") => "FINAL/SO",
                _ => "FINAL",
            };
            (label.to_string(), MUTED)
        }
        "FUT" | "PRE" => (
            start_time(game.start_time_utc.as_deref()),
            Style::new().fg(Color::Blue),
        ),
        other => (other.to_string(), Style::new().fg(Color::White)),
    }
}

/// Formats a UTC timestamp in the viewer's local timezone.
fn start_time(utc: Option<&str>) -> String {
    utc.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| "TBD".to_string())
}

/// Standings columns beyond the core set, in the order they are given up as
/// the terminal narrows. Rendering order is fixed separately below.
const OPTIONAL_COLUMNS: &[(&str, u16)] = &[
    ("P%", 6),
    ("L10", 9),
    ("STRK", 6),
    ("+/-", 5),
    ("GF", 5),
    ("GA", 5),
];
/// Indicator, #, Team, GP, W, L, OT, PTS.
/// Widest the team column ever gets. Long names lose their bracketed
/// abbreviation before they lose their letters.
const TEAM_WIDTH: u16 = 28;
const CORE_WIDTH: u16 = 2 + 3 + TEAM_WIDTH + 4 + 4 + 4 + 4 + 5;

/// One blank cell between columns, which is the table widget's default and
/// has to be budgeted for or the last column is squeezed instead.
const COLUMN_SPACING: u16 = 1;
/// Indicator, #, Team, GP, W, L, OT, PTS, plus the trailing spacer column.
const CORE_COLUMNS: u16 = 9;

/// Which optional columns fit, as a mask over `OPTIONAL_COLUMNS`.
///
/// Counting only the column widths overflowed the pane by one cell per gap,
/// and the table absorbed that by shrinking the team name until it read
/// "Tampa Bay Lightning (T". The gaps are counted here instead.
fn columns_for_width(width: u16) -> [bool; 6] {
    let mut keep = [false; 6];
    let mut used = CORE_WIDTH + COLUMN_SPACING * (CORE_COLUMNS - 1);
    for (i, (_, w)) in OPTIONAL_COLUMNS.iter().enumerate() {
        if used + w + COLUMN_SPACING <= width {
            used += w + COLUMN_SPACING;
            keep[i] = true;
        }
    }
    keep
}

fn draw_standings(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        format!(" Standings [{}] ", app.standings_filter.as_str()),
        " \u{25C4} h  |  l \u{25BA}  |  j/k \u{2195} ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let filtered = app.filtered_standings();
    if filtered.is_empty() {
        return placeholder(f, inner, "Loading\u{2026}", Style::new());
    }

    let keep = columns_for_width(inner.width);
    // Display order, independent of the order columns are dropped in.
    let order = [0usize, 1, 4, 5, 3, 2]; // P%, L10, GF, GA, +/-, STRK

    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Max(TEAM_WIDTH),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(5),
    ];
    let mut headers: Vec<&'static str> = vec!["", "#", "Team", "GP", "W", "L", "OT", "PTS"];
    for &i in &order {
        if keep[i] {
            headers.push(OPTIONAL_COLUMNS[i].0);
            widths.push(Constraint::Length(OPTIONAL_COLUMNS[i].1));
        }
    }
    widths.push(Constraint::Min(0));
    headers.push("");

    let mut rows = Vec::with_capacity(filtered.len() + 6);
    let mut selected_row = 0;
    let mut current_group: Option<String> = None;

    for (i, standing) in filtered.iter().enumerate() {
        if let Some(group) = app.standings_group(standing) {
            if current_group.as_deref() != Some(group) {
                current_group = Some(group.to_string());
                // The label goes in the Team column; the first columns are
                // only a few cells wide and would clip it.
                rows.push(Row::new(vec![
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(format!("\u{2500} {group} \u{2500}")).style(HEADING),
                ]));
            }
        }

        if i == app.standings_scroll {
            selected_row = rows.len();
        }

        let seq = match app.standings_filter {
            StandingsFilter::League => standing.league_sequence,
            StandingsFilter::Conference => standing.conference_sequence,
            StandingsFilter::Division | StandingsFilter::Wildcard => standing.division_sequence,
        }
        .unwrap_or(i as u32 + 1);
        // In the wild card block the divisional rank is meaningless; show the
        // race position instead.
        let seq = match (app.standings_filter, standing.wildcard_sequence) {
            (StandingsFilter::Wildcard, Some(w)) if w > 0 => w,
            _ => seq,
        };

        let mut cells = vec![
            Cell::from(if standing.in_playoff_spot() {
                " \u{25CF}"
            } else {
                ""
            })
            .style(Style::new().fg(Color::Green)),
            Cell::from(seq.to_string()),
            Cell::from(team_label(
                &standing.team_name.default,
                &standing.team_abbrev.default,
                TEAM_WIDTH,
            )),
            Cell::from(standing.games_played.to_string()),
            Cell::from(standing.wins.to_string()),
            Cell::from(standing.losses.to_string()),
            Cell::from(standing.ot_losses.to_string()),
            Cell::from(standing.points.to_string()),
        ];
        for &i in &order {
            if !keep[i] {
                continue;
            }
            cells.push(Cell::from(match OPTIONAL_COLUMNS[i].0 {
                "P%" => standing
                    .point_pctg
                    .map(|p| format!("{p:.3}").trim_start_matches('0').to_string())
                    .unwrap_or_default(),
                "L10" => standing.last_ten().unwrap_or_default(),
                "STRK" => format!(
                    "{}{}",
                    standing.streak_code.as_deref().unwrap_or(""),
                    standing.streak_count.unwrap_or(0)
                ),
                "+/-" => format!("{:+}", standing.goal_differential),
                "GF" => standing.goal_for.to_string(),
                "GA" => standing.goal_against.to_string(),
                _ => String::new(),
            }));
        }

        let style = if app.is_favorite_team(&standing.team_abbrev.default) {
            FAVORITE
        } else {
            Style::new()
        };
        rows.push(Row::new(cells).style(style));

        // The playoff cut: everything above this line is currently in.
        if app.is_playoff_cut(standing) {
            rows.push(Row::new(vec![
                Cell::from(""),
                Cell::from(""),
                Cell::from("\u{2504}".repeat(28)).style(MUTED),
            ]));
        }
    }

    let table = Table::new(rows, widths)
        .header(header_row(&headers))
        .row_highlight_style(SELECTED_STYLE)
        .highlight_spacing(HighlightSpacing::Always);

    // Rendering with state lets ratatui scroll the selection into view.
    let mut state = TableState::new().with_selected(Some(selected_row));
    f.render_stateful_widget(table, inner, &mut state);
}

fn draw_schedule(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        date_title(app, "Schedule"),
        " \u{25C4} h/Left  |  l/Right \u{25BA}  |  j/k \u{2195} ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(schedule) = &app.schedule else {
        return placeholder(f, inner, "Loading\u{2026}", Style::new());
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut game_idx = 0usize;
    let mut selected_line = 0usize;

    for day in &schedule.game_week {
        lines.push(Line::styled(
            format!("\u{2500} {} {} \u{2500}", day.day_abbrev, day.date),
            HEADING,
        ));
        for game in &day.games {
            let selected = game_idx == app.schedule_scroll;
            if selected {
                selected_line = lines.len();
            }
            let city = |team: &crate::api::models::ScheduleTeam| {
                let place = team
                    .place_name
                    .as_ref()
                    .map(|n| n.default.as_str())
                    .unwrap_or("");
                format!("{} {}", place, team.abbrev)
                    .trim_start()
                    .to_string()
            };
            let team_style = |abbrev: &str| {
                if app.is_favorite_team(abbrev) {
                    FAVORITE
                } else {
                    Style::new()
                }
            };

            lines.push(pad_to_width(
                Line::from(vec![
                    Span::raw(" "),
                    Span::raw(if selected { "\u{25B8}" } else { " " }),
                    Span::styled(
                        format!("{:>8}  ", start_time(Some(&game.start_time_utc))),
                        MUTED,
                    ),
                    Span::styled(city(&game.away_team), team_style(&game.away_team.abbrev)),
                    Span::raw(" @ "),
                    Span::styled(city(&game.home_team), team_style(&game.home_team.abbrev)),
                ])
                .style(if selected {
                    Style::new().bg(SELECTED_BG)
                } else {
                    Style::new()
                }),
                inner.width,
            ));
            game_idx += 1;
        }
        lines.push(Line::raw(""));
    }

    if game_idx == 0 {
        return draw_centered_lines(f, inner, no_games_lines(app));
    }

    let offset = scroll_offset(selected_line, inner.height as usize, lines.len());
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn draw_leaders(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        format!(
            " {} Leaders [{}] ",
            leaders_noun(app),
            app.leader_category_label()
        ),
        " \u{25C4} h  |  l \u{25BA}  |  j/k \u{2195} ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let entries = app.leader_entries();
    if entries.is_empty() {
        let text = if app.last_updated.is_some() {
            "No leaders for this category."
        } else {
            "Loading\u{2026}"
        };
        return placeholder(f, inner, text, MUTED);
    }

    let rows: Vec<Row> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            Row::new(vec![
                (i + 1).to_string(),
                format!("{} {}", text(&e.first_name), text(&e.last_name))
                    .trim()
                    .to_string(),
                e.position.clone().unwrap_or_default(),
                e.team_abbrev.clone().unwrap_or_default(),
                e.value
                    .map(|v| app.format_leader_value(v))
                    .unwrap_or_default(),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Max(26),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(8),
            // Absorbs leftover width so the stat columns stay beside the
            // name instead of being pushed to the far edge.
            Constraint::Min(0),
        ],
    )
    .header(header_row(&["#", "Player", "Pos", "Team", "Value", ""]))
    .row_highlight_style(SELECTED_STYLE)
    .highlight_spacing(HighlightSpacing::Always);

    let selected = if app.active_tab == Tab::Goalies {
        app.goalies_scroll
    } else {
        app.leaders_scroll
    };
    let mut state = TableState::new().with_selected(Some(selected));
    f.render_stateful_widget(table, inner, &mut state);
}

fn leaders_noun(app: &App) -> &'static str {
    if app.active_tab == Tab::Goalies {
        "Goalie"
    } else {
        "Skater"
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw("  ")];
    if let Some(updated) = &app.last_updated {
        spans.push(Span::styled(
            format!("Updated: {}  ", updated.format("%H:%M:%S")),
            MUTED,
        ));
    }
    if app.loading {
        spans.push(Span::styled(
            "\u{27F3} Loading\u{2026}  ",
            Style::new().fg(Color::Yellow),
        ));
    }
    if let Some(team) = &app.favorite_team {
        spans.push(Span::styled(format!("\u{2605} {team}  "), FAVORITE));
    }
    // Errors used to go to stderr, which is the stream this screen is drawn
    // on; showing them here keeps the display intact.
    if let Some(error) = &app.error {
        spans.push(Span::styled(
            format!("\u{26A0} {error}  "),
            Style::new().fg(Color::Red),
        ));
    }
    spans.push(Span::styled("[?]help [r]efresh [q]uit", MUTED));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_boxscore_overlay(f: &mut Frame, app: &App, area: Rect) {
    let Some(boxscore) = &app.boxscore else {
        let r = centered_rect(40, 20, area);
        f.render_widget(Clear, r);
        f.render_widget(
            Block::default()
                .title(" Loading\u{2026} ")
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::Yellow)),
            r,
        );
        return;
    };

    let overlay = centered_rect(80, 80, area);
    f.render_widget(Clear, overlay);

    let (away, home) = (&boxscore.away_team, &boxscore.home_team);
    let team_name = |team: &crate::api::models::BoxscoreTeam| {
        team.name
            .as_ref()
            .map(|n| n.default.as_str())
            .unwrap_or(&team.abbrev)
            .to_string()
    };

    let block = Block::default()
        .title(format!(
            " {} {} - {} {} ",
            team_name(away),
            away.score.unwrap_or(0),
            home.score.unwrap_or(0),
            team_name(home),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(overlay);
    f.render_widget(block, overlay);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            // Header plus a goals and a shots row per side.
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    // Goals and shots per period, straight from the API rather than derived by
    // counting the scoring summary: that mis-bucketed anything past the first
    // overtime and counted a shootout decider as a regulation goal.
    f.render_widget(Paragraph::new(line_score(app, away, home)), chunks[0]);

    let scoring = boxscore.summary.as_ref().and_then(|s| s.scoring.as_ref());

    let mut goal_lines = vec![Line::styled("   Goals:", Style::new().bold())];
    if let Some(periods) = scoring {
        for period in periods {
            // Built per goal rather than leaked: this runs on every frame.
            let period_label = period.period_descriptor.label();
            for goal in &period.goals {
                let scorer = format!(
                    "{} {}{}",
                    initial(&goal.first_name),
                    text(&goal.last_name),
                    goal.goals_to_date
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                );
                let assists = goal
                    .assists
                    .iter()
                    .map(|a| {
                        format!(
                            "{} {}{}",
                            initial(&a.first_name),
                            text(&a.last_name),
                            a.assists_to_date
                                .map(|n| format!(" ({n})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                let mut spans = vec![
                    Span::styled(format!("   {period_label:<5} "), MUTED),
                    Span::styled(format!("{:<5} ", goal.time_in_period), MUTED),
                    Span::styled(format!("{} - ", goal.team_abbrev.default), HEADING),
                    Span::styled(scorer, Style::new().bold()),
                ];
                if let Some(strength) = goal.strength.as_deref().filter(|s| *s != "ev") {
                    spans.push(Span::styled(
                        format!(" {}", strength.to_uppercase()),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                spans.push(Span::styled(
                    if assists.is_empty() {
                        "  [unassisted]".to_string()
                    } else {
                        format!("  [{assists}]")
                    },
                    MUTED,
                ));
                goal_lines.push(Line::from(spans));
            }
        }
    }
    if goal_lines.len() == 1 {
        goal_lines.push(Line::styled("   No goals.", MUTED));
    }

    if let Some(periods) = boxscore.summary.as_ref().and_then(|s| s.penalties.as_ref()) {
        let total: usize = periods.iter().map(|p| p.penalties.len()).sum();
        if total > 0 {
            goal_lines.push(Line::raw(""));
            goal_lines.push(Line::styled("   Penalties:", Style::new().bold()));
            for period in periods {
                let period_label = period.period_descriptor.label();
                for penalty in &period.penalties {
                    let who = penalty
                        .committed_by_player
                        .as_ref()
                        .map(|p| format!("{} {}", initial(&p.first_name), text(&p.last_name)))
                        .unwrap_or_default();
                    goal_lines.push(Line::from(vec![
                        Span::styled(format!("   {period_label:<5} "), MUTED),
                        Span::styled(format!("{:<5} ", penalty.time_in_period), MUTED),
                        Span::styled(
                            format!(
                                "{} - ",
                                penalty
                                    .team_abbrev
                                    .as_ref()
                                    .map(|t| t.default.as_str())
                                    .unwrap_or("")
                            ),
                            HEADING,
                        ),
                        Span::raw(who),
                        Span::styled(
                            format!(
                                " {} min, {}",
                                penalty.duration.unwrap_or(0),
                                // Slugs arrive as "delaying-game-puck-over-glass".
                                penalty
                                    .desc_key
                                    .as_deref()
                                    .unwrap_or("")
                                    .replace(['-', '_'], " ")
                            ),
                            MUTED,
                        ),
                    ]));
                }
            }
        }
    }

    if let Some(stars) = boxscore
        .summary
        .as_ref()
        .and_then(|s| s.three_stars.as_ref())
    {
        if !stars.is_empty() {
            goal_lines.push(Line::raw(""));
            goal_lines.push(Line::styled("   Three stars:", Style::new().bold()));
            for star in stars {
                goal_lines.push(Line::from(vec![
                    Span::styled(format!("   {}\u{2605}    ", star.star), FAVORITE),
                    Span::raw(format!("{:<24}", text(&star.name))),
                    Span::styled(
                        format!(
                            "{:<4}{}",
                            star.position.as_deref().unwrap_or(""),
                            star.team_abbrev.as_deref().unwrap_or("")
                        ),
                        MUTED,
                    ),
                    Span::styled(
                        match (star.goals, star.assists) {
                            (Some(g), Some(a)) => format!("   {g}G {a}A"),
                            _ => String::new(),
                        },
                        MUTED,
                    ),
                ]));
            }
        }
    }

    goal_lines.extend(team_stats_lines(app, &away.abbrev, &home.abbrev));

    // A high-scoring game produces more goals than fit, so the list scrolls.
    // The offset is clamped here rather than in the key handler, which has no
    // idea how tall the overlay is.
    let viewport = chunks[1].height as usize;
    let overflow = goal_lines.len().saturating_sub(viewport);
    let offset = app.boxscore_scroll.min(overflow);
    f.render_widget(
        Paragraph::new(goal_lines).scroll((offset as u16, 0)),
        chunks[1],
    );

    let footer = if overflow > 0 {
        format!(
            "[j/k] Scroll {}/{}  \u{2022}  [Esc] Close",
            offset + 1,
            overflow + 1
        )
    } else {
        "[Esc] Close".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::styled(footer, MUTED)).alignment(Alignment::Center),
        chunks[2],
    );
}

fn initial(name: &Option<crate::api::models::NameField>) -> String {
    name.as_ref()
        .and_then(|n| n.default.chars().next())
        .map(|c| format!("{c}."))
        .unwrap_or_default()
}

/// The value of an optional `{ "default": ... }` field, or "".
fn text(name: &Option<crate::api::models::NameField>) -> &str {
    name.as_ref().map(|n| n.default.as_str()).unwrap_or("")
}

/// The keymap, reachable with `?` from anywhere. Without this the only
/// discoverable hints are the cramped ones on the panel borders.
fn draw_help_overlay(f: &mut Frame, area: Rect) {
    const KEYS: &[(&str, &str)] = &[
        ("1 - 5", "Jump to a tab"),
        ("Tab / Shift-Tab", "Cycle tabs"),
        ("j / k, Down / Up", "Move the selection"),
        ("Ctrl-D / Ctrl-U", "Half page down / up"),
        ("PgDn / PgUp", "Full page down / up"),
        ("g / G, Home / End", "First / last row"),
        (
            "h / l, Left / Right",
            "Previous / next day, or cycle category",
        ),
        ("H / L", "Jump back / forward one week"),
        ("t", "Back to today"),
        ("n / p", "Next / previous day with games"),
        ("d", "Go to a specific date"),
        ("Enter", "Open the boxscore (Scores tab)"),
        ("r", "Refresh everything now"),
        ("?", "Toggle this help"),
        ("Esc", "Close an overlay"),
        ("q / Ctrl-C", "Quit"),
    ];

    // Two rows of chrome: the top and bottom border.
    let width = 62.min(area.width.saturating_sub(4));
    let height = (KEYS.len() as u16 + 2).min(area.height.saturating_sub(2));
    let overlay = centered_size(width, height, area);
    f.render_widget(Clear, overlay);

    let block = Block::default()
        .title(" Keys ")
        .title_alignment(Alignment::Center)
        .title_bottom(" any key to close ")
        .borders(Borders::ALL)
        .border_style(HEADING);
    let inner = block.inner(overlay);
    f.render_widget(block, overlay);

    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(format!(" {key:<20}"), FAVORITE),
                Span::raw(*description),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// The per-period grid: a goals row and a shots row per side, with a column
/// for every period the game actually reached.
fn line_score(
    app: &App,
    away: &crate::api::models::BoxscoreTeam,
    home: &crate::api::models::BoxscoreTeam,
) -> Vec<Line<'static>> {
    use crate::api::models::PeriodCount;

    let stats = app.game_stats.as_ref();
    let goals: &[PeriodCount] = stats
        .and_then(|s| s.linescore.as_ref())
        .map(|l| l.by_period.as_slice())
        .unwrap_or(&[]);
    let shots: &[PeriodCount] = stats.map(|s| s.shots_by_period.as_slice()).unwrap_or(&[]);

    // Periods come from whichever series is longer, so a game still in the
    // first period shows one column rather than a row of empty ones.
    let source = if goals.len() >= shots.len() {
        goals
    } else {
        shots
    };
    if source.is_empty() {
        return Vec::new();
    }

    let mut header = vec![Span::raw(format!("{:<9}", ""))];
    for period in source {
        header.push(Span::styled(
            format!("{:<6}", period.period_descriptor.label()),
            Style::new().bold(),
        ));
    }
    header.push(Span::styled("Total", Style::new().bold()));
    let mut lines = vec![Line::from(header)];

    let mut side = |abbrev: &str,
                    label: &str,
                    counts: &[PeriodCount],
                    pick: fn(&PeriodCount) -> u32,
                    total: Option<u32>| {
        let mut spans = vec![
            Span::styled(format!("{abbrev:<5}"), Style::new().bold()),
            Span::styled(format!("{label:<4}"), MUTED),
        ];
        let mut sum = 0;
        for period in source {
            let value = counts
                .iter()
                .find(|c| c.period_descriptor.number == period.period_descriptor.number)
                .map(pick);
            sum += value.unwrap_or(0);
            spans.push(Span::raw(format!(
                "{:<6}",
                value.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
            )));
        }
        spans.push(Span::styled(
            total.unwrap_or(sum).to_string(),
            Style::new().bold(),
        ));
        lines.push(Line::from(spans));
    };

    side(&away.abbrev, "G", goals, |c| c.away, away.score);
    side(&away.abbrev, "SOG", shots, |c| c.away, None);
    side(&home.abbrev, "G", goals, |c| c.home, home.score);
    side(&home.abbrev, "SOG", shots, |c| c.home, None);
    lines
}

/// The team stat comparison, appended below the scoring and penalty summaries.
fn team_stats_lines(app: &App, away: &str, home: &str) -> Vec<Line<'static>> {
    let Some(stats) = app.game_stats.as_ref() else {
        return Vec::new();
    };
    let rows: Vec<_> = stats
        .team_game_stats
        .iter()
        .filter_map(|stat| stat.label().map(|label| (label, stat)))
        .collect();
    if rows.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![
        Line::raw(""),
        Line::styled("   Team stats:", Style::new().bold()),
        Line::from(vec![
            Span::raw(format!("   {:<14}", "")),
            Span::styled(format!("{away:<8}"), Style::new().bold()),
            Span::styled(home.to_string(), Style::new().bold()),
        ]),
    ];
    for (label, stat) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("   {label:<14}"), MUTED),
            Span::raw(format!(
                "{:<8}",
                crate::api::models::stat_value(&stat.away_value)
            )),
            Span::raw(crate::api::models::stat_value(&stat.home_value)),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{columns_for_width, team_label, CORE_WIDTH};

    #[test]
    fn a_team_label_keeps_its_abbreviation_when_there_is_room() {
        assert_eq!(
            team_label("Tampa Bay Lightning", "TBL", 28),
            "Tampa Bay Lightning (TBL)"
        );
    }

    /// The table cuts a cell at the column edge, which used to leave rows
    /// reading "Tampa Bay Lightning (T".
    #[test]
    fn a_team_label_drops_the_whole_abbreviation_rather_than_cutting_into_it() {
        let label = team_label("Tampa Bay Lightning", "TBL", 22);
        assert_eq!(label, "Tampa Bay Lightning");
        assert!(!label.contains('('));
    }

    #[test]
    fn a_team_label_too_long_even_without_its_abbreviation_is_elided() {
        let label = team_label("Tampa Bay Lightning", "TBL", 10);
        assert_eq!(label.chars().count(), 10);
        assert!(label.ends_with('\u{2026}'));
    }

    /// The gaps between columns are real cells. Ignoring them overflowed the
    /// pane and the table paid for it by squeezing the team name.
    #[test]
    fn column_fitting_budgets_the_space_between_columns() {
        // The core columns plus their eight gaps, then exactly enough for the
        // first optional column and the gap before it.
        let exactly_one = CORE_WIDTH + 8 + 6 + 1;
        assert!(columns_for_width(exactly_one)[0]);
        assert!(!columns_for_width(exactly_one)[1]);
        // One cell short, and the gap is what does not fit.
        assert!(!columns_for_width(exactly_one - 1)[0]);
    }

    #[test]
    fn a_narrow_pane_keeps_no_optional_columns() {
        assert_eq!(columns_for_width(CORE_WIDTH), [false; 6]);
    }
}
