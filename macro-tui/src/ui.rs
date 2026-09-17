//! Every render function.
//!
//! Styles are `const` and use only ANSI named colours, so the app inherits
//! whatever palette the terminal is themed with rather than fighting it.

use chrono::{Local, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Clear, Dataset, GraphType, Paragraph, Tabs},
    Frame,
};

use crate::api::article::{Article, Block as Text};
use crate::api::models::{Quote, Series};
use crate::api::rss::Headline;
use crate::app::{topic, App, DowSort, Range, Story, Tab, MACRO_HEADLINES, MOVER_THRESHOLD};
use crate::catalog::{format_percent, DowStock, Group, Instrument, DOW_30, INSTRUMENTS};
use crate::dow;
use tui_common::layout::{centered_size, pad_left, pad_to_width, panel, scroll_offset, truncate};

const SELECTED_BG: Color = Color::DarkGray;
const SELECTED_STYLE: Style = Style::new()
    .bg(SELECTED_BG)
    .fg(Color::White)
    .add_modifier(Modifier::BOLD);
const HEADING: Style = Style::new().fg(Color::Cyan);
const MUTED: Style = Style::new().fg(Color::DarkGray);
const UP: Style = Style::new().fg(Color::Green);
const DOWN: Style = Style::new().fg(Color::Red);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

/// The reader's measure. Lines much longer than this are hard to track back
/// to the start of the next one, however wide the terminal.
const READER_WIDTH: usize = 92;

/// Below this width the news rail is dropped so the numbers stay readable.
const RAIL_MIN_WIDTH: u16 = 100;
const BOARD_WIDTH: u16 = 58;
/// Marker, name, value, change and percent. Whatever is left over after these
/// goes to the sparkline, so the columns stay aligned at any pane width.
const NAME_WIDTH: usize = 13;
const VALUE_WIDTH: usize = 11;
const CHANGE_WIDTH: usize = 11;
const PERCENT_WIDTH: usize = 8;
const FIXED_WIDTH: usize = 2 + NAME_WIDTH + VALUE_WIDTH + CHANGE_WIDTH + PERCENT_WIDTH;
/// A month of daily closes is about 23 points, so this is the practical
/// ceiling; a wider pane simply shows the whole series.
const SPARK_MAX: usize = 32;
/// Columns for a headline's age ("12m", "3h", "2d") and its source tag.
const AGE_WIDTH: usize = 4;
const SOURCE_WIDTH: usize = 4;

/// What a mover card is given when the terminal has the room.
const CARD_IDEAL_WIDTH: u16 = 34;
/// Past this a card is only stretched whitespace, so the grid centres its
/// cards instead of widening them.
const CARD_MAX_WIDTH: u16 = 44;
/// One blank column between cards. Rows need none: their borders already
/// separate them.
const CARD_GAP: u16 = 1;
/// Any more columns than this and a card is too narrow to read across.
const MAX_COLUMNS: usize = 6;

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
    // content pane actually is. Minus two for the panel's top and bottom
    // border, and minus the group headings the board interleaves.
    app.viewport_rows.set(
        chunks[1]
            .height
            .saturating_sub(2 + Group::ALL.len() as u16)
            .max(1) as usize,
    );

    if app.reader.is_some() {
        draw_reader(f, app, chunks[1]);
    } else if app.detail.is_some() {
        draw_detail(f, app, chunks[1]);
    } else {
        match app.active_tab {
            Tab::Movers => draw_movers(f, app, chunks[1]),
            Tab::Board => draw_board(f, app, chunks[1]),
            Tab::Dow => draw_dow(f, app, chunks[1]),
            Tab::News => draw_news(f, app, chunks[1]),
        }
    }
    draw_status(f, app, chunks[2]);

    if app.show_help {
        draw_help_overlay(f, area);
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(n, t)| Line::from(format!(" {} {} ", n + 1, t.as_str())))
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(" macro-tui "))
            .select(app.active_tab.index())
            .highlight_style(SELECTED_STYLE),
        area,
    );
}

// --- board ---------------------------------------------------------------

fn draw_board(f: &mut Frame, app: &App, area: Rect) {
    // The rail is dropped rather than squeezed: a half-width headline is
    // worse than none, and the News tab still has them all.
    let (board_area, rail_area) = if area.width >= RAIL_MIN_WIDTH {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(BOARD_WIDTH), Constraint::Min(30)])
            .split(area);
        (split[0], Some(split[1]))
    } else {
        (area, None)
    };

    draw_ticker_list(f, app, board_area);
    if let Some(rail) = rail_area {
        let (headlines, title) = app.related_headlines();
        draw_headline_pane(
            f,
            &headlines,
            app.rail_scroll,
            rail,
            &title,
            " n/N scroll \u{00b7} f all \u{00b7} o read \u{00b7} c card ",
        );
    }
}

fn draw_ticker_list(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        " Board ".into(),
        " j/k \u{2195} \u{00b7} h/l group \u{00b7} Enter detail \u{00b7} ? help ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    // One width for every row, so the trends line up in a column.
    //
    // Series lengths differ: crypto trades weekends and so has about 31 daily
    // closes in a month where the equity indices have 23. Sizing each row to
    // its own series left the shorter ones floating away from the percent
    // column, so the shortest series sets the width for all of them.
    let shortest = INSTRUMENTS
        .iter()
        .filter_map(|i| app.history.get(&(Range::OneMonth, i.history?)))
        .map(|s| s.len())
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(0);
    let spark_width = (inner.width as usize)
        .saturating_sub(FIXED_WIDTH + 1)
        .min(SPARK_MAX)
        .min(shortest);

    let mut lines: Vec<Line> = Vec::new();
    // Where the selected instrument ends up once headings are interleaved.
    let mut selected_line = 0usize;
    let mut group = None;

    for (n, instrument) in INSTRUMENTS.iter().enumerate() {
        if group != Some(instrument.group) {
            lines.push(Line::from(Span::styled(
                format!("\u{2500} {} ", instrument.group.as_str()),
                HEADING,
            )));
            group = Some(instrument.group);
        }
        if n == app.board_selected {
            selected_line = lines.len();
        }
        lines.push(ticker_row(
            app,
            instrument,
            n,
            n == app.board_selected,
            inner.width,
            spark_width,
        ));
    }

    let offset = scroll_offset(selected_line, inner.height as usize, lines.len());
    f.render_widget(
        Paragraph::new(lines[offset.min(lines.len())..].to_vec()),
        inner,
    );
}

