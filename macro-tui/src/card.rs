//! The share card: a story rendered as an image, for pasting into a post.
//!
//! A timeline shows an attached image at 16:9, so the card is 1600 by 900 and
//! its layout is fixed: a section kicker and the date across the top, the
//! headline as large as it can be while still fitting on three lines, CNBC's
//! key points or the feed's summary beneath, and the publisher's domain in
//! the footer beside the app's name. Everything is drawn here, with fonts
//! compiled into the binary, so the card looks the same on every machine and
//! needs nothing installed.

use std::borrow::Cow;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use chrono::{DateTime, Local, Utc};
use image::{ImageFormat, Rgba, RgbaImage};

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
const MARGIN: f32 = 96.0;
const CONTENT_WIDTH: f32 = WIDTH as f32 - 2.0 * MARGIN;
/// A headline longer than this is shrunk, then cut, rather than pushed into
/// the summary.
const HEADLINE_LINES: usize = 3;
const HEADLINE_SIZES: [f32; 4] = [68.0, 60.0, 52.0, 46.0];
const POINT_SIZE: f32 = 33.0;
const SUMMARY_SIZE: f32 = 34.0;
const META_SIZE: f32 = 28.0;
const MAX_POINTS: usize = 4;

// Subset to Latin, with the kerning folded into a legacy `kern` table, which
// is the one ab_glyph reads. See assets/fonts/LICENSE.txt.
const REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const DISPLAY: &[u8] = include_bytes!("../assets/fonts/InterDisplay-SemiBold.ttf");

const BACKGROUND: Rgba<u8> = Rgba([15, 20, 26, 255]);
const TEXT: Rgba<u8> = Rgba([243, 245, 247, 255]);
const BODY: Rgba<u8> = Rgba([205, 214, 223, 255]);
const MUTED: Rgba<u8> = Rgba([139, 152, 165, 255]);
const ACCENT: Rgba<u8> = Rgba([56, 189, 248, 255]);
const RULE: Rgba<u8> = Rgba([36, 46, 58, 255]);

/// Everything the card shows. Built from app state by the key handler, so
/// rendering needs nothing but this.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    let medium = FontRef::try_from_slice(MEDIUM).expect("embedded font");
    let display = FontRef::try_from_slice(DISPLAY).expect("embedded font");

    let mut img = RgbaImage::from_pixel(WIDTH, HEIGHT, BACKGROUND);
    fill(&mut img, 0, 0, WIDTH, 6, ACCENT);

    // Kicker on the left, date on the right, sharing a baseline.
    let mut y = MARGIN + META_SIZE;
    let (publisher, section) = card
        .kicker
        .split_once(" \u{b7} ")
        .unwrap_or((card.kicker.as_str(), ""));
    let mut x = MARGIN;
    x += draw(&mut img, &medium, META_SIZE, x, y, publisher, ACCENT);
    if !section.is_empty() {
        draw(
            &mut img,
            &regular,
            META_SIZE,
            x,
            y,
            &format!("  \u{b7}  {section}"),
            MUTED,
        );
    }
    if let Some(date) = card.published {
        let label = date.with_timezone(&Local).format("%-d %b %Y").to_string();
        let w = measure(&regular, META_SIZE, &label);
        draw(
            &mut img,
            &regular,
            META_SIZE,
            WIDTH as f32 - MARGIN - w,
            y,
            &label,
            MUTED,
        );
    }

    // The headline, at the largest size that fits.
    let (size, lines) = fit_headline(&display, &card.title);
    y += 64.0;
    let line_height = size * 1.14;
    for line in &lines {
        y += size;
        draw(&mut img, &display, size, MARGIN, y, line, TEXT);
        y += line_height - size;
    }

    // The footer's top edge is where the body has to stop.
    let footer_top = HEIGHT as f32 - MARGIN - META_SIZE - 24.0;
    y += 40.0;

    if card.points.is_empty() {
        let line_height = SUMMARY_SIZE * 1.4;
        for line in wrap(&regular, SUMMARY_SIZE, &card.summary, CONTENT_WIDTH)
            .into_iter()
            .take(4)
        {
            if y + line_height > footer_top {
                break;
            }
            y += SUMMARY_SIZE;
            draw(&mut img, &regular, SUMMARY_SIZE, MARGIN, y, &line, BODY);
            y += line_height - SUMMARY_SIZE;
        }
    } else {
        let indent = 40.0;
        let line_height = POINT_SIZE * 1.4;
        for text in card.points.iter().take(MAX_POINTS) {
            let lines: Vec<String> = wrap(&regular, POINT_SIZE, text, CONTENT_WIDTH - indent)
                .into_iter()
                .take(2)
                .collect();
            let needed = lines.len() as f32 * line_height;
            if y + needed > footer_top {
                break;
            }
            // A dot on the first line's x-height, then a hanging indent.
            dot(&mut img, MARGIN + 8.0, y + POINT_SIZE * 0.62, 5.0, ACCENT);
            for line in &lines {
                y += POINT_SIZE;
                draw(
                    &mut img,
                    &regular,
                    POINT_SIZE,
                    MARGIN + indent,
                    y,
                    line,
                    BODY,
                );
                y += line_height - POINT_SIZE;
            }
            y += 14.0;
        }
    }

    // Footer: a hairline, the domain, and the app's name.
    let rule_y = (HEIGHT as f32 - MARGIN - META_SIZE - 8.0) as u32;
    fill(
        &mut img,
        MARGIN as u32,
        rule_y,
        CONTENT_WIDTH as u32,
        1,
        RULE,
    );
    let baseline = HEIGHT as f32 - MARGIN + 8.0;
    draw(
        &mut img,
        &medium,
        META_SIZE - 2.0,
        MARGIN,
        baseline,
        &card.domain,
        MUTED,
    );
    let brand = "macro-tui";
    let w = measure(&regular, META_SIZE - 2.0, brand);
    draw(
        &mut img,
        &regular,
        META_SIZE - 2.0,
        WIDTH as f32 - MARGIN - w,
        baseline,
        brand,
        MUTED,
    );

    img
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
    let mut lines = wrap(font, size, title, CONTENT_WIDTH);
    lines.truncate(HEADLINE_LINES);
    if let Some(last) = lines.last_mut() {
        // Make room for the ellipsis by dropping trailing words until it fits.
        loop {
            let candidate = format!("{}\u{2026}", last.trim_end());
            if measure(font, size, &candidate) <= CONTENT_WIDTH || !last.contains(' ') {
                *last = candidate;
                break;
            }
            let cut = last.rfind(' ').unwrap_or(0);
            last.truncate(cut);
        }
    }
    (size, lines)
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
    let scale = PxScale::from(size);
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

