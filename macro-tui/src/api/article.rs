//! The story behind a headline, read out of a CNBC article page.
//!
//! CNBC renders its pages from a JSON document it embeds as `window.__s_data`
//! for the browser to hydrate from. That document carries the article as a
//! tree of typed nodes, which is a far steadier thing to read than the markup
//! around it: class names change with every redesign, while the data shape is
//! what CNBC's own front end depends on. Anything that is not text (images,
//! embedded video, interactive charts) is dropped rather than described.

use chrono::{DateTime, Utc};
use color_eyre::eyre::{eyre, Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(String),
    Heading(String),
    Bullet(String),
    Quote(String),
}

#[derive(Debug, Clone, Default)]
pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    /// The section eyebrow the page runs under, such as "Politics & Policy".
    pub section: Option<String>,
    pub published: Option<DateTime<Utc>>,
    /// CNBC's own summary bullets, when the story has them.
    pub key_points: Vec<String>,
    pub body: Vec<Block>,
    /// A CNBC Pro story. The page carries only the free preview, and the
    /// reader should say so rather than let the story simply stop.
    pub premium: bool,
}

/// Parses an article page into its text.
///
/// Fails when the page has no embedded document or the document has no story
/// in it, both of which mean the page is not an article (or CNBC changed its
/// front end) and there is nothing sensible to show.
pub fn parse(html: &str) -> Result<Article> {
    let json = embedded_json(html).ok_or_else(|| eyre!("story: no article data in the page"))?;
    let root: Value = serde_json::from_str(json).context("story: could not parse article data")?;
    let page = root
        .pointer("/page/page")
        .filter(|p| p.is_object())
        .ok_or_else(|| eyre!("story: article data has no page"))?;

    let mut article = Article {
        title: text_field(page, "headline")
            .or_else(|| text_field(page, "title"))
            .unwrap_or_default(),
        byline: byline(page),
        section: page
            .pointer("/sectionHierarchy/0/eyebrow")
            .and_then(Value::as_str)
            .or_else(|| page.pointer("/section/title").and_then(Value::as_str))
            .map(str::to_string),
        published: page
            .get("datePublished")
            .and_then(Value::as_str)
            .and_then(parse_date),
        premium: page
            .get("premium")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..Article::default()
    };

    let mut found_body = false;
    for module in modules(page) {
        let Some(data) = module.get("data").filter(|d| d.is_object()) else {
            continue;
        };
        // The key points arrive as a `ul`; only its items are wanted.
        if let Some(points) = data.get("keyPoints").and_then(Value::as_array) {
            let mut blocks = Vec::new();
            for node in points {
                collect(node, &mut blocks);
            }
            article
                .key_points
                .extend(blocks.into_iter().filter_map(|b| match b {
                    Block::Bullet(line) => Some(line),
                    _ => None,
                }));
        }
        // The first module with a body is the story. Later ones are embeds
        // and related links that happen to share the shape.
        if !found_body {
            if let Some(content) = data.pointer("/body/content").and_then(Value::as_array) {
                found_body = true;
                for node in content {
                    collect(node, &mut article.body);
                }
            }
        }
    }
    if !found_body {
        return Err(eyre!("story: the page has no article body"));
    }
    Ok(article)
}