fn ticker_row(
    app: &App,
    instrument: &Instrument,
    index: usize,
    selected: bool,
    width: u16,
    spark_width: usize,
) -> Line<'static> {
    let marker = if selected { "\u{25b8} " } else { "  " };
    let mut spans = vec![Span::raw(format!(
        "{marker}{:<NAME_WIDTH$}",
        truncate(instrument.name, NAME_WIDTH)
    ))];

    match &app.quotes[index] {
        Some(quote) => {
            let dir = if quote.change < 0.0 { DOWN } else { UP };
            spans.push(Span::raw(pad_left(
                &instrument.level(quote.last),
                VALUE_WIDTH,
            )));
            spans.push(Span::styled(
                pad_left(&instrument.change(quote.change), CHANGE_WIDTH),
                dir,
            ));
            spans.push(Span::styled(
                pad_left(&format_percent(quote.change_pct), PERCENT_WIDTH),
                dir,
            ));
            let series = app
                .history
                .get(&(Range::OneMonth, instrument.history.unwrap_or("")));
            let spark = series
                .map(|s| sparkline(s, spark_width))
                .unwrap_or_default();
            // Right-aligned so the trend ends at the pane edge rather than
            // trailing off into blank space on a wide terminal.
            spans.push(Span::styled(
                format!(" {}", pad_left(&spark, spark_width)),
                dir,
            ));
        }
        // A row the endpoint could not price keeps its place: the board's
        // shape has to be stable across refreshes or the selection would
        // wander.
        None => {
            spans.push(Span::styled(pad_left("\u{2014}", VALUE_WIDTH), MUTED));
            spans.push(Span::styled(pad_left("\u{2014}", CHANGE_WIDTH), MUTED));
            spans.push(Span::styled(pad_left("\u{2014}", PERCENT_WIDTH), MUTED));
        }
    }

    let base = if selected {
        Style::new().bg(SELECTED_BG)
    } else {
        Style::new()
    };
    pad_to_width(Line::from(spans).style(base), width)
}

// --- the Dow ------------------------------------------------------------

/// Columns for the members table. The band is fixed width so every row's fill
/// sits on the same scale and the column reads down as a distribution.
const DOW_NAME_WIDTH: usize = 19;
const DOW_PRICE_WIDTH: usize = 10;
const DOW_PCT_WIDTH: usize = 8;
const DOW_PTS_WIDTH: usize = 8;
const DOW_DD_WIDTH: usize = 8;
const DOW_VOL_WIDTH: usize = 7;

/// The Dow taken apart into the thirty names in it.
fn draw_dow(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(
        format!(
            " Dow 30 \u{00b7} members reviewed {} ",
            crate::catalog::REVIEWED
        ),
        app.dow_sort.hint(),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let divisor = app.dow_divisor();
    let mut lines: Vec<Line> = Vec::new();
    lines.extend(dow_header_lines(app, divisor));
    lines.push(Line::from(""));
    lines.push(dow_column_headings(app));

    let order = app.dow_order();
    let mut selected_line = lines.len();
    for (rank, index) in order.iter().enumerate() {
        if rank == app.dow_selected {
            selected_line = lines.len();
        }
        lines.push(dow_row(
            &DOW_30[*index],
            app.dow_quotes[*index].as_ref(),
            divisor,
            app.dow_in_the_news(*index),
            rank == app.dow_selected,
            inner.width,
        ));
    }

    let offset = scroll_offset(selected_line, inner.height as usize, lines.len());
    f.render_widget(
        Paragraph::new(lines[offset.min(lines.len())..].to_vec()),
        inner,
    );
}

/// The three lines the tab exists for: what the index did, how broadly, and
/// where the thirty sit in their own year.
fn dow_header_lines(app: &App, divisor: Option<f64>) -> Vec<Line<'static>> {
    let label = |s: &str| Span::styled(format!("  {s:<9}"), MUTED);
    let dir = |v: f64| if v < 0.0 { DOWN } else { UP };

    let mut lines = Vec::new();

    // The average itself, off the board, so the parts below have a whole to
    // add up to.
    let mut index_line = vec![label("index")];
    match app.dow_index_quote() {
        Some(q) => {
            index_line.push(Span::styled(
                crate::catalog::group_thousands(&format!("{:.2}", q.last)),
                BOLD,
            ));
            index_line.push(Span::styled(
                format!(
                    "  {}{:.0}  ",
                    if q.change < 0.0 { "\u{2212}" } else { "+" },
                    q.change.abs()
                ),
                dir(q.change),
            ));
            index_line.push(Span::styled(
                format_percent(q.change_pct),
                dir(q.change_pct),
            ));
        }
        None => index_line.push(Span::styled("\u{2014}", MUTED)),
    }
    if let Some(divisor) = divisor {
        index_line.push(Span::styled(format!("   divisor {divisor:.5}"), MUTED));
        // The divisor only moves on a split or a substitution, so a big drift
        // from the value recorded at review means this build's membership
        // list predates a reshuffle and every points figure below is off.
        if dow::divisor_drift(divisor) > dow::STALE_DRIFT {
            index_line.push(Span::styled(
                "  members may be stale",
                DOWN.add_modifier(Modifier::BOLD),
            ));
        }
    }
    lines.push(Line::from(index_line));

    let Some(session) = app.dow_session() else {
        lines.push(Line::from(Span::styled(
            "  waiting for prices\u{2026}",
            MUTED,
        )));
        return lines;
    };

    // Price-weighted against equal-weighted. The gap is the whole point: the
    // average is price-weighted, so a day carried by its dearest names shows
    // up here and nowhere else on the board.
    let mut session_line = vec![
        label("session"),
        Span::styled("price-wtd ", MUTED),
        Span::styled(
            format_percent(session.price_weighted),
            dir(session.price_weighted),
        ),
        Span::styled("   equal-wtd ", MUTED),
        Span::styled(
            format_percent(session.equal_weighted),
            dir(session.equal_weighted),
        ),
    ];
    if let Some(shape) = session.shape() {
        session_line.push(Span::styled(
            format!("   {shape}"),
            HEADING.add_modifier(Modifier::BOLD),
        ));
    }
    session_line.push(Span::styled(
        format!("   {} up / {} down", session.advancing, session.declining),
        MUTED,
    ));
    if let Some(share) = session.top_three_share {
        session_line.push(Span::styled(format!("   top 3 moved {share:.0}%"), MUTED));
    }
    lines.push(Line::from(session_line));

    let mut range_line = vec![label("range")];
    match session.median_position {
        Some(median) => range_line.push(Span::raw(format!("median {median:.0}% of 52w band"))),
        None => range_line.push(Span::styled("no 52-week band", MUTED)),
    }
    range_line.push(Span::styled(
        format!(
            "   {} of {} above {:.0}%",
            session.near_high,
            session.priced,
            dow::NEAR_HIGH
        ),
        MUTED,
    ));
    lines.push(Line::from(range_line));

    lines
}

fn dow_column_headings(app: &App) -> Line<'static> {
    let sorted = |s: String, by: DowSort| {
        if app.dow_sort == by {
            Span::styled(s, HEADING.add_modifier(Modifier::BOLD))
        } else {
            Span::styled(s, MUTED)
        }
    };
    Line::from(vec![
        Span::styled(format!("  {:<DOW_NAME_WIDTH$}", "name"), MUTED),
        sorted(pad_left("price", DOW_PRICE_WIDTH), DowSort::Weight),
        sorted(pad_left("chg%", DOW_PCT_WIDTH), DowSort::Move),
        sorted(pad_left("pts", DOW_PTS_WIDTH), DowSort::Points),
        Span::raw("  "),
        sorted(
            format!("{:<width$}", "52w range", width = dow::BAND_CELLS + 7),
            DowSort::Range,
        ),
        Span::styled(pad_left("off hi", DOW_DD_WIDTH), MUTED),
        Span::styled(pad_left("vol", DOW_VOL_WIDTH), MUTED),
    ])
}