/// Positions each glyph with kerning applied, calling `place` with its id and
/// x offset, and returns the total advance.
fn layout(font: &FontRef, size: f32, text: &str, mut place: impl FnMut(GlyphId, f32)) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
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

/// An anti-aliased filled circle.
fn dot(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, colour: Rgba<u8>) {
    let (x0, x1) = ((cx - r - 1.0) as i32, (cx + r + 1.0) as i32);
    let (y0, y1) = ((cy - r - 1.0) as i32, (cy + r + 1.0) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let coverage = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            blend(img, x, y, colour, coverage);
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

    fn display() -> FontRef<'static> {
        FontRef::try_from_slice(DISPLAY).unwrap()
    }

    fn card() -> Card {
        Card {
            title: "U.S. payrolls rose 162,000 in August, much more than expected".into(),
            kicker: "CNBC \u{b7} Economy".into(),
            published: DateTime::parse_from_rfc3339("2026-09-04T13:52:53+00:00")
                .ok()
                .map(|d| d.with_timezone(&Utc)),
            points: vec![
                "Nonfarm payrolls were expected to increase by 53,000.".into(),
                "The unemployment rate held at 4.3%, in line with the estimate.".into(),
                "Average hourly earnings rose 0.3% for the month and 3.7% from a year ago, both ahead of forecasts from economists surveyed by Dow Jones.".into(),
            ],
            summary: String::new(),
            domain: "cnbc.com".into(),
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

    #[test]
    fn a_card_with_no_points_falls_back_to_its_summary_without_panicking() {
        let mut c = card();
        c.points.clear();
        c.summary = "Nonfarm payrolls were expected to increase by 53,000.".into();
        let img = render(&c);
        assert_eq!(img.width(), WIDTH);
    }

    #[test]
    fn a_short_headline_gets_the_largest_size() {
        let (size, lines) = fit_headline(&display(), "Oil tops $100");
        assert_eq!(size, HEADLINE_SIZES[0]);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn a_long_headline_shrinks_to_fit_three_lines() {
        let title = "Treasury Secretary Bessent says the department will announce the size of \
                     its buyback operation on Wednesday morning as bond markets steady after \
                     a week of selling that pushed the 30-year yield past 5%";
        let (size, lines) = fit_headline(&display(), title);
        assert!(size < HEADLINE_SIZES[0]);
        assert!(lines.len() <= HEADLINE_LINES);
    }

    #[test]
    fn an_endless_headline_is_cut_with_an_ellipsis() {
        let title = "word ".repeat(120);
        let (_, lines) = fit_headline(&display(), &title);
        assert_eq!(lines.len(), HEADLINE_LINES);
        assert!(lines[2].ends_with('\u{2026}'));
    }

    #[test]
    fn wrapping_keeps_every_line_inside_the_width() {
        let font = display();
        let text = "The quick brown fox jumps over the lazy dog, again and again and again";
        for line in wrap(&font, 60.0, text, 500.0) {
            assert!(measure(&font, 60.0, &line) <= 500.0, "{line}");
        }
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken_rather_than_overflowing() {
        let font = display();
        let lines = wrap(
            &font,
            60.0,
            "Donaudampfschifffahrtsgesellschaftskapitän",
            300.0,
        );
        assert!(lines.len() > 1);
        for line in lines {
            assert!(measure(&font, 60.0, &line) <= 300.0, "{line}");
        }
    }

    #[test]
    fn kerning_is_read_from_the_embedded_fonts() {
        let font = display();
        let apart = measure(&font, 60.0, "A") + measure(&font, 60.0, "V");
        assert!(measure(&font, 60.0, "AV") < apart, "AV should kern tighter");
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
