//! The share card: a story rendered as an image, for pasting into a post.
//!
//! The card is drawn as a terminal window running macro-tui, so it is
//! unmistakably from the app: a box-drawing frame, a monospace face
//! throughout, the headline in bold, CNBC's key points as a list, and, when
//! the story mentions one of the board's instruments, that row's price, move
//! and month of closes as a ticker strip. A timeline
//! shows an attached image at 16:9, so it is 1600 by 900. Everything is drawn
//! here with fonts compiled into the binary, so the card looks the same on
//! every machine and needs nothing installed.

use std::borrow::Cow;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use chrono::{DateTime, Local, Utc};
use image::{ImageFormat, Rgba, RgbaImage};

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
/// Where the window frame sits, in from the image edge.
const FRAME: f32 = 40.0;
/// How far text sits in from the frame.
const GUTTER: f32 = 40.0;
const CONTENT_LEFT: f32 = FRAME + GUTTER;
const CONTENT_RIGHT: f32 = WIDTH as f32 - FRAME - GUTTER;
const CONTENT_WIDTH: f32 = CONTENT_RIGHT - CONTENT_LEFT;
const BORDER_WIDTH: u32 = 2;

/// The terminal's text size and its line pitch.
const BASE: f32 = 32.0;
const ROW: f32 = 48.0;
/// A headline longer than three lines at the smallest size is cut.
const HEADLINE_LINES: usize = 3;
const HEADLINE_SIZES: [f32; 3] = [52.0, 46.0, 40.0];
const MAX_POINTS: usize = 4;
/// A point longer than this is cut with an ellipsis rather than allowed to
/// crowd out the ones after it.
const POINT_LINES: usize = 3;
/// Bars in the ticker strip: width, gap and tallest bar.
const BAR_WIDTH: f32 = 10.0;
const BAR_GAP: f32 = 4.0;
const BAR_HEIGHT: f32 = 30.0;
const BAR_MAX: usize = 24;

// Subset to Latin plus the box-drawing and block ranges. See
// assets/fonts/LICENSE.txt.
const REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");

const BACKGROUND: Rgba<u8> = Rgba([13, 17, 23, 255]);
const TEXT: Rgba<u8> = Rgba([230, 237, 243, 255]);
const BODY: Rgba<u8> = Rgba([201, 209, 217, 255]);
const MUTED: Rgba<u8> = Rgba([125, 133, 144, 255]);
const BORDER: Rgba<u8> = Rgba([72, 79, 88, 255]);
const CYAN: Rgba<u8> = Rgba([121, 192, 255, 255]);
const GREEN: Rgba<u8> = Rgba([63, 185, 80, 255]);
const RED: Rgba<u8> = Rgba([248, 81, 73, 255]);

/// The instrument a story is about, as it reads on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Ticker {
    pub name: String,
    pub level: String,
    pub change: String,
    pub percent: String,
    pub up: bool,
    /// A month of daily closes, oldest first. Empty when there is no history.
    pub closes: Vec<f64>,
}

/// Everything the card shows. Built from app state by the key handler, so
/// rendering needs nothing but this.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub title: String,
    /// Publisher and section, such as "CNBC · Economy".
    pub kicker: String,
    pub published: Option<DateTime<Utc>>,
    /// The story's key points. Empty when it has none.
    pub points: Vec<String>,
    /// Shown instead of the points when there are none.
    pub summary: String,
    /// The story's host, without a `www.` prefix.
    pub domain: String,
    /// The board row the story mentions, if any.
    pub ticker: Option<Ticker>,
}

/// Renders the card, writes it beside the user's pictures, and puts it on the
/// clipboard. Returns a note for the status line.
///
/// The file is written first and always: a clipboard can fail for reasons the
/// app cannot fix (no display server, a sandbox), and a file on disk is
/// something the user can still attach.
pub fn share(card: &Card) -> Result<String, String> {
    let image = render(card);
    let path = save(&image, card).map_err(|e| format!("could not save the card: {e}"))?;
    let shown = display_path(&path);
    match copy(&image) {
        Ok(()) => Ok(format!("Card copied to the clipboard and saved to {shown}")),
        Err(e) => Err(format!("Card saved to {shown}; the clipboard failed: {e}")),
    }
}