/// The JSON assigned to `window.__s_data`, located by matching braces rather
/// than by looking for the closing script tag, because the document itself
/// contains scripts.
fn embedded_json(html: &str) -> Option<&str> {
    let at = html.find("window.__s_data")?;
    let rest = &html[at..];
    let start = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (n, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[start..=n]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every module in the page's layout, in reading order.
fn modules(page: &Value) -> impl Iterator<Item = &Value> {
    page.get("layout")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|layout| {
            layout
                .get("columns")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .flat_map(|column| {
            column
                .get("modules")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
}

/// Walks a node tree, appending text blocks in reading order.
fn collect(node: &Value, blocks: &mut Vec<Block>) {
    let Some(tag) = node.get("tagName").and_then(Value::as_str) else {
        return;
    };
    match tag {
        "p" => push(blocks, Block::Paragraph, text(node)),
        "h2" | "h3" | "subtitle" => push(blocks, Block::Heading, text(node)),
        "blockquote" => push(blocks, Block::Quote, text(node)),
        "li" => push(blocks, Block::Bullet, text(node)),
        // Containers: the text is in the children.
        "div" | "group" | "ul" | "ol" | "section" => {
            for child in children(node) {
                collect(child, blocks);
            }
        }
        // Images, video, charts, related-story cards and every other embed.
        _ => {}
    }
}

fn push(blocks: &mut Vec<Block>, make: fn(String) -> Block, line: String) {
    if !line.is_empty() {
        blocks.push(make(line));
    }
}

fn children(node: &Value) -> impl Iterator<Item = &Value> {
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The text of a node and everything inside it, with whitespace collapsed.
fn text(node: &Value) -> String {
    let mut out = String::new();
    gather(node, &mut out);
    collapse(&out)
}

fn gather(node: &Value, out: &mut String) {
    match node {
        Value::String(s) => out.push_str(s),
        Value::Object(_) => {
            if node.get("tagName").and_then(Value::as_str) == Some("br") {
                out.push(' ');
            }
            for child in children(node) {
                gather(child, out);
            }
        }
        _ => {}
    }
}

/// Runs of whitespace, including the non-breaking spaces CNBC's editor
/// sprinkles in, become one space.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
    out.trim_end().to_string()
}

fn text_field(page: &Value, key: &str) -> Option<String> {
    page.get(key)
        .and_then(Value::as_str)
        .map(collapse)
        .filter(|s| !s.is_empty())
}

fn byline(page: &Value) -> Option<String> {
    let names: Vec<&str> = page
        .get("author")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("name").and_then(Value::as_str))
        .filter(|n| !n.trim().is_empty())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// CNBC writes `2026-09-08T19:14:45+0000`, which is RFC 3339 apart from the
/// missing colon in the offset.
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z")
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed page: the document sits between other scripts, has braces
    /// inside strings, and carries an image, a video and a related-story
    /// embed alongside the text.
    const PAGE: &str = r##"<html><head><script>window.__a={"x":"{"}</script>
<script>window.__s_data={"routing":{"x":null},"page":{"page":{
 "headline":"U.S. denies claims Iran struck two American vessels",
 "title":"U.S. denies claims Iran struck two American vessels",
 "description":"The U.S. denied Iran’s claim.",
 "datePublished":"2026-09-08T19:14:45+0000",
 "premium":false,
 "author":[{"name":"Anniek Bao"},{"name":"Spencer Kimball"}],
 "sectionHierarchy":[{"eyebrow":"Politics & Policy"}],
 "layout":[
  {"columns":[{"modules":[{"name":"articleHeader","data":{"headline":"x"}}]}]},
  {"columns":[{"modules":[
   {"name":"keyPoints","data":{"keyPoints":[{"tagName":"group","children":[{"tagName":"div","children":[{"tagName":"ul","children":[
     {"tagName":"li","children":["First point."]},
     {"tagName":"li","children":["Second ","point with a ",{"tagName":"a","attributes":{"href":"https://x"},"children":["link"]},"."]}
   ]}]}]}]}},
   {"name":"articleBody","data":{"body":{"content":[
     {"tagName":"image","attributes":{"caption":"A ship {at sea}"}},
     {"tagName":"div","attributes":{"className":["group"]},"children":[
       {"tagName":"p","children":["The U.S. has denied a claim by Iran’s   Revolutionary Guard."]},
       {"tagName":"p","children":["It said the ships were in a zone it called ",{"tagName":"em","children":["forbidden"]},{"tagName":"br"},"and unsafe."]}
     ]},
     {"tagName":"subtitle","children":["Brent crude hits $100 a barrel"]},
     {"tagName":"cnbcvideo","attributes":{"headline":"Oil tops $100"}},
     {"tagName":"div","attributes":{"className":["group"]},"children":[
       {"tagName":"blockquote","children":[{"tagName":"p","children":["No warship has been struck."]}]},
       {"tagName":"p","children":["   "]},
       {"tagName":"ul","children":[{"tagName":"li","children":["A bullet in the body."]}]},
       {"tagName":"p","children":["After the bullet."]}
     ]},
     {"tagName":"content_embed","data":{"body":{"content":[{"tagName":"p","children":["Read more coverage"]}]}}}
   ]}}}
  ]}]}
 ]}}};</script><script>window.__b=1</script></head><body></body></html>"##;

    #[test]
    fn the_header_fields_come_from_the_page() {
        let a = parse(PAGE).unwrap();
        assert_eq!(
            a.title,
            "U.S. denies claims Iran struck two American vessels"
        );
        assert_eq!(a.byline.as_deref(), Some("Anniek Bao, Spencer Kimball"));
        assert_eq!(a.section.as_deref(), Some("Politics & Policy"));
        assert_eq!(
            a.published.unwrap().to_rfc3339(),
            "2026-09-08T19:14:45+00:00"
        );
        assert!(!a.premium);
    }

    #[test]
    fn key_points_are_read_as_plain_lines_with_their_links_flattened() {
        let a = parse(PAGE).unwrap();
        assert_eq!(
            a.key_points,
            vec!["First point.", "Second point with a link."]
        );
    }

    /// Text blocks keep their order; images, video and the related-story
    /// embed vanish without leaving a gap.
    #[test]
    fn the_body_is_the_text_in_reading_order_without_the_embeds() {
        let a = parse(PAGE).unwrap();
        assert_eq!(
            a.body,
            vec![
                Block::Paragraph(
                    "The U.S. has denied a claim by Iran\u{2019}s Revolutionary Guard.".into()
                ),
                Block::Paragraph(
                    "It said the ships were in a zone it called forbidden and unsafe.".into()
                ),
                Block::Heading("Brent crude hits $100 a barrel".into()),
                Block::Quote("No warship has been struck.".into()),
                Block::Bullet("A bullet in the body.".into()),
                Block::Paragraph("After the bullet.".into()),
            ]
        );
    }

    /// The embed after the story has the same `body.content` shape. Reading
    /// it too would append "Read more coverage" to every article.
    #[test]
    fn only_the_first_body_in_the_page_is_the_story() {
        let a = parse(PAGE).unwrap();
        assert!(!a
            .body
            .iter()
            .any(|b| matches!(b, Block::Paragraph(p) if p.contains("Read more"))));
    }

    #[test]
    fn a_page_without_the_embedded_document_is_an_error() {
        assert!(parse("<html><body><p>Hello</p></body></html>").is_err());
    }

    #[test]
    fn a_document_without_a_story_is_an_error() {
        let html =
            r#"<script>window.__s_data={"page":{"page":{"headline":"x","layout":[]}}}</script>"#;
        assert!(parse(html).is_err());
    }

    #[test]
    fn brace_matching_survives_braces_and_escaped_quotes_inside_strings() {
        let html = r#"window.__s_data={"a":"}\"{","b":{"c":1}};"#;
        assert_eq!(embedded_json(html), Some(r#"{"a":"}\"{","b":{"c":1}}"#));
    }

    #[test]
    fn a_premium_story_is_flagged() {
        let html = PAGE.replace(r#""premium":false"#, r#""premium":true"#);
        assert!(parse(&html).unwrap().premium);
    }

    #[test]
    fn whitespace_runs_collapse_to_one_space() {
        assert_eq!(collapse("  a \u{a0}\n b  "), "a b");
    }
}
