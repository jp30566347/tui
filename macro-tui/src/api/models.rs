//! Wire shapes for the quote and history endpoints, and the parsing that turns
//! their preformatted strings back into numbers.

use serde::Deserialize;

// --- CNBC quotes ---------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QuoteResponse {
    #[serde(rename = "FormattedQuoteResult")]
    pub result: QuoteResult,
}

#[derive(Debug, Deserialize)]
pub struct QuoteResult {
    #[serde(rename = "FormattedQuote", default)]
    pub quotes: Vec<RawQuote>,
}

/// One row as the endpoint sends it.
///
/// Every field is optional because an unrecognised symbol comes back as a
/// populated object with `code` 1 and nulls everywhere else, rather than as an
/// error or an omission.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RawQuote {
    pub symbol: Option<String>,
    /// 0 means the row is good. Anything else means the symbol was not
    /// recognised and no other field can be trusted.
    pub code: i64,
    pub name: Option<String>,
    pub last: Option<String>,
    pub change: Option<String>,
    pub change_pct: Option<String>,
    pub open: Option<String>,
    pub high: Option<String>,
    pub low: Option<String>,
    pub previous_day_closing: Option<String>,
    pub yrhiprice: Option<String>,
    pub yrloprice: Option<String>,
    /// "REG_MKT", "AFT_MKT", "PRE_MKT" or "CLOSED".
    pub curmktstatus: Option<String>,
    /// RFC 3339 with an offset. Preferred over `last_timedate`, which is a
    /// display string in an unspecified zone.
    pub last_time: Option<String>,
}

/// A parsed row, ready to render.
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub last: f64,
    /// Absolute move since the previous close.
    pub change: f64,
    /// Percent move since the previous close.
    pub change_pct: f64,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    pub year_high: Option<f64>,
    pub year_low: Option<f64>,
    pub market_status: Option<String>,
}

impl RawQuote {
    /// Parses a row, or returns `None` when the endpoint did not recognise the
    /// symbol or sent no price.
    ///
    /// The change and the percent change are recomputed from the last price
    /// and the previous close rather than read off the response, because the
    /// reported `change_pct` is not consistent across instrument types: for
    /// the 2-year note it carries the percent change in the bond's *price*,
    /// which has the opposite sign to the change in its yield. Taking it at
    /// face value paints a rising yield red. The reported fields are used only
    /// when there is no previous close to compute from.
    pub fn parse(&self) -> Option<Quote> {
        if self.code != 0 {
            return None;
        }
        let last = number(self.last.as_deref())?;
        let prev_close = number(self.previous_day_closing.as_deref());

        let (change, change_pct) = match prev_close {
            Some(prev) if prev != 0.0 => (last - prev, (last - prev) / prev * 100.0),
            _ => (
                number(self.change.as_deref()).unwrap_or(0.0),
                number(self.change_pct.as_deref()).unwrap_or(0.0),
            ),
        };

        Some(Quote {
            last,
            change,
            change_pct,
            open: number(self.open.as_deref()),
            high: number(self.high.as_deref()),
            low: number(self.low.as_deref()),
            prev_close,
            year_high: number(self.yrhiprice.as_deref()),
            year_low: number(self.yrloprice.as_deref()),
            market_status: self.curmktstatus.clone(),
        })
    }
}

/// Parses one of the endpoint's display strings into a number.
///
/// They arrive with thousands separators, a leading sign, and sometimes a
/// trailing percent: "7,738.79", "+0.043", "4.768%", "UNCH".
fn number(s: Option<&str>) -> Option<f64> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '%' | '+' | '$' | ' '))
        .collect();
    cleaned.parse().ok()
}

// --- MarketWatch history -------------------------------------------------