fn dow_row(
    member: &DowStock,
    quote: Option<&Quote>,
    divisor: Option<f64>,
    in_the_news: bool,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let marker = if selected { "\u{25b8} " } else { "  " };
    // One column, after the name, so it reads down as "which of these the
    // session is talking about".
    let news = if in_the_news { "\u{00b7}" } else { " " };
    let mut spans = vec![
        Span::raw(format!(
            "{marker}{:<width$}",
            truncate(member.name, DOW_NAME_WIDTH - 2),
            width = DOW_NAME_WIDTH - 1
        )),
        Span::styled(news.to_string(), HEADING),
    ];

    match quote {
        Some(q) => {
            let dir = if q.change_pct < 0.0 { DOWN } else { UP };
            spans.push(Span::raw(pad_left(
                &crate::catalog::group_thousands(&format!("{:.2}", q.last)),
                DOW_PRICE_WIDTH,
            )));
            spans.push(Span::styled(
                pad_left(&format_percent(q.change_pct), DOW_PCT_WIDTH),
                dir,
            ));
            spans.push(Span::styled(
                pad_left(
                    &points(divisor.and_then(|d| dow::contribution(q, d))),
                    DOW_PTS_WIDTH,
                ),
                dir,
            ));
            spans.push(Span::raw("  "));
            spans.extend(band(dow::range_position(q)));
            spans.push(Span::styled(
                pad_left(&drawdown(dow::drawdown(q)), DOW_DD_WIDTH),
                MUTED,
            ));
            spans.push(volume(q.vol_ratio));
        }
        None => {
            for w in [
                DOW_PRICE_WIDTH,
                DOW_PCT_WIDTH,
                DOW_PTS_WIDTH,
                dow::BAND_CELLS + 9,
                DOW_DD_WIDTH,
                DOW_VOL_WIDTH,
            ] {
                spans.push(Span::styled(pad_left("\u{2014}", w), MUTED));
            }
        }
    }

    let base = if selected {
        Style::new().bg(SELECTED_BG)
    } else {
        Style::new()
    };
    pad_to_width(Line::from(spans).style(base), width)
}

/// A member's contribution to the index, in index points.
///
/// Whole points: the average is five figures, so a tenth of a point is below
/// anything a reader would act on and only costs column width.
fn points(contribution: Option<f64>) -> String {
    match contribution {
        Some(pts) => format!(
            "{}{:.0}",
            if pts < 0.0 { "\u{2212}" } else { "+" },
            pts.abs()
        ),
        None => "\u{2014}".into(),
    }
}

/// Where the price sits in its 52-week band, as a filled gauge and a percent.
///
/// Deliberately not colour-coded by direction: the band says where the name
/// is in its own year, which has nothing to do with today's move, and a red
/// or green gauge would invite reading it as one. The fill carries the value
/// on its own, so nothing here depends on being able to tell hues apart.
fn band(position: Option<f64>) -> Vec<Span<'static>> {
    let Some(position) = position else {
        return vec![Span::styled(
            format!("{:<width$}", "\u{2014}", width = dow::BAND_CELLS + 7),
            MUTED,
        )];
    };
    let lit = dow::band_index(position) + 1;
    vec![
        Span::styled("\u{2588}".repeat(lit), HEADING),
        Span::styled("\u{2591}".repeat(dow::BAND_CELLS - lit), MUTED),
        Span::raw(format!("{position:>5.0}% ")),
    ]
}

fn drawdown(dd: Option<f64>) -> String {
    match dd {
        Some(dd) => format!("{dd:.1}"),
        None => "\u{2014}".into(),
    }
}

/// Today's volume against the ten-day average. Only the unusual sessions are
/// worth the reader's eye, so an ordinary one stays muted.
fn volume(ratio: Option<f64>) -> Span<'static> {
    let Some(ratio) = ratio else {
        return Span::styled(pad_left("\u{2014}", DOW_VOL_WIDTH), MUTED);
    };
    let text = pad_left(&format!("{ratio:.1}x"), DOW_VOL_WIDTH);
    if ratio >= 1.5 {
        Span::styled(text, HEADING.add_modifier(Modifier::BOLD))
    } else {
        Span::styled(text, MUTED)
    }
}

// --- movers --------------------------------------------------------------

/// The day's big moves as cards, under the stories behind them.
fn draw_movers(f: &mut Frame, app: &App, area: Rect) {
    let movers = app.movers();
    let block = panel(
        format!(
            " Movers \u{00b7} {} above {MOVER_THRESHOLD:.0}% ",
            movers.len()
        ),
        if app.movers_on_news {
            " h/l story \u{00b7} j cards \u{00b7} Enter read \u{00b7} c card \u{00b7} ? help "
        } else {
            " j/k/h/l \u{2195} \u{00b7} k news \u{00b7} Enter detail \u{00b7} ? help "
        },
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Spanning exactly the grid's cards, so the two rows line up.
    // The grid keeps a column of clearance each side.
    let width = inner.width.saturating_sub(2);
    let (left, span) = if movers.is_empty() {
        (0, width)
    } else {
        grid_span(width)
    };
    let row = Rect {
        x: inner.x + 1 + left,
        width: span,
        ..inner
    };

    // The stories get their row only once the grid has enough of its own: on
    // a short terminal the prices are what the tab is for.
    let stories = app.macro_headlines();
    let mut slots = news_columns(row.width).min(stories.len());
    let mut height = story_row_height(&stories[..slots], row.width);
    while slots > 0 && height + MIN_GRID_ROWS > inner.height {
        slots -= 1;
        height = story_row_height(&stories[..slots], row.width);
    }
    app.news_slots.set(slots);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(height), Constraint::Min(3)])
        .split(inner);

    if slots > 0 {
        draw_story_row(
            f,
            &stories[..slots],
            app.movers_on_news.then_some(app.movers_news_scroll),
            Rect { height, ..row },
        );
    }
    draw_mover_grid(f, app, &movers, chunks[1]);
}

/// Rows the grid keeps under the story cards: two rows of the smallest card
/// and room to spare.
const MIN_GRID_ROWS: u16 = 8;
/// Narrower than this and a headline wraps into a column of single words.
const STORY_MIN_WIDTH: u16 = 24;

/// Story cards across a pane, up to one per macro story.
fn news_columns(width: u16) -> usize {
    (((width + CARD_GAP) / (STORY_MIN_WIDTH + CARD_GAP)) as usize).clamp(1, MACRO_HEADLINES)
}

/// Each story card's width when `count` of them share the row. The last one
/// takes what the even split left over, so the row ends where the grid under
/// it does.
fn story_widths(count: usize, width: u16) -> Vec<u16> {
    let Some(last) = count.checked_sub(1) else {
        return Vec::new();
    };
    let even = width.saturating_sub(CARD_GAP * last as u16) / count as u16;
    let rest = width.saturating_sub((even + CARD_GAP) * last as u16);
    (0..count)
        .map(|n| if n == last { rest } else { even })
        .collect()
}

/// The row is as tall as its longest headline, wrapped whole, plus the line
/// under it and the borders. Nothing is cut short.
fn story_row_height(stories: &[&Headline], width: u16) -> u16 {
    stories
        .iter()
        .zip(story_widths(stories.len(), width))
        .map(|(s, w)| wrap(&s.title, w.saturating_sub(4) as usize).len() as u16 + 3)
        .max()
        .unwrap_or(0)
}

