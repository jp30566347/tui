//! RSS parsing for the news pane.
//!
//! The feeds are ordinary RSS 2.0 but they escape differently from each other:
//! CNBC sends plain titles beside CDATA descriptions and namespaced siblings
//! like `metadata:id`, MarketWatch sends numeric character references in its
//! titles, and the FT wraps every field in CDATA. Getting any of that wrong
//! shows up as mojibake in a headline rather than as an error, which is why
//! this uses a real parser.

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Context, Result};
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    CnbcTop,
    CnbcEconomy,
    CnbcFinance,
    MarketWatch,
    FinancialTimes,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::CnbcTop,
        Source::CnbcEconomy,
        Source::CnbcFinance,
        Source::MarketWatch,
        Source::FinancialTimes,
    ];

    pub fn url(self) -> &'static str {
        match self {
            Source::CnbcTop => "https://www.cnbc.com/id/100003114/device/rss/rss.html",
            Source::CnbcEconomy => "https://www.cnbc.com/id/20910258/device/rss/rss.html",
            Source::CnbcFinance => "https://www.cnbc.com/id/10000664/device/rss/rss.html",
            Source::MarketWatch => "https://feeds.content.dowjones.io/public/rss/mw_topstories",
            Source::FinancialTimes => "https://www.ft.com/markets?format=rss",
        }
    }

    /// Short enough for the four-character column in the news list.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::CnbcTop | Source::CnbcEconomy | Source::CnbcFinance => "CNBC",
            Source::MarketWatch => "MW",
            Source::FinancialTimes => "FT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Headline {
    pub title: String,
    pub link: String,
    pub published: Option<DateTime<Utc>>,
    pub source: Source,
    /// Lowercased title and description, built once so the per-frame alias
    /// filter is a plain substring scan.
    pub haystack: String,
}