/// The request body. Sent as JSON over POST; the field names are the wire
/// names, so `rename_all` does the whole job.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HistoryRequest {
    /// "P1D" for daily points.
    pub step: &'static str,
    /// "P1M", "P6M" and so on.
    pub time_frame: &'static str,
    pub entitlement_token: &'static str,
    pub include_mock_tick: bool,
    pub filter_null_slots: bool,
    pub filter_closed_points: bool,
    pub include_closed_slots: bool,
    pub include_official_close: bool,
    pub inject_open: bool,
    pub show_pre_market: bool,
    pub show_after_hours: bool,
    #[serde(rename = "ShowATH")]
    pub show_ath: bool,
    pub include_current_quotes: bool,
    pub reset_todays_after_hours_percent_change: bool,
    pub series: Vec<SeriesRequest>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesRequest {
    pub key: &'static str,
    pub dialect: &'static str,
    pub kind: &'static str,
    pub series_id: String,
    pub data_types: Vec<&'static str>,
    pub indicators: Vec<()>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HistoryResponse {
    pub time_info: TimeInfo,
    #[serde(default)]
    pub series: Vec<SeriesResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimeInfo {
    /// Epoch milliseconds, one per slot. This is the union of the trading
    /// sessions of every series in the request, which is why an individual
    /// series can have nulls in it.
    #[serde(default)]
    pub ticks: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesResponse {
    pub series_id: String,
    /// One slot per tick. A slot is `[value]`, or `[null]` when this series
    /// did not trade in that session.
    #[serde(default)]
    pub data_points: Vec<Vec<Option<f64>>>,
}

/// One instrument's price series: the epoch-millisecond timestamp and the
/// close, with the empty slots dropped.
pub type Series = Vec<(i64, f64)>;

impl HistoryResponse {
    /// Pulls out one series by the id it was requested under, pairing values
    /// with their timestamps and discarding slots this instrument did not
    /// trade in.
    ///
    /// Returns `None` when the response carried no such series, so a missing
    /// series is reported rather than silently becoming an empty chart.
    pub fn series(&self, series_id: &str) -> Option<Series> {
        let series = self.series.iter().find(|s| s.series_id == series_id)?;
        Some(
            series
                .data_points
                .iter()
                .enumerate()
                .filter_map(|(n, slot)| {
                    let value = (*slot.first()?)?;
                    Some((*self.time_info.ticks.get(n)?, value))
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(json: &str) -> RawQuote {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_quote_parses_its_comma_formatted_price_into_a_number() {
        let q = raw(r#"{"symbol":".SPX","code":0,"last":"7,738.79",
                        "previous_day_closing":"7,747.71"}"#)
        .parse()
        .unwrap();
        assert_eq!(q.last, 7738.79);
    }

    #[test]
    fn a_bond_quote_strips_the_percent_sign_from_its_yield() {
        let q = raw(r#"{"symbol":"US10Y","code":0,"last":"4.776%",
                        "previous_day_closing":"4.762%"}"#)
        .parse()
        .unwrap();
        assert_eq!(q.last, 4.776);
    }

    /// The endpoint answers an unknown symbol with a populated object rather
    /// than an error, so the row has to be recognised and dropped here.
    #[test]
    fn a_quote_with_code_one_yields_nothing_rather_than_failing_the_batch() {
        assert!(raw(r#"{"symbol":".FTSEMIB","code":1}"#).parse().is_none());
    }

    /// The real payload for the 2-year on a day its yield rose: the reported
    /// percent change is negative because it describes the bond's price, not
    /// its yield. Trusting it would paint a rising yield red.
    #[test]
    fn change_is_recomputed_because_the_reported_percent_contradicts_the_prices() {
        let q = raw(r#"{"symbol":"US2Y","code":0,"last":"4.379%",
                        "previous_day_closing":"4.334%",
                        "change":"+0.045","change_pct":"-0.0859%"}"#)
        .parse()
        .unwrap();
        assert!(q.change > 0.0, "yield rose, so the change must be positive");
        assert!(
            q.change_pct > 0.0,
            "yield rose, so the percent change must be positive, got {}",
            q.change_pct
        );
        assert!((q.change_pct - 1.0383).abs() < 1e-3);
    }

    #[test]
    fn a_quote_without_a_previous_close_falls_back_to_the_reported_change() {
        let q = raw(r#"{"symbol":".SPX","code":0,"last":"7,738.79",
                        "change":"-8.92","change_pct":"-0.12%"}"#)
        .parse()
        .unwrap();
        assert_eq!(q.change, -8.92);
        assert_eq!(q.change_pct, -0.12);
    }

    #[test]
    fn a_quote_with_no_price_at_all_yields_nothing() {
        assert!(raw(r#"{"symbol":".SPX","code":0}"#).parse().is_none());
        assert!(raw(r#"{"symbol":".SPX","code":0,"last":"UNCH"}"#)
            .parse()
            .is_none());
    }

    #[test]
    fn the_optional_detail_fields_are_carried_through() {
        let q = raw(r#"{"symbol":".SPX","code":0,"last":"7,738.79",
                        "previous_day_closing":"7,747.71","open":"7,750.19",
                        "high":"7,750.19","low":"7,733.93",
                        "yrhiprice":"7,816.70","yrloprice":"6,316.91",
                        "curmktstatus":"REG_MKT"}"#)
        .parse()
        .unwrap();
        assert_eq!(q.open, Some(7750.19));
        assert_eq!(q.year_low, Some(6316.91));
        assert_eq!(q.market_status.as_deref(), Some("REG_MKT"));
    }

    fn history() -> HistoryResponse {
        // Three sessions; the second series did not trade in the middle one,
        // which is what a batched response actually looks like.
        serde_json::from_str(
            r#"{"TimeInfo":{"Ticks":[1000,2000,3000]},
                "Series":[
                  {"SeriesId":"s0","DataPoints":[[7700.0],[7720.5],[7738.79]]},
                  {"SeriesId":"s1","DataPoints":[[1.16],[null],[1.1617]]}
                ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn history_pairs_ticks_with_values() {
        assert_eq!(
            history().series("s0").unwrap(),
            vec![(1000, 7700.0), (2000, 7720.5), (3000, 7738.79)]
        );
    }

    /// Ticks are the union across every series in the batch, so a series that
    /// did not trade in a session must lose that slot, not shift into it.
    #[test]
    fn history_drops_the_null_slots_without_shifting_the_timestamps() {
        assert_eq!(
            history().series("s1").unwrap(),
            vec![(1000, 1.16), (3000, 1.1617)]
        );
    }

    #[test]
    fn a_series_missing_from_the_response_is_reported_not_silently_empty() {
        assert!(history().series("s9").is_none());
    }

    #[test]
    fn the_request_body_uses_the_field_names_the_endpoint_expects() {
        let body = serde_json::to_string(&HistoryRequest {
            step: "P1D",
            time_frame: "P1M",
            entitlement_token: "token",
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
            series: vec![SeriesRequest {
                key: "INDEX/US//VIX",
                dialect: "Charting",
                kind: "Ticker",
                series_id: "s0".into(),
                data_types: vec!["Last"],
                indicators: vec![],
            }],
        })
        .unwrap();
        assert!(body.contains(r#""TimeFrame":"P1M""#));
        assert!(body.contains(r#""EntitlementToken":"token""#));
        assert!(body.contains(r#""ShowATH":false"#));
        assert!(body.contains(r#""ResetTodaysAfterHoursPercentChange":false"#));
        assert!(body.contains(r#""SeriesId":"s0""#));
    }
}