/// The card as an RGBA bitmap.
pub fn render(card: &Card) -> RgbaImage {
    let regular = FontRef::try_from_slice(REGULAR).expect("embedded font");
    let bold = FontRef::try_from_slice(BOLD).expect("embedded font");
    let mut img = RgbaImage::from_pixel(WIDTH, HEIGHT, BACKGROUND);

    let top = FRAME;
    let bottom = HEIGHT as f32 - FRAME;
    frame(&mut img, FRAME, top, WIDTH as f32 - FRAME, bottom);

    // Kicker on the left, date on the right.
    let mut y = top + ROW * 1.9;
    let (publisher, section) = card
        .kicker
        .split_once(" \u{b7} ")
        .unwrap_or((card.kicker.as_str(), ""));
    let mut x = CONTENT_LEFT;
    x += draw(&mut img, &bold, BASE, x, y, publisher, CYAN);
    if !section.is_empty() {
        draw(
            &mut img,
            &regular,
            BASE,
            x,
            y,
            &format!(" \u{b7} {section}"),
            MUTED,
        );
    }
    if let Some(date) = card.published {
        let text = date.with_timezone(&Local).format("%-d %b %Y").to_string();
        let w = measure(&regular, BASE, &text);
        draw(&mut img, &regular, BASE, CONTENT_RIGHT - w, y, &text, MUTED);
    }

    // The headline, bold, at the largest size that fits.
    let (size, lines) = fit_headline(&bold, &card.title);
    y += ROW * 0.6;
    for line in &lines {
        y += size * 1.15;
        draw(&mut img, &bold, size, CONTENT_LEFT, y, line, TEXT);
    }

    // The ticker strip is anchored above the bottom edge; whatever is
    // between the headline and it is for the points.
    let strip_top = match card.ticker {
        Some(_) => bottom - ROW * 2.7,
        None => bottom - ROW * 0.9,
    };
    // Clearance between the last line and whatever comes after it.
    let clearance = ROW * 0.4;
    y += ROW * 0.4;
    let indent = char_width(BASE) * 2.0;
    if card.points.is_empty() {
        for line in wrap(&regular, BASE, &card.summary, CONTENT_WIDTH)
            .into_iter()
            .take(4)
        {
            if y + ROW + clearance > strip_top {
                break;
            }
            y += ROW;
            draw(&mut img, &regular, BASE, CONTENT_LEFT, y, &line, BODY);
        }
    } else {
        for text in card.points.iter().take(MAX_POINTS) {
            let width = CONTENT_WIDTH - indent;
            let lines = cut(
                &regular,
                BASE,
                wrap(&regular, BASE, text, width),
                width,
                POINT_LINES,
            );
            if y + ROW * lines.len() as f32 + clearance > strip_top {
                break;
            }
            for (n, line) in lines.iter().enumerate() {
                y += ROW;
                if n == 0 {
                    draw(&mut img, &regular, BASE, CONTENT_LEFT, y, "\u{25b8}", CYAN);
                }
                draw(
                    &mut img,
                    &regular,
                    BASE,
                    CONTENT_LEFT + indent,
                    y,
                    line,
                    BODY,
                );
            }
            y += ROW * 0.1;
        }
    }

    if let Some(ticker) = &card.ticker {
        draw_ticker(&mut img, &regular, &bold, ticker, strip_top);
    }

    // The bottom edge carries the source and the app's name, as the app's
    // own panels carry their hints.
    label(
        &mut img,
        &regular,
        FRAME + 24.0,
        bottom,
        &format!(" {} ", card.domain),
        MUTED,
    );
    let brand = " macro-tui ";
    let w = measure(&regular, BASE, brand);
    label(
        &mut img,
        &regular,
        WIDTH as f32 - FRAME - 24.0 - w,
        bottom,
        brand,
        MUTED,
    );

    img
}

