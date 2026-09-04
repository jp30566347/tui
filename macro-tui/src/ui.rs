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

use crate::api::models::{Quote, Series};
use crate::api::rss::Headline;
use crate::app::{App, Range, Tab};
use crate::catalog::{format_percent, Group, Instrument, INSTRUMENTS};
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

    if app.detail.is_some() {
        draw_detail(f, app, chunks[1]);
    } else {
        match app.active_tab {
            Tab::Board => draw_board(f, app, chunks[1]),
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
            " n/N scroll \u{00b7} f all \u{00b7} o open ",
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

// --- news ----------------------------------------------------------------

fn draw_news(f: &mut Frame, app: &App, area: Rect) {
    let headlines = app.filtered_headlines();
    let source = app
        .news_filter
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "all sources".into());
    draw_headline_pane(
        f,
        &headlines,
        app.news_scroll,
        area,
        &format!("News \u{00b7} {source}"),
        " j/k \u{2195} \u{00b7} h/l source \u{00b7} Enter open ",
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

// --- detail --------------------------------------------------------------

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let instrument = app.focused();
    let index = app.detail.unwrap_or(0);
    let quote = app.quotes[index].as_ref();

    let block = panel(
        format!(" {} \u{00b7} {} ", instrument.name, instrument.cnbc),
        " h/l range \u{00b7} j/k headlines \u{00b7} o open \u{00b7} Esc back ",
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

    let priced = app.quotes.iter().filter(|q| q.is_some()).count();
    spans.push(Span::styled(
        format!("   {priced}/{} quotes", INSTRUMENTS.len()),
        MUTED,
    ));
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
        ("1 / 2, Tab", "switch between the board and news"),
        ("j / k, arrows", "move the selection"),
        ("Ctrl-D / Ctrl-U", "half page down / up"),
        ("g / G, Home/End", "first / last row"),
        ("h / l", "board: jump group   news: cycle source"),
        ("", "detail: switch the chart range"),
        ("Enter", "board: open the detail view"),
        ("", "news: open the story"),
        ("n / N", "scroll the board's news rail"),
        ("f", "rail: matched headlines or the whole pool"),
        ("o", "open the selected story in a browser"),
        ("r", "refresh everything now"),
        ("Esc", "close the detail view or this overlay"),
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
}