/// The stories that moved everything, one card each, above the cards that
/// show it. `selected` is `None` while the cursor is down on the grid.
fn draw_story_row(f: &mut Frame, stories: &[&Headline], selected: Option<usize>, area: Rect) {
    let mut x = area.x;
    for (n, (story, width)) in stories
        .iter()
        .zip(story_widths(stories.len(), area.width))
        .enumerate()
    {
        let picked = selected == Some(n);
        let rect = Rect { x, width, ..area };
        x += width + CARD_GAP;
        let title = format!(
            "{} {} ",
            if picked { "\u{25b8}" } else { " " },
            truncate(topic(story), width.saturating_sub(5) as usize)
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if picked { BOLD } else { MUTED })
            .title(Span::styled(
                title,
                if picked { SELECTED_STYLE } else { HEADING },
            ));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        // A column of air inside each border, so the headline reads as a
        // paragraph rather than text pressed against a frame.
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };

        let mut lines: Vec<Line> = wrap(&story.title, inner.width as usize)
            .into_iter()
            .map(|l| Line::from(Span::styled(l, BOLD)))
            .collect();
        lines.push(Line::from(Span::styled(
            truncate(&story_meta(story), inner.width as usize),
            MUTED,
        )));
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// A story's section and age, with whatever it does not have left out rather
/// than shown as an empty field.
fn story_meta(story: &Headline) -> String {
    let mut parts = vec![story.source.name().to_string()];
    let age = age(story);
    if !age.is_empty() {
        parts.push(format!("{age} ago"));
    }
    parts.join(" \u{00b7} ")
}

fn draw_mover_grid(f: &mut Frame, app: &App, movers: &[usize], area: Rect) {
    // A column of clearance each side, so a card is never flush against the
    // pane's own border.
    let area = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    if movers.is_empty() || area.width == 0 {
        app.grid_columns.set(1);
        draw_no_movers(f, app, area);
        return;
    }

    let columns = grid_columns(area.width);
    app.grid_columns.set(columns);
    let card_width = grid_card_width(area.width);
    let size = card_size(card_width, area.height);
    let rows = (area.height / size.rows()).max(1) as usize;
    // Page keys should move by a screenful of the grid, not of the board.
    app.viewport_rows.set(rows);

    let total = movers.len().div_ceil(columns);
    let first = scroll_offset(app.movers_selected / columns, rows, total);
    let left = area.x + grid_span(area.width).0;

    for (slot, index) in movers
        .iter()
        .enumerate()
        .skip(first * columns)
        .take(rows * columns)
    {
        let rect = Rect {
            x: left + (slot % columns) as u16 * (card_width + CARD_GAP),
            y: area.y + (slot / columns - first) as u16 * size.rows(),
            width: card_width,
            height: size.rows(),
        };
        let selected = !app.movers_on_news && slot == app.movers_selected;
        draw_mover_card(f, app, *index, selected, size, rect);
    }
}

fn grid_card_width(width: u16) -> u16 {
    let columns = grid_columns(width) as u16;
    (width.saturating_sub(CARD_GAP * (columns - 1)) / columns).min(CARD_MAX_WIDTH)
}

/// Where the grid's cards start across `width`, and how wide they run. The
/// grid is centred, so the columns a wide terminal cannot fill do not all
/// pile up on one side.
fn grid_span(width: u16) -> (u16, u16) {
    let columns = grid_columns(width) as u16;
    let used = columns * grid_card_width(width) + CARD_GAP * (columns - 1);
    (width.saturating_sub(used) / 2, used)
}

/// As many ideal-width cards as the pane is nearest to fitting, so widening
/// the terminal adds a column rather than stretching the ones it has.
fn grid_columns(width: u16) -> usize {
    let pitch = CARD_IDEAL_WIDTH + CARD_GAP;
    (((width + CARD_GAP + pitch / 2) / pitch) as usize).clamp(1, MAX_COLUMNS)
}

/// How much of a card is drawn, decided by the room it has.
///
/// A narrow card drops the trend, then the move, rather than wrapping a
/// number onto a second line or clipping one mid-digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardSize {
    /// Level and percent, the move and its group, and a month of closes.
    Tall,
    /// Level and percent, the move and its group.
    Short,
    /// Level and percent.
    Tiny,
}

impl CardSize {
    /// Rows on screen, borders included.
    fn rows(self) -> u16 {
        match self {
            CardSize::Tall => 5,
            CardSize::Short => 4,
            CardSize::Tiny => 3,
        }
    }

    fn smaller(self) -> Self {
        match self {
            CardSize::Tall => CardSize::Short,
            _ => CardSize::Tiny,
        }
    }
}

fn card_size(card_width: u16, height: u16) -> CardSize {
    let mut size = if card_width >= 28 {
        CardSize::Tall
    } else if card_width >= 22 {
        CardSize::Short
    } else {
        CardSize::Tiny
    };
    // A card taller than the pane would leave the grid showing nothing at
    // all. A pane with room for only one row of tall cards shows two rows of
    // short ones instead: on a short terminal, how much of the day is on
    // screen is worth more than the trend under each price.
    while size != CardSize::Tiny
        && (size.rows() > height
            || (height / size.rows() < 2 && height / size.smaller().rows() >= 2))
    {
        size = size.smaller();
    }
    size
}