/// The instrument's row: a rule carrying its name, then the level, the move
/// and a month of closes as bars, the way the board shows it.
fn draw_ticker(img: &mut RgbaImage, regular: &FontRef, bold: &FontRef, ticker: &Ticker, top: f32) {
    let rule_y = top + ROW * 0.5;
    fill(
        img,
        CONTENT_LEFT as u32,
        rule_y as u32,
        CONTENT_WIDTH as u32,
        BORDER_WIDTH,
        BORDER,
    );
    label(
        img,
        regular,
        CONTENT_LEFT + char_width(BASE),
        rule_y,
        &format!(" {} ", ticker.name),
        CYAN,
    );

    let y = rule_y + ROW * 1.3;
    let colour = if ticker.up { GREEN } else { RED };
    let mut x = CONTENT_LEFT;
    x += draw(img, bold, BASE, x, y, &ticker.level, TEXT);
    x += char_width(BASE) * 3.0;
    x += draw(img, regular, BASE, x, y, &ticker.change, colour);
    x += char_width(BASE) * 2.0;
    x += draw(img, regular, BASE, x, y, &ticker.percent, colour);
    x += char_width(BASE) * 3.0;

    let closes = &ticker.closes[ticker.closes.len().saturating_sub(BAR_MAX)..];
    if !closes.is_empty() {
        let low = closes.iter().cloned().fold(f64::MAX, f64::min);
        let high = closes.iter().cloned().fold(f64::MIN, f64::max);
        let span = high - low;
        for value in closes {
            // A flat month has no shape to show; half height says so.
            let level = if span > 0.0 {
                ((value - low) / span) as f32
            } else {
                0.5
            };
            let h = (3.0 + level * (BAR_HEIGHT - 3.0)).round();
            fill(
                img,
                x as u32,
                (y - h) as u32,
                BAR_WIDTH as u32,
                h as u32,
                colour,
            );
            x += BAR_WIDTH + BAR_GAP;
        }
        x += char_width(BASE) * 2.0;
        draw(img, regular, BASE, x, y, "1M", MUTED);
    }
}

/// The window frame: two-pixel edges, with the corners squared off.
fn frame(img: &mut RgbaImage, left: f32, top: f32, right: f32, bottom: f32) {
    let (l, t, r, b) = (left as u32, top as u32, right as u32, bottom as u32);
    fill(img, l, t, r - l, BORDER_WIDTH, BORDER);
    fill(img, l, b, r - l + BORDER_WIDTH, BORDER_WIDTH, BORDER);
    fill(img, l, t, BORDER_WIDTH, b - t, BORDER);
    fill(img, r, t, BORDER_WIDTH, b - t, BORDER);
}

/// Text sitting in a frame edge, the way a panel title does: the background
/// is painted behind it so the line breaks around the words.
fn label(img: &mut RgbaImage, font: &FontRef, x: f32, line_y: f32, text: &str, colour: Rgba<u8>) {
    let w = measure(font, BASE, text);
    let h = ROW * 0.9;
    fill(
        img,
        x as u32,
        (line_y - h / 2.0) as u32,
        w.ceil() as u32,
        h as u32,
        BACKGROUND,
    );
    // A baseline that centres the x-height on the line.
    draw(img, font, BASE, x, line_y + BASE * 0.36, text, colour);
}

/// The advance of one cell in the monospace face.
fn char_width(size: f32) -> f32 {
    size * 0.6
}

/// ab_glyph scales by line height rather than by em, so a size given in
/// pixels per em has to go through the font's own metrics first.
fn scale(font: &FontRef, size: f32) -> PxScale {
    let per_em = font.units_per_em().expect("font has units per em");
    PxScale::from(size * font.height_unscaled() / per_em)
}

/// The card as PNG bytes.
pub fn png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The largest headline size whose wrapped lines fit, cut to the line limit
/// with an ellipsis if even the smallest does not.
fn fit_headline(font: &FontRef, title: &str) -> (f32, Vec<String>) {
    for size in HEADLINE_SIZES {
        let lines = wrap(font, size, title, CONTENT_WIDTH);
        if lines.len() <= HEADLINE_LINES {
            return (size, lines);
        }
    }
    let size = HEADLINE_SIZES[HEADLINE_SIZES.len() - 1];
    let lines = wrap(font, size, title, CONTENT_WIDTH);
    (size, cut(font, size, lines, CONTENT_WIDTH, HEADLINE_LINES))
}

