//! What the mega-cap cohort looks like taken as a group.
//!
//! Everything here is descriptive. Nothing ranks a name as cheap or dear, and
//! nothing suggests an action: the questions are "where does this sit in its
//! own year" and "is the group's strength broad or concentrated". Every
//! measure is computed from one quote response, so the whole tab costs no
//! extra request.

use crate::api::models::Quote;

/// Where a price sits between its 52-week low and high, as a percent.
///
/// 0 is the low, 100 the high. `None` when the endpoint sent no range, or
/// when the range has collapsed to a point and the position is undefined
/// rather than arbitrarily 0 or 100.
///
/// The band can be stale by a day at the edges: the endpoint's 52-week high
/// updates on its own schedule, so a name printing a new high can briefly
/// read slightly above 100. It is clamped rather than hidden, because a bar
/// that overshoots its track is a worse lie than one that pins.
pub fn range_position(quote: &Quote) -> Option<f64> {
    let (lo, hi) = (quote.year_low?, quote.year_high?);
    let span = hi - lo;
    if span <= 0.0 {
        return None;
    }
    Some((((quote.last - lo) / span) * 100.0).clamp(0.0, 100.0))
}

/// How far below the 52-week high the last price is, as a negative percent.
///
/// Zero at the high. This is drawdown from the *52-week* high, not the all
/// time high: the quote endpoint carries the one-year figure, and a longer
/// lookback would need a second request per name.
pub fn drawdown(quote: &Quote) -> Option<f64> {
    let hi = quote.year_high?;
    if hi <= 0.0 {
        return None;
    }
    Some((quote.last / hi - 1.0) * 100.0)
}

/// Above this share of the 52-week band a name counts as near its high.
pub const NEAR_HIGH: f64 = 80.0;

/// The cohort read as one line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Breadth {
    /// Today's move weighted by market capitalisation, in percent.
    pub cap_weighted: f64,
    /// Today's move with every name counted once, in percent.
    pub equal_weighted: f64,
    /// Names that rose, and names priced at all. Unchanged counts as neither.
    pub advancing: usize,
    pub declining: usize,
    pub priced: usize,
    /// Median position in the 52-week band across priced names.
    pub median_position: Option<f64>,
    /// How many sit above `NEAR_HIGH` in their own band.
    pub near_high: usize,
    /// Share of the cohort's capitalisation held by its three largest names,
    /// in percent. The concentration the other numbers are a symptom of.
    pub top_three_share: Option<f64>,
}

impl Breadth {
    /// Cap-weighted less equal-weighted, in percentage points.
    ///
    /// Positive means the move was carried by the biggest names and the
    /// average name did less; negative means the opposite. This is the
    /// difference the tab exists to show, and it is the one number here that
    /// says something neither list says alone.
    pub fn divergence(&self) -> f64 {
        self.cap_weighted - self.equal_weighted
    }

    /// A word for the divergence, or `None` when it is too small to mean
    /// anything.
    ///
    /// The threshold is deliberately not zero. Two weightings of the same
    /// fifteen names differ by a few basis points on almost any day, and
    /// labelling that noise "narrow" would make the word worthless.
    pub fn shape(&self) -> Option<&'static str> {
        const MEANINGFUL: f64 = 0.15;
        let d = self.divergence();
        if d.abs() < MEANINGFUL {
            None
        } else if d > 0.0 {
            Some("narrow")
        } else {
            Some("broad")
        }
    }
}

