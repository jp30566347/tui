pub mod article;
pub mod models;
pub mod rss;

use std::collections::HashMap;
use std::time::Duration;

use article::Article;
use color_eyre::eyre::{Context, Result};
use models::*;
use rss::{Headline, Source};
use tui_common::http::preview;

const QUOTE_URL: &str = "https://quote.cnbc.com/quote-html-webservice/restQuote/symbolType/symbol\
     ?requestMethod=itv&noform=1&partnerId=2&fund=1&exthrs=1&output=json&events=1";
const HISTORY_URL: &str = "https://api.wsj.net/api/michelangelo/timeseries/history?ckey=cecc4267a0";
/// Published in MarketWatch's own page scripts; the endpoint rejects the
/// request without it, in both the header and the body.
const ENTITLEMENT_TOKEN: &str = "cecc4267a0194af89c1d2a1d05dd7d5e";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on a response body. The largest here is an article page at under a
/// megabyte, so this is a wide margin; it exists so a hostile or
/// malfunctioning endpoint cannot exhaust memory.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Cheap to clone: `reqwest::Client` is internally reference counted.
#[derive(Clone)]
pub struct MarketClient {
    client: reqwest::Client,
}

impl Default for MarketClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                // Load-bearing, not cosmetic: the quote endpoint sits behind a
                // CDN that answers the default client string with a 403.
                .user_agent(concat!("macro-tui/", env!("CARGO_PKG_VERSION")))
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    async fn get_text(&self, url: &str, what: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("{what}: request failed"))?
            .error_for_status()
            .with_context(|| format!("{what}: bad status"))?;
        tui_common::http::read_body(resp, what, MAX_BODY_BYTES).await
    }

    /// Every instrument's quote in one request.
    ///
    /// Returns a map rather than a list: the response has matched the request
    /// order in practice, but placing prices by position would put gold's
    /// price on the copper row the first time that changes.
    pub async fn get_quotes(&self, symbols: &str) -> Result<HashMap<String, RawQuote>> {
        let url = format!("{QUOTE_URL}&symbols={}", urlencode(symbols));
        let body = self.get_text(&url, "quotes").await?;
        let parsed: QuoteResponse = serde_json::from_str(&body)
            .with_context(|| format!("quotes: could not parse response ({})", preview(&body)))?;
        Ok(parsed
            .result
            .quotes
            .into_iter()
            .filter_map(|q| q.symbol.clone().map(|s| (s, q)))
            .collect())
    }

    /// Daily closes for a set of instruments.
    ///
    /// Sent as POST rather than GET: the endpoint documents a GET form, but
    /// the request is a JSON blob in the query string and anything past
    /// roughly five series exceeds the server's URL length limit and 404s.
    /// POST carries all twenty-six in one ~32 KB response.
    ///
    /// One unrecognised key fails the whole batch with a 400, so the caller is
    /// expected to fall back to probing keys individually and quarantining the
    /// bad one. See `App::spawn_fetch`.
    pub async fn get_history(
        &self,
        keys: &[&'static str],
        time_frame: &'static str,
    ) -> Result<HashMap<&'static str, Series>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let request = HistoryRequest {
            step: "P1D",
            time_frame,
            entitlement_token: ENTITLEMENT_TOKEN,
            include_mock_tick: true,
            filter_null_slots: false,
            filter_closed_points: true,
            include_closed_slots: false,
            include_official_close: true,
            inject_open: false,
            show_pre_market: false,
            show_after_hours: false,
            show_ath: false,
            include_current_quotes: false,
            reset_todays_after_hours_percent_change: false,
            series: keys
                .iter()
                .enumerate()
                .map(|(n, key)| SeriesRequest {
                    key,
                    dialect: "Charting",
                    kind: "Ticker",
                    series_id: format!("s{n}"),
                    data_types: vec!["Last"],
                    indicators: vec![],
                })
                .collect(),
        };

        let resp = self
            .client
            .post(HISTORY_URL)
            .header("Dylan2010.EntitlementToken", ENTITLEMENT_TOKEN)
            .json(&request)
            .send()
            .await
            .context("history: request failed")?
            .error_for_status()
            .context("history: bad status")?;
        let body = tui_common::http::read_body(resp, "history", MAX_BODY_BYTES).await?;
        let parsed: HistoryResponse = serde_json::from_str(&body)
            .with_context(|| format!("history: could not parse response ({})", preview(&body)))?;

        Ok(keys
            .iter()
            .enumerate()
            .filter_map(|(n, key)| Some((*key, parsed.series(&format!("s{n}"))?)))
            .collect())
    }

    pub async fn get_feed(&self, source: Source) -> Result<Vec<Headline>> {
        let body = self.get_text(source.url(), "news").await?;
        rss::parse(&body, source)
    }

    /// The whole story behind a headline, for the reader.
    pub async fn get_article(&self, url: &str) -> Result<Article> {
        let body = self.get_text(url, "story").await?;
        article::parse(&body)
    }
}