/// Parses a feed body into headlines.
///
/// Items missing a title or a link are skipped: a headline with nothing to
/// show or nowhere to go is not worth a row. A truncated body yields the items
/// that were readable rather than nothing at all.
pub fn parse(xml: &str, source: Source) -> Result<Vec<Headline>> {
    // Text is deliberately not trimmed by the reader. An entity reference
    // splits its element into several text events, and trimming each one
    // would eat the spaces around it: "veteran &amp; a head" would come back
    // as "veteran&a head". Fields are trimmed once, whole, in `build`.
    let reader = &mut Reader::from_str(xml);

    let mut headlines = Vec::new();
    let mut in_item = false;
    // Which element's text we are currently accumulating. Set on a start tag
    // we care about and cleared on the matching end tag.
    let mut field: Option<Field> = None;
    let mut item = Item::default();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                // Compared against the qualified name rather than the local
                // one, so a namespaced sibling such as CNBC's `metadata:id`
                // or MarketWatch's `dc:creator` cannot shadow a real field.
                match e.name().as_ref() {
                    "item" => {
                        in_item = true;
                        item = Item::default();
                    }
                    "title" if in_item => field = Some(Field::Title),
                    "link" if in_item => field = Some(Field::Link),
                    "description" if in_item => field = Some(Field::Description),
                    "pubDate" if in_item => field = Some(Field::PubDate),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                "item" => {
                    in_item = false;
                    if let Some(headline) = item.build(source) {
                        headlines.push(headline);
                    }
                }
                _ => field = None,
            },
            Ok(Event::Text(e)) => {
                if let Some(f) = field {
                    item.push(f, &e.xml10_content());
                }
            }
            // A CDATA section is already literal and must not be unescaped,
            // or an "&amp;" written inside one would collapse to "&".
            Ok(Event::CData(e)) => {
                if let Some(f) = field {
                    item.push(f, &e.into_inner());
                }
            }
            // Entity references arrive as their own events rather than inside
            // the surrounding text, so resolving them is not optional: skip
            // this and MarketWatch's "Here&#x2019;s" renders as "Heres".
            Ok(Event::GeneralRef(e)) => {
                if let Some(f) = field {
                    match e.resolve_char_ref() {
                        // A numeric reference: "&#x2019;" or "&#8217;".
                        Ok(Some(c)) => item.push(f, &c.to_string()),
                        // A named one: "&amp;", "&apos;". An entity we cannot
                        // resolve is dropped rather than shown raw.
                        _ => {
                            let name = e.into_inner();
                            if let Ok(text) = unescape(&format!("&{name};")) {
                                item.push(f, &text);
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            // A malformed tail should not discard what was already read; the
            // feeds come over the network and can arrive truncated.
            Err(_) if !headlines.is_empty() || in_item => break,
            Err(e) => return Err(e).context("could not parse feed"),
            _ => {}
        }
    }

    // A feed cut off mid-item still has a usable headline in hand, as long as
    // the title and the link both made it through.
    if in_item {
        if let Some(headline) = item.build(source) {
            headlines.push(headline);
        }
    }
    Ok(headlines)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Link,
    Description,
    PubDate,
}

/// Fields accumulate rather than overwrite, because a run of text and CDATA
/// inside one element arrives as several events.
#[derive(Default)]
struct Item {
    title: String,
    link: String,
    description: String,
    pub_date: String,
}

impl Item {
    fn push(&mut self, field: Field, text: &str) {
        let target = match field {
            Field::Title => &mut self.title,
            Field::Link => &mut self.link,
            Field::Description => &mut self.description,
            Field::PubDate => &mut self.pub_date,
        };
        target.push_str(text);
    }

    fn build(&self, source: Source) -> Option<Headline> {
        let title = self.title.trim();
        let link = self.link.trim();
        let description = self.description.trim();
        if title.is_empty() || link.is_empty() {
            return None;
        }
        Some(Headline {
            haystack: format!("{title} {description}").to_lowercase(),
            title: title.to_string(),
            link: link.to_string(),
            published: parse_date(self.pub_date.trim()),
            source,
        })
    }
}

/// RSS dates are RFC 2822. An unparseable or absent one is not fatal; the item
/// simply sorts last.
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed CNBC item: plain title, CDATA description, and the
    /// namespaced siblings that must not be mistaken for real fields.
    const CNBC: &str = r#"<rss><channel>
      <title>US Top News and Analysis</title>
      <item>
        <link>https://www.cnbc.com/2026/09/04/jobs-report-august-2026.html</link>
        <guid isPermaLink="false">108358891</guid>
        <metadata:type>cnbcnewsstory</metadata:type>
        <metadata:id>108358891</metadata:id>
        <title>U.S. payrolls rose 162,000 in August, much more than expected</title>
        <description><![CDATA[Nonfarm payrolls were expected to increase by 53,000.]]></description>
        <pubDate>Fri, 04 Sep 2026 13:52:53 GMT</pubDate>
      </item>
    </channel></rss>"#;

    /// MarketWatch escapes its titles with numeric character references.
    const MW: &str = r#"<rss><channel><item>
        <guid isPermaLink="false">WP-MKTW-0005216921</guid>
        <title>Adobe just announced its next CEO. Here&#x2019;s why its stock is dropping.</title>
        <description>Incoming CEO is a company veteran &amp; a longtime head.</description>
        <link>https://www.marketwatch.com/story/adobe-ceo-bad9ed8a</link>
        <pubDate>Fri, 04 Sep 2026 13:42:00 GMT</pubDate>
        <dc:creator>A Reporter</dc:creator>
      </item></channel></rss>"#;

    /// The FT wraps every single field in CDATA.
    const FT: &str = r#"<rss><channel><item><title><![CDATA[Japan&#8217;s vital link between Bessent and markets]]></title><description><![CDATA[A pivotal figure for relations with Washington]]></description><link>https://www.ft.com/content/d8f53899</link><guid isPermaLink="false">d8f53899</guid><pubDate>Fri, 04 Sep 2026 12:00:06 GMT</pubDate></item></channel></rss>"#;

    #[test]
    fn cnbc_items_yield_a_title_a_link_and_a_timestamp() {
        let items = parse(CNBC, Source::CnbcTop).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].title,
            "U.S. payrolls rose 162,000 in August, much more than expected"
        );
        assert!(items[0].link.ends_with("jobs-report-august-2026.html"));
        assert!(items[0].published.is_some());
    }

    /// The channel also has a `<title>`, and CNBC items carry `metadata:id`.
    /// Neither may become a headline or overwrite one.
    #[test]
    fn elements_outside_an_item_and_namespaced_siblings_are_ignored() {
        let items = parse(CNBC, Source::CnbcTop).unwrap();
        assert_eq!(items.len(), 1);
        assert!(!items[0].title.contains("Top News"));
        assert!(!items[0].title.contains("108358891"));
    }

    #[test]
    fn marketwatch_titles_have_their_character_references_unescaped() {
        let items = parse(MW, Source::MarketWatch).unwrap();
        assert_eq!(
            items[0].title,
            "Adobe just announced its next CEO. Here\u{2019}s why its stock is dropping."
        );
        assert!(items[0].haystack.contains("veteran & a longtime"));
    }

    #[test]
    fn ft_fields_wrapped_in_cdata_come_out_as_plain_text() {
        let items = parse(FT, Source::FinancialTimes).unwrap();
        assert!(items[0].title.starts_with("Japan"));
        assert!(!items[0].title.contains("CDATA"));
        assert_eq!(items[0].link, "https://www.ft.com/content/d8f53899");
    }

    #[test]
    fn the_haystack_is_lowercased_title_and_description() {
        let items = parse(CNBC, Source::CnbcTop).unwrap();
        assert!(items[0].haystack.contains("u.s. payrolls rose"));
        assert!(items[0].haystack.contains("nonfarm payrolls"));
        assert!(!items[0].haystack.contains("U.S."));
    }

    #[test]
    fn an_item_without_a_pubdate_still_parses() {
        let xml = r#"<rss><channel><item><title>No date</title>
                     <link>https://example.com/a</link></item></channel></rss>"#;
        let items = parse(xml, Source::CnbcTop).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].published.is_none());
    }

    #[test]
    fn an_item_with_no_title_or_no_link_is_skipped() {
        let xml = r#"<rss><channel>
            <item><link>https://example.com/a</link></item>
            <item><title>Nowhere to go</title></item>
            <item><title>Good</title><link>https://example.com/b</link></item>
          </channel></rss>"#;
        let items = parse(xml, Source::CnbcTop).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Good");
    }

    /// A feed can arrive cut short. Whatever was readable is still worth
    /// showing.
    #[test]
    fn a_truncated_feed_yields_the_items_it_managed_to_read() {
        let truncated = &FT[..FT.len() - 40];
        let items = parse(truncated, Source::FinancialTimes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn every_source_has_a_distinct_url() {
        let mut urls: Vec<&str> = Source::ALL.iter().map(|s| s.url()).collect();
        urls.sort_unstable();
        let count = urls.len();
        urls.dedup();
        assert_eq!(urls.len(), count);
    }
}