/// Folds the cohort's quotes into one reading.
///
/// Returns `None` when nothing is priced yet, so the caller shows the tab
/// loading rather than a row of zeroes that look like a flat session.
///
/// Weighting uses each name's capitalisation *before* today's move, recovered
/// from the reported cap and the percent change. Weighting by the current cap
/// would let a name's own gain inflate the weight that gain is counted at,
/// which biases the cap-weighted figure upward on an up day and is exactly
/// the error that would make the divergence look larger than it is.
pub fn breadth(quotes: &[Option<Quote>]) -> Option<Breadth> {
    let priced: Vec<&Quote> = quotes.iter().flatten().collect();
    if priced.is_empty() {
        return None;
    }

    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    let mut caps: Vec<f64> = Vec::with_capacity(priced.len());
    for q in &priced {
        let Some(cap) = q.market_cap else { continue };
        caps.push(cap);
        let opening_cap = cap / (1.0 + q.change_pct / 100.0);
        if opening_cap.is_finite() && opening_cap > 0.0 {
            weighted_sum += opening_cap * q.change_pct;
            weight_total += opening_cap;
        }
    }

    let equal_weighted = priced.iter().map(|q| q.change_pct).sum::<f64>() / priced.len() as f64;
    let cap_weighted = if weight_total > 0.0 {
        weighted_sum / weight_total
    } else {
        equal_weighted
    };

    let mut positions: Vec<f64> = priced.iter().filter_map(|q| range_position(q)).collect();
    positions.sort_by(f64::total_cmp);

    Some(Breadth {
        cap_weighted,
        equal_weighted,
        advancing: priced.iter().filter(|q| q.change_pct > 0.0).count(),
        declining: priced.iter().filter(|q| q.change_pct < 0.0).count(),
        priced: priced.len(),
        median_position: median(&positions),
        near_high: positions.iter().filter(|p| **p > NEAR_HIGH).count(),
        top_three_share: top_three_share(&mut caps),
    })
}

/// The median of an already-sorted slice, averaging the middle pair when the
/// count is even.
fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

fn top_three_share(caps: &mut [f64]) -> Option<f64> {
    if caps.len() < 3 {
        return None;
    }
    let total: f64 = caps.iter().sum();
    if total <= 0.0 {
        return None;
    }
    caps.sort_by(|a, b| b.total_cmp(a));
    Some(caps[..3].iter().sum::<f64>() / total * 100.0)
}

/// Cells in the bar that shows where a price sits in its 52-week band.
///
/// The bar fills to the position, so the reading survives without colour —
/// red against green is the pair the most common form of colour blindness
/// loses, which is why the band is shape and the percent is printed beside
/// it.
pub const BAND_CELLS: usize = 8;