/// Percent-encodes the characters that matter in a query value. The symbol
/// list uses `|` as its separator and contains `@` and `=`, none of which
/// survive unencoded.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_the_symbol_separator_and_suffixes() {
        assert_eq!(urlencode(".SPX|@CL.1|EUR="), ".SPX%7C%40CL.1%7CEUR%3D");
    }

    #[test]
    fn urlencode_leaves_unreserved_characters_alone() {
        assert_eq!(urlencode("US10Y"), "US10Y");
    }

    #[test]
    fn preview_truncates_on_a_character_boundary() {
        let body = "\u{2019}".repeat(300);
        let p = preview(&body);
        assert!(p.ends_with('\u{2026}'));
        assert_eq!(p.chars().count(), 201);
    }

    #[test]
    fn preview_leaves_a_short_body_whole() {
        assert_eq!(preview("oops"), "oops");
    }
}

/// Checks against the live endpoints.
///
/// Ignored by default: CI must not fail because a third party is having a bad
/// day. Run deliberately with `cargo test -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;
    use crate::catalog::{INSTRUMENTS, MEGA_CAPS};

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn every_catalog_symbol_still_returns_a_price() {
        let client = MarketClient::new();
        let quotes = client
            .get_quotes(&crate::catalog::all_symbols())
            .await
            .expect("quote request failed");

        let mut missing = Vec::new();
        for instrument in INSTRUMENTS {
            match quotes.get(instrument.cnbc).and_then(|q| q.parse()) {
                Some(q) => println!("  {:<16} {}", instrument.name, instrument.level(q.last)),
                None => missing.push(instrument.name),
            }
        }
        assert!(missing.is_empty(), "no price for: {missing:?}");
    }

    /// The cohort rides the board's quote request, and its whole point is the
    /// four fields the macro instruments leave null. A silent change to any
    /// of them would empty a column rather than raise an error, so this
    /// asserts on them by name.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn every_mega_cap_still_returns_the_equity_fields() {
        let client = MarketClient::new();
        let quotes = client
            .get_quotes(&crate::catalog::all_symbols())
            .await
            .expect("quote request failed");

        let mut missing = Vec::new();
        for mega in MEGA_CAPS {
            let Some(q) = quotes.get(mega.cnbc).and_then(|q| q.parse()) else {
                missing.push(format!("{}: no price", mega.name));
                continue;
            };
            println!(
                "  {:<14} {:>10.2}  cap {:?}  beta {:?}  52w {:?}-{:?}  vol {:?}",
                mega.name, q.last, q.market_cap, q.beta, q.year_low, q.year_high, q.vol_ratio
            );
            for (field, present) in [
                ("market cap", q.market_cap.is_some()),
                ("beta", q.beta.is_some()),
                ("52-week high", q.year_high.is_some()),
                ("52-week low", q.year_low.is_some()),
                ("volume ratio", q.vol_ratio.is_some()),
            ] {
                if !present {
                    missing.push(format!("{}: no {field}", mega.name));
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    /// The cohort's history keys go in the same batch as the board's, so one
    /// rotted stock key would take the whole board's charts down with it.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn every_mega_cap_history_key_still_resolves() {
        let client = MarketClient::new();
        let keys: Vec<&'static str> = MEGA_CAPS.iter().map(|m| m.history).collect();
        let series = client
            .get_history(&keys, "P1M")
            .await
            .expect("batched history request failed");

        let mut empty = Vec::new();
        for mega in MEGA_CAPS {
            match series.get(mega.history).filter(|s| !s.is_empty()) {
                Some(s) => println!("  {:<14} {} points", mega.name, s.len()),
                None => empty.push(mega.name),
            }
        }
        assert!(empty.is_empty(), "no history for: {empty:?}");
    }

    /// The whole board's history is one batched request, and one unrecognised
    /// key fails all of it, so a rotted key has to be caught here.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn every_history_key_still_resolves_in_one_batch() {
        let client = MarketClient::new();
        let keys: Vec<&'static str> = INSTRUMENTS.iter().filter_map(|i| i.history).collect();
        let series = client
            .get_history(&keys, "P1M")
            .await
            .expect("batched history request failed");

        let mut empty = Vec::new();
        for instrument in INSTRUMENTS {
            let Some(key) = instrument.history else {
                continue;
            };
            match series.get(key).filter(|s| !s.is_empty()) {
                Some(s) => println!("  {:<16} {} points", instrument.name, s.len()),
                None => empty.push(instrument.name),
            }
        }
        assert!(empty.is_empty(), "no history for: {empty:?}");
    }

    /// The reader depends on CNBC's embedded article document. If its shape
    /// moves, this is the check that notices.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn the_newest_story_in_every_feed_still_has_a_body() {
        let client = MarketClient::new();
        for source in Source::ALL {
            let items = client.get_feed(source).await.unwrap();
            let first = &items[0];
            let article = client
                .get_article(&first.link)
                .await
                .unwrap_or_else(|e| panic!("{} failed: {e}", first.link));
            assert!(
                !article.body.is_empty(),
                "{} parsed to an empty body",
                first.link
            );
            println!(
                "  {:<5} {:>2} blocks  {:>2} key points  {}",
                source.as_str(),
                article.body.len(),
                article.key_points.len(),
                article.title
            );
        }
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn every_news_feed_still_parses() {
        let client = MarketClient::new();
        for source in Source::ALL {
            let items = client
                .get_feed(source)
                .await
                .unwrap_or_else(|e| panic!("{} failed: {e}", source.url()));
            assert!(!items.is_empty(), "{} returned no items", source.url());
            println!(
                "  {:<5} {:>3} items  {}",
                source.as_str(),
                items.len(),
                items[0].title
            );
        }
    }
}