/// Keeps the first `max` lines, ending the last with an ellipsis when
/// anything was dropped. Trailing words go before the ellipsis does.
fn cut(font: &FontRef, size: f32, mut lines: Vec<String>, width: f32, max: usize) -> Vec<String> {
    if lines.len() <= max {
        return lines;
    }
    lines.truncate(max);
    if let Some(last) = lines.last_mut() {
        loop {
            let candidate = format!("{}\u{2026}", last.trim_end());
            if measure(font, size, &candidate) <= width || !last.contains(' ') {
                *last = candidate;
                break;
            }
            let at = last.rfind(' ').unwrap_or(0);
            last.truncate(at);
        }
    }
    lines
}

/// Greedy word wrap by measured width. A single word wider than the line is
/// broken between characters rather than overflowing.
fn wrap(font: &FontRef, size: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if measure(font, size, &candidate) <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if measure(font, size, word) <= width {
            line = word.to_string();
            continue;
        }
        for c in word.chars() {
            let mut longer = line.clone();
            longer.push(c);
            if measure(font, size, &longer) > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn measure(font: &FontRef, size: f32, text: &str) -> f32 {
    layout(font, size, text, |_, _| {})
}

/// Draws `text` with its baseline at `y`. Returns the advance width.
fn draw(
    img: &mut RgbaImage,
    font: &FontRef,
    size: f32,
    x: f32,
    y: f32,
    text: &str,
    colour: Rgba<u8>,
) -> f32 {
    let scale = scale(font, size);
    layout(font, size, text, |id, at| {
        let glyph = id.with_scale_and_position(scale, point(x + at, y));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|px, py, coverage| {
                let cx = bounds.min.x as i32 + px as i32;
                let cy = bounds.min.y as i32 + py as i32;
                blend(img, cx, cy, colour, coverage);
            });
        }
    })
}

/// Positions each glyph, calling `place` with its id and x offset, and
/// returns the total advance.
fn layout(font: &FontRef, size: f32, text: &str, mut place: impl FnMut(GlyphId, f32)) -> f32 {
    let scaled = font.as_scaled(scale(font, size));
    let mut x = 0.0;
    let mut previous: Option<GlyphId> = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(prev) = previous {
            x += scaled.kern(prev, id);
        }
        place(id, x);
        x += scaled.h_advance(id);
        previous = Some(id);
    }
    x
}

fn blend(img: &mut RgbaImage, x: i32, y: i32, colour: Rgba<u8>, coverage: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let px = img.get_pixel_mut(x as u32, y as u32);
    for n in 0..3 {
        px.0[n] = (px.0[n] as f32 * (1.0 - a) + colour.0[n] as f32 * a).round() as u8;
    }
    px.0[3] = 255;
}

fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, colour: Rgba<u8>) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, colour);
        }
    }
}

/// Writes the PNG under the user's pictures, or the cache directory when the
/// platform has no such folder.
fn save(image: &RgbaImage, card: &Card) -> Result<PathBuf, String> {
    let dir = dirs::picture_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("macro-tui");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let date = card
        .published
        .unwrap_or_else(Utc::now)
        .with_timezone(&Local)
        .format("%Y-%m-%d");
    let path = dir.join(format!("{date}-{}.png", slug(&card.title)));
    std::fs::write(&path, png(image)?).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn copy(image: &RgbaImage) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        })
        .map_err(|e| e.to_string())
}

/// A file-name-safe version of a title: lowercase, words joined by hyphens,
/// cut at a word boundary.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut hyphen = true;
    for c in title.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            hyphen = false;
        } else if !hyphen {
            out.push('-');
            hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.chars().count() > 60 {
        out = out.chars().take(60).collect();
        if let Some(cut) = out.rfind('-') {
            out.truncate(cut);
        }
    }
    if out.is_empty() {
        "story".into()
    } else {
        out
    }
}