fn draw_mover_card(
    f: &mut Frame,
    app: &App,
    index: usize,
    selected: bool,
    size: CardSize,
    area: Rect,
) {
    let instrument = &INSTRUMENTS[index];
    // Only priced rows can be movers, so this is a formality.
    let Some(quote) = app.quotes[index].as_ref() else {
        return;
    };
    let up = quote.change_pct >= 0.0;
    let dir = if up { UP } else { DOWN };

    // The name rides the top border, which buys the card a row of content.
    // The marker takes a column the unselected cards also leave blank, so
    // moving the cursor does not shunt every name sideways.
    let title = format!(
        "{} {} ",
        if selected { "\u{25b8}" } else { " " },
        truncate(instrument.name, area.width.saturating_sub(5) as usize)
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if selected { BOLD } else { MUTED })
        .title(Span::styled(
            title,
            if selected { SELECTED_STYLE } else { BOLD },
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let width = inner.width as usize;
    let mut lines = vec![spread(
        Span::styled(instrument.level(quote.last), BOLD),
        Span::styled(
            format!(
                "{} {}",
                if up { "\u{25b2}" } else { "\u{25bc}" },
                format_percent(quote.change_pct)
            ),
            dir.add_modifier(Modifier::BOLD),
        ),
        width,
    )];

    if size != CardSize::Tiny {
        lines.push(spread(
            Span::styled(instrument.change(quote.change), dir),
            Span::styled(instrument.group.as_str(), MUTED),
            width,
        ));
    }
    if size == CardSize::Tall {
        let spark = instrument
            .history
            .and_then(|key| app.history.get(&(Range::OneMonth, key)))
            .map(|series| sparkline(series, width))
            .unwrap_or_default();
        // Right-aligned, so the trend ends under the percent it explains.
        lines.push(Line::from(Span::styled(pad_left(&spark, width), dir)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Two spans pushed to the edges of one line. The right one is dropped rather
/// than overlapped when the card is too narrow to hold both.
fn spread(left: Span<'static>, right: Span<'static>, width: usize) -> Line<'static> {
    let used = left.content.chars().count() + right.content.chars().count();
    if used >= width {
        return Line::from(left);
    }
    Line::from(vec![left, Span::raw(" ".repeat(width - used)), right])
}

/// A board with nothing over the threshold is itself the news, so the tab
/// says so and points at the largest move there is.
fn draw_no_movers(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from(Span::styled(
        format!("Nothing has moved more than {MOVER_THRESHOLD:.0}% today."),
        MUTED,
    ))];
    match app.biggest_move().and_then(|n| {
        app.quotes[n]
            .as_ref()
            .map(|q| (INSTRUMENTS[n].name, q.change_pct))
    }) {
        Some((name, pct)) => lines.push(Line::from(vec![
            Span::styled("Biggest so far: ", MUTED),
            Span::raw(name),
            Span::raw(" "),
            Span::styled(format_percent(pct), if pct < 0.0 { DOWN } else { UP }),
        ])),
        None => lines.push(Line::from(Span::styled(
            "Waiting for the first quotes\u{2026}",
            MUTED,
        ))),
    }
    let rect = centered_size(area.width, lines.len() as u16, area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rect);
}

// --- news ----------------------------------------------------------------

fn draw_news(f: &mut Frame, app: &App, area: Rect) {
    let headlines = app.filtered_headlines();
    let section = app
        .news_filter
        .map(|s| s.name().to_string())
        .unwrap_or_else(|| "all sections".into());
    draw_headline_pane(
        f,
        &headlines,
        app.news_scroll,
        area,
        &format!("News \u{00b7} {section}"),
        " j/k \u{2195} \u{00b7} h/l section \u{00b7} Enter read \u{00b7} c card ",
    );
}

fn draw_headline_pane(
    f: &mut Frame,
    headlines: &[&Headline],
    selected: usize,
    area: Rect,
    title: &str,
    hint: &'static str,
) {
    let block = panel(format!(" {title} "), hint);
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_headline_list(f, headlines, selected, inner);
}

/// The headlines themselves, with no frame of their own, so a caller that is
/// already inside a bordered pane does not end up with two boxes.
fn draw_headline_list(f: &mut Frame, headlines: &[&Headline], selected: usize, inner: Rect) {
    if headlines.is_empty() {
        f.render_widget(
            Paragraph::new("No headlines yet.")
                .alignment(Alignment::Center)
                .style(MUTED),
            inner,
        );
        return;
    }

    // One line per headline. Two lines each would halve how much of the feed
    // is visible, and the age and source are short enough to share the row.
    let lines: Vec<Line> = headlines
        .iter()
        .enumerate()
        .map(|(n, h)| {
            let picked = n == selected;
            let base = if picked {
                Style::new().bg(SELECTED_BG)
            } else {
                Style::new()
            };
            let meta_width = 2 + AGE_WIDTH + 1 + SOURCE_WIDTH + 1;
            let title = truncate(&h.title, (inner.width as usize).saturating_sub(meta_width));
            pad_to_width(
                Line::from(vec![
                    Span::raw(if picked { "\u{25b8} " } else { "  " }),
                    Span::styled(pad_left(&age(h), AGE_WIDTH), MUTED),
                    Span::raw(" "),
                    Span::styled(format!("{:<SOURCE_WIDTH$}", h.source.as_str()), HEADING),
                    Span::raw(" "),
                    Span::raw(title),
                ])
                .style(base),
                inner.width,
            )
        })
        .collect();

    let offset = scroll_offset(selected, inner.height as usize, lines.len());
    f.render_widget(
        Paragraph::new(lines[offset.min(lines.len())..].to_vec()),
        inner,
    );
}

/// Compact relative age: "12m", "3h", "2d".
fn age(h: &Headline) -> String {
    let Some(published) = h.published else {
        return String::new();
    };
    let minutes = (Utc::now() - published).num_minutes().max(0);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 60 * 48 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (60 * 24))
    }
}

// --- reader --------------------------------------------------------------

/// The story, wrapped to a reading measure, over whichever view opened it.
fn draw_reader(f: &mut Frame, app: &App, area: Rect) {
    let Some(reader) = &app.reader else {
        return;
    };
    let headline = &reader.headline;
    let block = panel(
        format!(
            " {} \u{00b7} {} ",
            headline.source.publisher(),
            headline.source.name()
        ),
        " j/k \u{2195} \u{00b7} c copy card \u{00b7} Esc back ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Page keys should move by the reader's own height, which has no group
    // headings in it.
    app.viewport_rows.set(inner.height.max(1) as usize);

    // A two-column gutter each side, and the measure beyond that.
    let width = (inner.width as usize).saturating_sub(4).min(READER_WIDTH);
    if width < 8 || inner.height == 0 {
        return;
    }
    let lines = reader_lines(headline, app.story(), width);
    let max = lines.len().saturating_sub(inner.height as usize);
    app.reader_max_scroll.set(max);
    let offset = reader.scroll.min(max);
    f.render_widget(
        Paragraph::new(lines[offset..].to_vec()),
        Rect {
            x: inner.x + 2,
            y: inner.y,
            width: width as u16,
            height: inner.height,
        },
    );
}

/// Every line of the reader: the headline, a byline, then the story or a
/// note on why it is not there yet.
fn reader_lines(headline: &Headline, story: Option<&Story>, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let article = match story {
        Some(Story::Ready(article)) => Some(article.as_ref()),
        _ => None,
    };

    let title = article
        .map(|a| a.title.as_str())
        .filter(|t| !t.is_empty())
        .unwrap_or(&headline.title);
    for line in wrap(title, width) {
        lines.push(Line::from(Span::styled(line, BOLD)));
    }

    let mut meta: Vec<String> = Vec::new();
    if let Some(byline) = article.and_then(|a| a.byline.as_deref()) {
        meta.push(format!("By {byline}"));
    }
    if let Some(at) = article.and_then(|a| a.published).or(headline.published) {
        meta.push(
            at.with_timezone(&Local)
                .format("%-d %b %Y, %H:%M")
                .to_string(),
        );
    }
    if let Some(section) = article.and_then(|a| a.section.as_deref()) {
        meta.push(section.to_string());
    }
    if !meta.is_empty() {
        for line in wrap(&meta.join(" \u{00b7} "), width) {
            lines.push(Line::from(Span::styled(line, MUTED)));
        }
    }
    lines.push(Line::default());

    match article {
        Some(article) => push_article(&mut lines, article, width),
        None => {
            if !headline.description.is_empty() {
                for line in wrap(&headline.description, width) {
                    lines.push(Line::from(line));
                }
                lines.push(Line::default());
            }
            match story {
                Some(Story::Failed(e)) => {
                    for line in wrap(&format!("Could not load the story: {e}"), width) {
                        lines.push(Line::from(Span::styled(line, DOWN)));
                    }
                    lines.push(Line::from(Span::styled("Press r to try again.", MUTED)));
                }
                _ => lines.push(Line::from(Span::styled("Loading the story\u{2026}", MUTED))),
            }
        }
    }
    lines
}

fn push_article(lines: &mut Vec<Line<'static>>, article: &Article, width: usize) {
    if !article.key_points.is_empty() {
        lines.push(Line::from(Span::styled("Key points", HEADING)));
        for point in &article.key_points {
            push_bullet(lines, point, width);
        }
        lines.push(Line::default());
    }
    for block in &article.body {
        match block {
            Text::Paragraph(text) => {
                for line in wrap(text, width) {
                    lines.push(Line::from(line));
                }
                lines.push(Line::default());
            }
            Text::Heading(text) => {
                for line in wrap(text, width) {
                    lines.push(Line::from(Span::styled(
                        line,
                        HEADING.add_modifier(Modifier::BOLD),
                    )));
                }
                lines.push(Line::default());
            }
            Text::Quote(text) => {
                for line in wrap(text, width.saturating_sub(2)) {
                    lines.push(Line::from(vec![
                        Span::styled("\u{2502} ", HEADING),
                        Span::styled(line, Style::new().add_modifier(Modifier::ITALIC)),
                    ]));
                }
                lines.push(Line::default());
            }
            Text::Bullet(text) => push_bullet(lines, text, width),
        }
    }
    if article.premium {
        lines.push(Line::from(Span::styled(
            "A CNBC Pro story: only the free preview is available.",
            MUTED,
        )));
    }
}

/// A bullet with a hanging indent, so a wrapped point reads as one item.
fn push_bullet(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    for (n, line) in wrap(text, width.saturating_sub(2)).into_iter().enumerate() {
        let marker = if n == 0 { "\u{2022} " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(marker, HEADING),
            Span::raw(line),
        ]));
    }
}

/// Greedy word wrap on character count. A word longer than the width is
/// broken rather than left to overflow, which a URL in a story would do.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for word in text.split_whitespace() {
        let len = word.chars().count();
        if used > 0 && used + 1 + len > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if len > width {
            for c in word.chars() {
                if used == width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(c);
                used += 1;
            }
            continue;
        }
        if used > 0 {
            line.push(' ');
            used += 1;
        }
        line.push_str(word);
        used += len;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// --- detail --------------------------------------------------------------

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let instrument = app.focused();
    let index = app.detail.unwrap_or(0);
    let quote = app.quotes[index].as_ref();

    let block = panel(
        format!(" {} \u{00b7} {} ", instrument.name, instrument.cnbc),
        " h/l range \u{00b7} j/k headlines \u{00b7} o read \u{00b7} c card \u{00b7} Esc back ",
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(1),
            Constraint::Min(4),
        ])
        .split(inner);

    draw_detail_header(f, instrument, quote, app.range, chunks[0]);

    let series = instrument
        .history
        .and_then(|key| app.history.get(&(app.range, key)));
    draw_chart(f, instrument, series, chunks[1]);

    let (headlines, title) = app.related_headlines();
    let rule = format!("\u{2500} {title} ");
    let fill = (chunks[2].width as usize).saturating_sub(rule.chars().count());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(rule, HEADING),
            Span::styled("\u{2500}".repeat(fill), MUTED),
        ])),
        chunks[2],
    );
    draw_headline_list(f, &headlines, app.detail_news_scroll, chunks[3]);
}