/// The last cell the bar fills for a position, from 0 to `BAND_CELLS - 1`.
///
/// A price exactly at the 52-week high would otherwise index one past the
/// end, so the top of the range shares the last cell with everything just
/// below it.
pub fn band_index(position: f64) -> usize {
    let cell = (position / 100.0 * BAND_CELLS as f64) as usize;
    cell.min(BAND_CELLS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(last: f64, change_pct: f64, lo: f64, hi: f64, cap: f64) -> Option<Quote> {
        Some(Quote {
            last,
            change: 0.0,
            change_pct,
            open: None,
            high: None,
            low: None,
            prev_close: None,
            year_high: Some(hi),
            year_low: Some(lo),
            market_status: None,
            market_cap: Some(cap),
            beta: None,
            vol_ratio: None,
        })
    }

    #[test]
    fn position_is_the_share_of_the_band_below_the_price() {
        let q = quote(150.0, 0.0, 100.0, 200.0, 1e12).unwrap();
        assert_eq!(range_position(&q), Some(50.0));
    }

    #[test]
    fn position_pins_at_the_ends_rather_than_overshooting() {
        // A new high the endpoint's 52-week figure has not caught up with.
        let q = quote(210.0, 0.0, 100.0, 200.0, 1e12).unwrap();
        assert_eq!(range_position(&q), Some(100.0));
        let q = quote(90.0, 0.0, 100.0, 200.0, 1e12).unwrap();
        assert_eq!(range_position(&q), Some(0.0));
    }

    #[test]
    fn a_collapsed_band_has_no_position_rather_than_a_made_up_one() {
        let q = quote(100.0, 0.0, 100.0, 100.0, 1e12).unwrap();
        assert_eq!(range_position(&q), None);
    }

    #[test]
    fn drawdown_is_zero_at_the_high_and_negative_below_it() {
        let at_high = quote(200.0, 0.0, 100.0, 200.0, 1e12).unwrap();
        assert_eq!(drawdown(&at_high), Some(0.0));
        let below = quote(150.0, 0.0, 100.0, 200.0, 1e12).unwrap();
        assert_eq!(drawdown(&below), Some(-25.0));
    }

    #[test]
    fn nothing_priced_yields_no_reading() {
        assert!(breadth(&[None, None]).is_none());
    }

    /// The case the tab exists for: a huge name up, a pile of small ones flat,
    /// so the cap-weighted move is real and the average name did nothing.
    #[test]
    fn a_move_carried_by_the_biggest_name_reads_as_narrow() {
        let quotes = vec![
            quote(110.0, 10.0, 50.0, 120.0, 9e12),
            quote(100.0, 0.0, 50.0, 120.0, 1e11),
            quote(100.0, 0.0, 50.0, 120.0, 1e11),
            quote(100.0, 0.0, 50.0, 120.0, 1e11),
        ];
        let b = breadth(&quotes).unwrap();
        assert!(b.cap_weighted > b.equal_weighted);
        assert_eq!(b.shape(), Some("narrow"));
        assert_eq!(b.advancing, 1);
        assert_eq!(b.declining, 0);
        assert_eq!(b.priced, 4);
    }

    #[test]
    fn a_move_the_giant_sat_out_reads_as_broad() {
        let quotes = vec![
            quote(100.0, 0.0, 50.0, 120.0, 9e12),
            quote(105.0, 5.0, 50.0, 120.0, 1e11),
            quote(105.0, 5.0, 50.0, 120.0, 1e11),
            quote(105.0, 5.0, 50.0, 120.0, 1e11),
        ];
        let b = breadth(&quotes).unwrap();
        assert!(b.cap_weighted < b.equal_weighted);
        assert_eq!(b.shape(), Some("broad"));
    }

    /// Two weightings of the same names differ by basis points on a quiet
    /// day. That is not a story and must not be labelled one.
    #[test]
    fn a_trivial_gap_gets_no_label() {
        let quotes = vec![
            quote(100.0, 1.00, 50.0, 120.0, 5e12),
            quote(100.0, 1.05, 50.0, 120.0, 4e12),
            quote(100.0, 0.98, 50.0, 120.0, 3e12),
        ];
        let b = breadth(&quotes).unwrap();
        assert!(b.divergence().abs() < 0.15);
        assert_eq!(b.shape(), None);
    }

    /// Weighting by the post-move cap would count the winner's gain at the
    /// weight its own gain created. With one name up 100% the error is large
    /// enough to assert on: the correct figure here is 50%, the biased one
    /// would be 66.7%.
    #[test]
    fn weights_are_taken_before_todays_move_not_after() {
        let quotes = vec![
            quote(200.0, 100.0, 10.0, 250.0, 2e12),
            quote(100.0, 0.0, 10.0, 250.0, 1e12),
        ];
        let b = breadth(&quotes).unwrap();
        assert!(
            (b.cap_weighted - 50.0).abs() < 1e-9,
            "expected 50.0, got {}",
            b.cap_weighted
        );
    }

    #[test]
    fn median_position_and_near_high_count_the_band() {
        let quotes = vec![
            quote(190.0, 0.0, 100.0, 200.0, 1e12), // 90%
            quote(150.0, 0.0, 100.0, 200.0, 1e12), // 50%
            quote(110.0, 0.0, 100.0, 200.0, 1e12), // 10%
        ];
        let b = breadth(&quotes).unwrap();
        assert_eq!(b.median_position, Some(50.0));
        assert_eq!(b.near_high, 1);
    }

    #[test]
    fn median_averages_the_middle_pair_when_the_count_is_even() {
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), Some(25.0));
        assert_eq!(median(&[10.0, 20.0, 30.0]), Some(20.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn top_three_share_needs_three_names() {
        let mut two = vec![1e12, 2e12];
        assert_eq!(top_three_share(&mut two), None);
        let mut four = vec![1e12, 1e12, 1e12, 1e12];
        assert_eq!(top_three_share(&mut four), Some(75.0));
    }

    #[test]
    fn a_name_with_no_cap_still_counts_in_the_equal_weighted_average() {
        let mut no_cap = quote(100.0, 6.0, 50.0, 120.0, 0.0).unwrap();
        no_cap.market_cap = None;
        let quotes = vec![quote(100.0, 2.0, 50.0, 120.0, 1e12), Some(no_cap)];
        let b = breadth(&quotes).unwrap();
        assert_eq!(b.priced, 2);
        assert!((b.equal_weighted - 4.0).abs() < 1e-9);
        // Only the name that reported a cap can carry the weighted figure.
        assert!((b.cap_weighted - 2.0).abs() < 1e-9);
    }

    #[test]
    fn the_band_lights_one_cell_and_never_runs_off_the_track() {
        assert_eq!(band_index(0.0), 0);
        assert_eq!(band_index(50.0), 4);
        assert_eq!(band_index(100.0), BAND_CELLS - 1);
    }
}