/// The host of a link without its `www.`, or the link itself if it has no
/// recognisable host.
pub fn domain(link: &str) -> String {
    let rest = link.split_once("://").map(|(_, r)| r).unwrap_or(link);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// `~/Pictures/...` rather than the full home path, for the status line.
fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> FontRef<'static> {
        FontRef::try_from_slice(BOLD).unwrap()
    }

    fn card() -> Card {
        Card {
            title: "Bessent bond plan details to be revealed as Treasury secretary warns FX \
                    traders he's 'the house now'"
                .into(),
            kicker: "CNBC \u{b7} Markets".into(),
            published: DateTime::parse_from_rfc3339("2026-09-09T08:25:00+00:00")
                .ok()
                .map(|d| d.with_timezone(&Utc)),
            points: vec![
                "The Treasury Department on Wednesday will announce the size of a buyback \
                 operation it is slated to begin on long-dated U.S. debt."
                    .into(),
                "While a prior announcement indicated the total would be at least $4 billion, \
                 analysts see the potential for the number to climb significantly higher."
                    .into(),
                "\"I'm the house now,\" Treasury Secretary Scott Bessent warned currency \
                 traders this week."
                    .into(),
            ],
            summary: String::new(),
            domain: "cnbc.com".into(),
            ticker: Some(Ticker {
                name: "US 10-year".into(),
                level: "4.814%".into(),
                change: "+1.0 bp".into(),
                percent: "+0.21%".into(),
                up: true,
                closes: vec![
                    4.62, 4.65, 4.71, 4.70, 4.74, 4.78, 4.77, 4.79, 4.75, 4.76, 4.80, 4.83, 4.79,
                    4.78, 4.81, 4.85, 4.84, 4.82, 4.80, 4.79, 4.81, 4.814,
                ],
            }),
        }
    }

    #[test]
    fn the_card_is_a_full_16_by_9_bitmap_with_text_on_it() {
        let img = render(&card());
        assert_eq!((img.width(), img.height()), (WIDTH, HEIGHT));
        let painted = img.pixels().filter(|p| **p != BACKGROUND).count();
        assert!(
            painted > 20_000,
            "only {painted} pixels differ from the background"
        );
    }

    #[test]
    fn the_png_has_the_png_signature() {
        let bytes = png(&render(&card())).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    /// The frame is drawn on every edge, in the border colour.
    #[test]
    fn the_window_frame_runs_round_all_four_edges() {
        let img = render(&card());
        let mid_x = WIDTH / 2;
        let mid_y = HEIGHT / 2;
        assert_eq!(*img.get_pixel(mid_x, FRAME as u32), BORDER, "top edge");
        assert_eq!(
            *img.get_pixel(mid_x, HEIGHT - FRAME as u32),
            BORDER,
            "bottom edge"
        );
        assert_eq!(*img.get_pixel(FRAME as u32, mid_y), BORDER, "left edge");
        assert_eq!(
            *img.get_pixel(WIDTH - FRAME as u32, mid_y),
            BORDER,
            "right edge"
        );
    }

    /// A rising instrument paints green bars; a falling one, red.
    #[test]
    fn the_ticker_strip_is_coloured_by_the_direction_of_the_move() {
        let up = render(&card());
        assert!(up.pixels().any(|p| *p == GREEN));
        assert!(!up.pixels().any(|p| *p == RED));

        let mut c = card();
        c.ticker.as_mut().unwrap().up = false;
        let down = render(&c);
        assert!(down.pixels().any(|p| *p == RED));
        assert!(!down.pixels().any(|p| *p == GREEN));
    }

    #[test]
    fn a_card_without_a_ticker_or_points_still_renders() {
        let mut c = card();
        c.ticker = None;
        c.points.clear();
        c.summary = "Nonfarm payrolls were expected to increase by 53,000.".into();
        let img = render(&c);
        assert_eq!(img.width(), WIDTH);
        assert!(!img.pixels().any(|p| *p == GREEN || *p == RED));
    }

    #[test]
    fn a_ticker_with_no_history_draws_no_bars() {
        let mut c = card();
        c.ticker.as_mut().unwrap().closes.clear();
        let img = render(&c);
        // The move is still green, but far fewer pixels of it without bars.
        let green = img.pixels().filter(|p| **p == GREEN).count();
        let with_bars = render(&card()).pixels().filter(|p| **p == GREEN).count();
        assert!(green > 0 && green < with_bars);
    }

    #[test]
    fn a_short_headline_gets_the_largest_size() {
        let (size, lines) = fit_headline(&bold(), "Oil tops $100");
        assert_eq!(size, HEADLINE_SIZES[0]);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn a_long_headline_shrinks_to_fit_three_lines() {
        let title = "Treasury Secretary Bessent says the department will announce the size of \
                     its buyback operation on Wednesday morning as bond markets steady after \
                     a week of selling that pushed the 30-year yield past 5%";
        let (size, lines) = fit_headline(&bold(), title);
        assert!(size < HEADLINE_SIZES[0]);
        assert!(lines.len() <= HEADLINE_LINES);
    }

    #[test]
    fn an_endless_headline_is_cut_with_an_ellipsis() {
        let title = "word ".repeat(120);
        let (_, lines) = fit_headline(&bold(), &title);
        assert_eq!(lines.len(), HEADLINE_LINES);
        assert!(lines[2].ends_with('\u{2026}'));
    }

    #[test]
    fn a_point_that_runs_long_is_cut_with_an_ellipsis_not_silently() {
        let font = FontRef::try_from_slice(REGULAR).unwrap();
        let text = "word ".repeat(80);
        let lines = cut(
            &font,
            BASE,
            wrap(&font, BASE, &text, 600.0),
            600.0,
            POINT_LINES,
        );
        assert_eq!(lines.len(), POINT_LINES);
        assert!(lines[POINT_LINES - 1].ends_with('\u{2026}'));
        assert!(measure(&font, BASE, &lines[POINT_LINES - 1]) <= 600.0);
        let short = cut(&font, BASE, vec!["fits".into()], 600.0, POINT_LINES);
        assert_eq!(short, vec!["fits"]);
    }

    #[test]
    fn wrapping_keeps_every_line_inside_the_width() {
        let font = bold();
        let text = "The quick brown fox jumps over the lazy dog, again and again and again";
        for line in wrap(&font, 40.0, text, 500.0) {
            assert!(measure(&font, 40.0, &line) <= 500.0, "{line}");
        }
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken_rather_than_overflowing() {
        let font = bold();
        let lines = wrap(
            &font,
            40.0,
            "Donaudampfschifffahrtsgesellschaftskapitän",
            300.0,
        );
        assert!(lines.len() > 1);
        for line in lines {
            assert!(measure(&font, 40.0, &line) <= 300.0, "{line}");
        }
    }

    /// The layout assumes a monospace face: every cell the same width.
    #[test]
    fn the_embedded_face_is_monospace_and_has_the_box_glyphs() {
        let font = FontRef::try_from_slice(REGULAR).unwrap();
        let w = measure(&font, BASE, "i");
        assert_eq!(measure(&font, BASE, "W"), w);
        assert!((w - char_width(BASE)).abs() < 0.01, "cell is {w}");
        for c in ['\u{25b8}', '\u{2500}', '\u{b7}', '\u{2026}'] {
            assert_ne!(font.glyph_id(c).0, 0, "{c:?} missing from the subset");
        }
    }

    #[test]
    fn slugs_are_lowercase_hyphenated_and_bounded() {
        assert_eq!(
            slug("U.S. payrolls rose 162,000!"),
            "u-s-payrolls-rose-162-000"
        );
        assert_eq!(slug("Japan\u{2019}s yen"), "japan-s-yen");
        assert_eq!(slug("???"), "story");
        assert!(slug(&"word ".repeat(40)).chars().count() <= 60);
    }

    #[test]
    fn the_domain_drops_the_scheme_the_www_and_the_path() {
        assert_eq!(domain("https://www.cnbc.com/2026/09/09/a.html"), "cnbc.com");
        assert_eq!(domain("https://cnbc.com"), "cnbc.com");
        assert_eq!(domain("nonsense"), "nonsense");
    }

    /// The whole path, on a real desktop: writes the file and takes the
    /// clipboard. Ignored because it needs a display server.
    #[test]
    #[ignore = "writes a file and takes the clipboard"]
    fn share_a_sample_card() {
        let note = share(&card()).unwrap();
        println!("{note}");
        assert!(note.starts_with("Card copied"));
    }

    /// Writes the sample card to `target/card-sample.png` for a look.
    #[test]
    #[ignore = "writes a file to look at"]
    fn write_a_sample_card() {
        let bytes = png(&render(&card())).unwrap();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/card-sample.png"),
            bytes,
        )
        .unwrap();
    }
}