fn draw_detail_header(
    f: &mut Frame,
    instrument: &Instrument,
    quote: Option<&Quote>,
    range: Range,
    area: Rect,
) {
    let mut lines = Vec::new();
    match quote {
        Some(q) => {
            let dir = if q.change < 0.0 { DOWN } else { UP };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    instrument.level(q.last),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled(instrument.change(q.change), dir),
                Span::raw("  "),
                Span::styled(format!("({})", format_percent(q.change_pct)), dir),
                Span::raw("   "),
                Span::styled(market_status(q), MUTED),
            ]));
            let field = |label: &str, value: Option<f64>| match value {
                Some(v) => format!("{label} {}   ", instrument.level(v)),
                None => String::new(),
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}{}{}{}",
                    field("Open", q.open),
                    field("High", q.high),
                    field("Low", q.low),
                    field("Prev", q.prev_close)
                ),
                MUTED,
            )));
            if let (Some(low), Some(high)) = (q.year_low, q.year_high) {
                lines.push(year_range_line(instrument, q.last, low, high));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "  No quote available for this instrument.",
            MUTED,
        ))),
    }
    lines.push(Line::from(range_selector(range)));
    f.render_widget(Paragraph::new(lines), area);
}

/// Where the last price sits inside the 52-week range.
fn year_range_line(instrument: &Instrument, last: f64, low: f64, high: f64) -> Line<'static> {
    const WIDTH: usize = 32;
    let span = high - low;
    let at = if span > 0.0 {
        (((last - low) / span) * WIDTH as f64)
            .round()
            .clamp(0.0, WIDTH as f64) as usize
    } else {
        WIDTH / 2
    };
    let mut bar = String::new();
    for n in 0..=WIDTH {
        bar.push(if n == at { '\u{25cf}' } else { '\u{2500}' });
    }
    Line::from(vec![
        Span::styled(format!("  52w {} ", instrument.level(low)), MUTED),
        Span::raw(bar),
        Span::styled(format!(" {}", instrument.level(high)), MUTED),
    ])
}

fn market_status(q: &Quote) -> String {
    match q.market_status.as_deref() {
        Some("REG_MKT") => "\u{25cf} open".into(),
        Some("AFT_MKT") => "\u{25cb} after hours".into(),
        Some("PRE_MKT") => "\u{25cb} pre-market".into(),
        Some(_) | None => "\u{25cb} closed".into(),
    }
}

fn range_selector(current: Range) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled("  Price  ", MUTED)];
    for range in Range::ALL {
        spans.push(if range == current {
            Span::styled(format!(" [{}] ", range.as_str()), SELECTED_STYLE)
        } else {
            Span::styled(format!("  {}  ", range.as_str()), MUTED)
        });
    }
    spans
}

fn draw_chart(f: &mut Frame, instrument: &Instrument, series: Option<&Series>, area: Rect) {
    let Some(series) = series.filter(|s| !s.is_empty()) else {
        f.render_widget(
            Paragraph::new("No history for this range.")
                .alignment(Alignment::Center)
                .style(MUTED),
            area,
        );
        return;
    };

    // x is the point index, not the timestamp: spacing sessions evenly is what
    // a price chart does, and using real time would draw long flat runs across
    // weekends and holidays.
    let points: Vec<(f64, f64)> = series
        .iter()
        .enumerate()
        .map(|(n, (_, v))| (n as f64, *v))
        .collect();

    let (low, high) = bounds(series);
    let rising = series.last().map(|(_, v)| *v) >= series.first().map(|(_, v)| *v);
    let colour = if rising { Color::Green } else { Color::Red };

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(colour))
        .data(&points);

    let y_labels: Vec<Line> = [low, (low + high) / 2.0, high]
        .iter()
        .map(|v| Line::from(Span::styled(instrument.level(*v), MUTED)))
        .collect();
    let x_labels = vec![
        Line::from(Span::styled(date_label(series.first()), MUTED)),
        Line::from(Span::styled(date_label(series.last()), MUTED)),
    ];

    f.render_widget(
        Chart::new(vec![dataset])
            .x_axis(
                Axis::default()
                    .bounds([0.0, (points.len().saturating_sub(1)).max(1) as f64])
                    .labels(x_labels)
                    .style(MUTED),
            )
            .y_axis(
                Axis::default()
                    .bounds([low, high])
                    .labels(y_labels)
                    .style(MUTED),
            ),
        area,
    );
}

/// Chart bounds padded around the series' own range.
///
/// Never anchored at zero: a price series has no meaningful zero, and starting
/// there flattens every real move into a straight line at the top.
fn bounds(series: &Series) -> (f64, f64) {
    let mut low = f64::MAX;
    let mut high = f64::MIN;
    for (_, v) in series {
        low = low.min(*v);
        high = high.max(*v);
    }
    if !low.is_finite() || !high.is_finite() {
        return (0.0, 1.0);
    }
    // A dead-flat series would otherwise get a zero-height axis.
    let pad = ((high - low) * 0.05)
        .max(high.abs() * 1e-4)
        .max(f64::EPSILON);
    (low - pad, high + pad)
}

fn date_label(point: Option<&(i64, f64)>) -> String {
    point
        .and_then(|(ts, _)| chrono::DateTime::from_timestamp_millis(*ts))
        .map(|d| d.with_timezone(&Local).format("%-d %b").to_string())
        .unwrap_or_default()
}

// --- sparkline -----------------------------------------------------------

const SPARK_GLYPHS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// A one-cell-tall trend for a board row.
///
/// Normalised over the window's own minimum and maximum rather than over zero,
/// for the same reason the detail chart is: anchoring a price series at zero
/// renders every bar full height and shows nothing.
fn sparkline(series: &Series, width: usize) -> String {
    if series.is_empty() || width == 0 {
        return String::new();
    }
    let values: Vec<f64> = series
        .iter()
        .skip(series.len().saturating_sub(width))
        .map(|(_, v)| *v)
        .collect();
    let low = values.iter().cloned().fold(f64::MAX, f64::min);
    let high = values.iter().cloned().fold(f64::MIN, f64::max);
    let span = high - low;
    values
        .iter()
        .map(|v| {
            // A flat window has no shape to show; a mid-height run says so
            // without dividing by zero.
            let n = if span > 0.0 {
                ((v - low) / span * (SPARK_GLYPHS.len() - 1) as f64).round() as usize
            } else {
                SPARK_GLYPHS.len() / 2
            };
            SPARK_GLYPHS[n.min(SPARK_GLYPHS.len() - 1)]
        })
        .collect()
}

// --- status and help -----------------------------------------------------

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    if app.loading {
        spans.push(Span::styled(" \u{27f3} ", HEADING));
    } else {
        spans.push(Span::raw(" "));
    }

    match app.last_updated {
        Some(at) => {
            let age = (Local::now() - at).num_seconds();
            // Three refresh intervals without an update means the board is
            // frozen, and it should say so rather than look live.
            let style = if age > 45 { DOWN } else { MUTED };
            spans.push(Span::styled(
                format!("Updated {}", at.format("%H:%M:%S")),
                style,
            ));
            if age > 45 {
                spans.push(Span::styled(format!(" ({age}s ago)"), DOWN));
            }
        }
        // Distinguishes "still waiting" from "tried and got nothing", which
        // otherwise both read as a load that never finishes.
        None if app.loading => spans.push(Span::styled("Loading\u{2026}", MUTED)),
        None => spans.push(Span::styled("No data", DOWN)),
    }

    // Both tables ride one request, so one count covers them.
    let priced = app
        .quotes
        .iter()
        .chain(app.dow_quotes.iter())
        .filter(|q| q.is_some())
        .count();
    spans.push(Span::styled(
        format!("   {priced}/{} quotes", INSTRUMENTS.len() + DOW_30.len()),
        MUTED,
    ));
    if app.active_tab == Tab::Movers && app.detail.is_none() {
        spans.push(Span::styled(
            format!("   {} movers", app.movers().len()),
            MUTED,
        ));
    }
    spans.push(Span::styled(
        format!("   {} headlines", app.headlines.len()),
        MUTED,
    ));

    if let Some(error) = &app.error {
        spans.push(Span::styled(
            format!("   \u{26a0} {}", truncate(error, 60)),
            DOWN,
        ));
    }
    match &app.notice {
        Some(Ok(note)) => spans.push(Span::styled(format!("   \u{2713} {note}"), UP)),
        Some(Err(note)) => spans.push(Span::styled(format!("   \u{26a0} {note}"), DOWN)),
        None => {}
    }

    let hint = "[?] help  [r]efresh  [q]uit ";
    let left = Line::from(spans);
    // Dropped rather than overlapped: the two are drawn into the same row, so
    // on a narrow terminal they would print over each other.
    // Two cells of clearance, so the two halves never sit flush against
    // each other and read as one run of text.
    if left.width() + hint.len() + 2 <= area.width as usize {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, MUTED))).alignment(Alignment::Right),
            area,
        );
    }
    f.render_widget(Paragraph::new(left), area);
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("1/2/3/4, Tab", "movers, board, the Dow 30, news"),
        ("j / k, arrows", "move the selection, or scroll a story"),
        ("Ctrl-D / Ctrl-U", "half page down / up"),
        ("g / G, Home/End", "first / last"),
        ("h / l", "movers: previous / next card or story"),
        ("", "board: jump group   news: cycle section"),
        ("", "dow: order by points, weight, range or move"),
        ("", "detail: switch the chart range"),
        ("Enter", "movers, board: open the detail view"),
        ("", "movers stories: read the story"),
        ("", "news: read the story, right here"),
        ("", "movers: k from the top row reaches the stories"),
        ("n / N", "movers: step through the stories"),
        ("", "board: scroll the news rail"),
        ("f", "rail: matched headlines or the whole pool"),
        ("o", "read the selected story"),
        ("c", "copy the story as an image, ready to paste in a post"),
        ("r", "refresh everything now, or retry a story"),
        ("Esc", "close the story, the detail view or this overlay"),
        ("q / Ctrl-C", "quit"),
    ];

    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!("  {key:<17}"), HEADING),
                Span::raw(*what),
            ])
        })
        .collect();

    let rect = centered_size(66, lines.len() as u16 + 2, area);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Keys ")
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(values: &[f64]) -> Series {
        values
            .iter()
            .enumerate()
            .map(|(n, v)| (n as i64 * 86_400_000, *v))
            .collect()
    }

    /// The regression this whole renderer exists to avoid: normalising over
    /// zero would render a price series near 7,700 as ten identical full
    /// blocks.
    #[test]
    fn a_sparkline_normalizes_over_the_window_not_over_zero() {
        let spark = sparkline(&series(&[7700.0, 7720.0, 7740.0]), 10);
        assert_eq!(spark.chars().count(), 3);
        assert_eq!(spark.chars().next(), Some('\u{2581}'));
        assert_eq!(spark.chars().last(), Some('\u{2588}'));
    }

    /// Crypto trades weekends and so carries more sessions than the equity
    /// indices. Sizing each row to its own series left the shorter ones
    /// floating away from the percent column, ragged down the board.
    #[test]
    fn a_shorter_series_still_fills_the_width_it_is_given() {
        let equities = sparkline(&series(&[1.0, 2.0, 3.0]), 3);
        let crypto = sparkline(&series(&[1.0, 2.0, 3.0, 4.0, 5.0]), 3);
        assert_eq!(equities.chars().count(), crypto.chars().count());
    }

    #[test]
    fn a_sparkline_shows_only_the_most_recent_points_that_fit() {
        assert_eq!(
            sparkline(&series(&[1.0, 2.0, 3.0, 4.0, 5.0]), 3)
                .chars()
                .count(),
            3
        );
    }

    #[test]
    fn a_flat_series_sparks_to_one_level_instead_of_dividing_by_zero() {
        let spark = sparkline(&series(&[4.5, 4.5, 4.5]), 10);
        assert_eq!(spark, "\u{2585}\u{2585}\u{2585}");
    }

    #[test]
    fn an_empty_series_sparks_to_nothing() {
        assert_eq!(sparkline(&series(&[]), 10), "");
        assert_eq!(sparkline(&series(&[1.0]), 0), "");
    }

    #[test]
    fn chart_bounds_pad_the_series_so_the_line_never_touches_the_frame() {
        let (low, high) = bounds(&series(&[100.0, 200.0]));
        assert!(low < 100.0 && high > 200.0);
        assert!(low > 90.0 && high < 210.0, "padding should be small");
    }

    /// A zero-height axis would make the chart widget draw nothing.
    #[test]
    fn chart_bounds_of_a_flat_series_are_still_a_nonzero_range() {
        let (low, high) = bounds(&series(&[4.5, 4.5]));
        assert!(high > low);
    }

    #[test]
    fn chart_bounds_never_anchor_at_zero() {
        let (low, _) = bounds(&series(&[7700.0, 7750.0]));
        assert!(low > 7000.0, "bounds must follow the data, got {low}");
    }

    #[test]
    fn wrap_breaks_between_words_and_inside_words_that_do_not_fit() {
        assert_eq!(
            wrap("the quick brown fox", 9),
            vec!["the quick", "brown fox"]
        );
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a  b", 10), vec!["a b"]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn wrap_counts_characters_not_bytes() {
        assert_eq!(wrap("caf\u{e9} au lait", 7), vec!["caf\u{e9} au", "lait"]);
    }

    #[test]
    fn the_grid_adds_a_column_as_the_terminal_widens() {
        assert_eq!(grid_columns(40), 1);
        assert_eq!(grid_columns(60), 2);
        assert_eq!(grid_columns(100), 3);
        assert_eq!(grid_columns(160), 5);
        assert_eq!(grid_columns(400), MAX_COLUMNS, "cards stop multiplying");
        assert_eq!(grid_columns(0), 1, "there is always one column");
    }

    #[test]
    fn a_card_drops_the_trend_then_the_move_as_it_narrows() {
        assert_eq!(card_size(34, 20), CardSize::Tall);
        assert_eq!(card_size(24, 20), CardSize::Short);
        assert_eq!(card_size(18, 20), CardSize::Tiny);
    }

    /// A card taller than its pane would leave the grid blank.
    #[test]
    fn a_card_is_never_taller_than_the_pane_it_sits_in() {
        assert_eq!(card_size(34, 4), CardSize::Short);
        assert_eq!(card_size(34, 3), CardSize::Tiny);
        assert_eq!(card_size(34, 1), CardSize::Tiny);
    }

    /// Nine rows hold one tall card and four wasted lines, or two short ones.
    #[test]
    fn a_short_pane_trades_the_trend_for_a_second_row_of_cards() {
        assert_eq!(card_size(34, 9), CardSize::Short);
        assert_eq!(card_size(34, 10), CardSize::Tall);
        // Seven rows hold one card either way, so it keeps its trend.
        assert_eq!(card_size(34, 7), CardSize::Tall);
    }

    #[test]
    fn spread_pushes_two_values_apart_and_drops_the_second_when_both_will_not_fit() {
        assert_eq!(
            spread(Span::raw("a"), Span::raw("b"), 6).to_string(),
            "a    b"
        );
        assert_eq!(
            spread(Span::raw("abc"), Span::raw("def"), 6).to_string(),
            "abc"
        );
    }

    #[test]
    fn a_macro_story_with_no_date_or_summary_shows_no_empty_fields() {
        let headline = Headline {
            title: "Payrolls rose".into(),
            link: "https://www.cnbc.com/x".into(),
            description: String::new(),
            published: None,
            source: crate::api::rss::Source::Top,
            haystack: String::new(),
        };
        assert_eq!(story_meta(&headline), "Top news");
    }

    #[test]
    fn story_cards_share_the_row_up_to_one_per_macro_story() {
        assert_eq!(news_columns(20), 1);
        assert_eq!(news_columns(50), 2);
        assert_eq!(news_columns(80), 3);
        assert_eq!(news_columns(300), MACRO_HEADLINES);
    }

    /// A long headline makes the whole row taller rather than being cut off.
    #[test]
    fn the_story_row_grows_to_fit_the_longest_headline() {
        let story = |title: &str| Headline {
            title: title.into(),
            link: String::new(),
            description: String::new(),
            published: None,
            source: crate::api::rss::Source::Top,
            haystack: String::new(),
        };
        let short = story("Fed holds");
        let long = story("Treasury yields climb as traders price in a slower pace of rate cuts");
        // Two cards of 29 and 30, so 25 and 26 columns of text: the long
        // title takes three.
        assert_eq!(story_widths(2, 60), [29, 30]);
        assert_eq!(story_widths(3, 74), [24, 24, 24]);
        assert_eq!(story_row_height(&[&short], 60), 4);
        assert_eq!(story_row_height(&[&short, &long], 60), 6);
        let text: String = wrap(&long.title, 25).join(" ");
        assert_eq!(text, long.title, "every word is kept");
    }

    #[test]
    fn a_loading_story_shows_the_summary_and_says_it_is_loading() {
        let headline = Headline {
            title: "Payrolls rose".into(),
            link: "https://www.cnbc.com/x".into(),
            description: "More than expected.".into(),
            published: None,
            source: crate::api::rss::Source::Top,
            haystack: String::new(),
        };
        let lines = reader_lines(&headline, Some(&Story::Loading), 40);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(text[0], "Payrolls rose");
        assert!(text.contains(&"More than expected.".to_string()));
        assert!(text.last().unwrap().starts_with("Loading"));
    }

    #[test]
    fn a_loaded_story_lists_its_key_points_before_the_body() {
        let headline = Headline {
            title: "t".into(),
            link: "https://www.cnbc.com/x".into(),
            description: String::new(),
            published: None,
            source: crate::api::rss::Source::Top,
            haystack: String::new(),
        };
        let article = Box::new(Article {
            title: "The real title".into(),
            byline: Some("A Reporter".into()),
            key_points: vec!["One.".into()],
            body: vec![
                Text::Paragraph("Body.".into()),
                Text::Heading("Sub".into()),
                Text::Quote("Said.".into()),
            ],
            ..Article::default()
        });
        let story = Story::Ready(article);
        let text: Vec<String> = reader_lines(&headline, Some(&story), 40)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(text[0], "The real title");
        assert_eq!(text[1], "By A Reporter");
        let at = |s: &str| {
            text.iter()
                .position(|l| l == s)
                .unwrap_or_else(|| panic!("{s:?} missing in {text:?}"))
        };
        assert!(at("Key points") < at("\u{2022} One."));
        assert!(at("\u{2022} One.") < at("Body."));
        assert!(at("Body.") < at("Sub"));
        assert!(at("Sub") < at("\u{2502} Said."));
    }
}
