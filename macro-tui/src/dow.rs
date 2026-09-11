//! The Dow Jones Industrial Average taken apart into the thirty names in it.
//!
//! Everything here is descriptive. Nothing ranks a name as cheap or dear, and
//! nothing suggests an action: the questions are "which names moved the index
//! today" and "was the move broad or the work of two or three". Every measure
//! comes out of the one quote response the board already makes, so the tab
//! costs no extra request.
//!
//! The average is price-weighted, which is the fact the whole module turns
//! on. A name's effect on the index depends on its share price and nothing
//! else: a $800 stock moves the Dow roughly eight times as far as a $100 one
//! on the same percentage move, whatever the two companies are worth. That is
//! why the interesting comparison here is the index against an equal-weighted
//! average of its own members, and why a name's contribution is worth stating
//! in index points.

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

/// The divisor that turns the sum of the thirty share prices into the index
/// level, derived from yesterday's settled closes.
///
/// The Dow is the sum of its members' prices over a divisor that absorbs
/// splits and substitutions, so the divisor is recoverable from any consistent
/// set of prices and the matching index level. Yesterday's closes are used
/// rather than today's prices because they are settled and simultaneous:
/// during a session the index ticks continuously while each member's last
/// trade is its own instant, and that skew would wobble the figure.
///
/// `None` unless every one of the thirty reported a previous close, because a
/// divisor derived from a partial sum would be wrong rather than approximate,
/// and would then quietly corrupt every contribution computed from it.
pub fn divisor(members: &[Option<Quote>], index: &Quote) -> Option<f64> {
    if members.len() != crate::catalog::DOW_30.len() {
        return None;
    }
    let mut sum = 0.0;
    for q in members {
        sum += q.as_ref()?.prev_close?;
    }
    let prev_index = index.prev_close?;
    if prev_index <= 0.0 || sum <= 0.0 {
        return None;
    }
    Some(sum / prev_index)
}

/// How far a derived divisor has drifted from the one recorded when the
/// membership was last reviewed, as a fraction.
///
/// The divisor only moves on a split or a substitution, and a substitution
/// moves it by a whole share price. So a large drift means this build's
/// membership table is out of date, and every contribution computed from it
/// is wrong by the missing member's weight. Worth telling the user, since
/// they may be running a binary older than the last reshuffle.
pub fn divisor_drift(derived: f64) -> f64 {
    (derived - crate::catalog::DIVISOR_AT_REVIEW).abs() / crate::catalog::DIVISOR_AT_REVIEW
}

/// Past this drift the membership table is treated as stale on screen.
///
/// Splits move the divisor by well under a percent; losing a member moves it
/// by several.
pub const STALE_DRIFT: f64 = 0.01;

/// What a member added to or took off the index today, in index points.
///
/// A name's price change over the divisor. These sum to the index's move in
/// points, exactly at the close and approximately during a session, where the
/// residual is the skew between each member's last trade and the index's own
/// stamp.
pub fn contribution(quote: &Quote, divisor: f64) -> Option<f64> {
    if divisor <= 0.0 {
        return None;
    }
    let prev = quote.prev_close?;
    Some((quote.last - prev) / divisor)
}

/// The session read across the whole average.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Session {
    /// The move the index itself made, in percent: the change in the sum of
    /// member prices over that sum. The divisor cancels, so this is the
    /// average's own return computed from its parts.
    pub price_weighted: f64,
    /// The same day with every member counted once, in percent.
    pub equal_weighted: f64,
    /// Members that rose, and members priced at all. Unchanged counts as
    /// neither.
    pub advancing: usize,
    pub declining: usize,
    pub priced: usize,
    /// Median position in the 52-week band across priced members.
    pub median_position: Option<f64>,
    /// How many sit above `NEAR_HIGH` in their own band.
    pub near_high: usize,
    /// Share of today's index move, in percent of the total absolute
    /// movement, made by the three members that moved it most. The
    /// concentration the divergence is a symptom of.
    pub top_three_share: Option<f64>,
}

impl Session {
    /// Price-weighted less equal-weighted, in percentage points.
    ///
    /// Positive means the index did better than its average member, so the
    /// day was carried by the higher-priced names — the ones price weighting
    /// gives the most say. Negative means the index understated a move most
    /// of its members took part in. This is the difference the tab exists to
    /// show, and it is the one number here that neither list gives alone.
    pub fn divergence(&self) -> f64 {
        self.price_weighted - self.equal_weighted
    }

    /// A word for the divergence, or `None` when it is too small to mean
    /// anything.
    ///
    /// The threshold is deliberately not zero. Two weightings of the same
    /// thirty names differ by a few basis points on almost any day, and
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

/// Folds the members' quotes into one reading of the session.
///
/// Returns `None` when nothing is priced yet, so the caller shows the tab
/// loading rather than a row of zeroes that look like a flat session.
pub fn session(quotes: &[Option<Quote>]) -> Option<Session> {
    let priced: Vec<&Quote> = quotes.iter().flatten().collect();
    if priced.is_empty() {
        return None;
    }

    // The index's own return, recovered from its parts: the divisor is a
    // constant through the day, so it cancels out of the ratio entirely and
    // never has to be known here.
    let mut sum_last = 0.0;
    let mut sum_prev = 0.0;
    for q in &priced {
        let Some(prev) = q.prev_close else { continue };
        sum_last += q.last;
        sum_prev += prev;
    }

    let equal_weighted = priced.iter().map(|q| q.change_pct).sum::<f64>() / priced.len() as f64;
    let price_weighted = if sum_prev > 0.0 {
        (sum_last - sum_prev) / sum_prev * 100.0
    } else {
        equal_weighted
    };

    let mut positions: Vec<f64> = priced.iter().filter_map(|q| range_position(q)).collect();
    positions.sort_by(f64::total_cmp);

    // Shares of the day's movement, which needs no divisor either: every
    // contribution carries the same one, so it cancels out of the ratio.
    let mut swings: Vec<f64> = priced
        .iter()
        .filter_map(|q| Some((q.last - q.prev_close?).abs()))
        .collect();

    Some(Session {
        price_weighted,
        equal_weighted,
        advancing: priced.iter().filter(|q| q.change_pct > 0.0).count(),
        declining: priced.iter().filter(|q| q.change_pct < 0.0).count(),
        priced: priced.len(),
        median_position: median(&positions),
        near_high: positions.iter().filter(|p| **p > NEAR_HIGH).count(),
        top_three_share: top_three_share(&mut swings),
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

/// The three largest values as a share of the whole, in percent.
fn top_three_share(values: &mut [f64]) -> Option<f64> {
    if values.len() < 3 {
        return None;
    }
    let total: f64 = values.iter().sum();
    if total <= 0.0 {
        return None;
    }
    values.sort_by(|a, b| b.total_cmp(a));
    Some(values[..3].iter().sum::<f64>() / total * 100.0)
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

    /// `change_pct` is derived from the two prices, the way the wire parser
    /// derives it, so a fixture cannot disagree with itself.
    fn quote(last: f64, prev: f64, lo: f64, hi: f64) -> Option<Quote> {
        Some(Quote {
            last,
            change: last - prev,
            change_pct: (last - prev) / prev * 100.0,
            open: None,
            high: None,
            low: None,
            prev_close: Some(prev),
            year_high: Some(hi),
            year_low: Some(lo),
            market_status: None,
            market_cap: None,
            beta: None,
            vol_ratio: None,
        })
    }

    fn flat(price: f64) -> Option<Quote> {
        quote(price, price, price / 2.0, price * 2.0)
    }

    /// A stand-in membership the size of the real one, so `divisor` accepts it.
    fn thirty(mut members: Vec<Option<Quote>>) -> Vec<Option<Quote>> {
        while members.len() < crate::catalog::DOW_30.len() {
            members.push(flat(100.0));
        }
        members
    }

    #[test]
    fn position_is_the_share_of_the_band_below_the_price() {
        let q = quote(150.0, 150.0, 100.0, 200.0).unwrap();
        assert_eq!(range_position(&q), Some(50.0));
    }

    #[test]
    fn position_pins_at_the_ends_rather_than_overshooting() {
        // A new high the endpoint's 52-week figure has not caught up with.
        let q = quote(210.0, 210.0, 100.0, 200.0).unwrap();
        assert_eq!(range_position(&q), Some(100.0));
        let q = quote(90.0, 90.0, 100.0, 200.0).unwrap();
        assert_eq!(range_position(&q), Some(0.0));
    }

    #[test]
    fn a_collapsed_band_has_no_position_rather_than_a_made_up_one() {
        let q = quote(100.0, 100.0, 100.0, 100.0).unwrap();
        assert_eq!(range_position(&q), None);
    }

    #[test]
    fn drawdown_is_zero_at_the_high_and_negative_below_it() {
        assert_eq!(
            drawdown(&quote(200.0, 200.0, 100.0, 200.0).unwrap()),
            Some(0.0)
        );
        assert_eq!(
            drawdown(&quote(150.0, 150.0, 100.0, 200.0).unwrap()),
            Some(-25.0)
        );
    }

    // --- the divisor -----------------------------------------------------

    /// The real relationship, with the real shape of the numbers: thirty
    /// closes summing to 8,566 against an index near 52,600 gives the
    /// published divisor of about 0.1628.
    #[test]
    fn the_divisor_is_the_sum_of_closes_over_the_index_level() {
        let members = thirty(vec![]);
        let sum = 100.0 * crate::catalog::DOW_30.len() as f64;
        let index = quote(20_000.0, sum / 0.25, 1.0, 1e9).unwrap();
        assert_eq!(divisor(&members, &index), Some(0.25));
    }

    /// A divisor from a partial sum would not be approximate, it would be
    /// wrong, and every contribution derived from it would inherit the error.
    #[test]
    fn one_unpriced_member_yields_no_divisor_at_all() {
        let mut members = thirty(vec![]);
        members[7] = None;
        let index = quote(20_000.0, 20_000.0, 1.0, 1e9).unwrap();
        assert_eq!(divisor(&members, &index), None);
    }

    #[test]
    fn a_short_member_list_yields_no_divisor() {
        let index = quote(20_000.0, 20_000.0, 1.0, 1e9).unwrap();
        assert_eq!(divisor(&[flat(100.0)], &index), None);
    }

    #[test]
    fn a_contribution_is_the_price_change_over_the_divisor() {
        // Ten dollars on a 0.25 divisor is forty index points.
        let q = quote(110.0, 100.0, 50.0, 200.0).unwrap();
        assert_eq!(contribution(&q, 0.25), Some(40.0));
        let faller = quote(90.0, 100.0, 50.0, 200.0).unwrap();
        assert_eq!(contribution(&faller, 0.25), Some(-40.0));
    }

    /// The property that makes the column trustworthy: the parts add up to
    /// the whole.
    #[test]
    fn contributions_sum_to_the_index_move_in_points() {
        let members = [
            quote(110.0, 100.0, 50.0, 200.0),
            quote(95.0, 100.0, 50.0, 200.0),
            quote(103.0, 100.0, 50.0, 200.0),
        ];
        let div = 0.25;
        let points: f64 = members
            .iter()
            .flatten()
            .filter_map(|q| contribution(q, div))
            .sum();
        let prices_before = 300.0;
        let prices_after = 110.0 + 95.0 + 103.0;
        assert!((points - (prices_after - prices_before) / div).abs() < 1e-9);
    }

    // --- the session -----------------------------------------------------

    #[test]
    fn nothing_priced_yields_no_reading() {
        assert!(session(&[None, None]).is_none());
    }

    /// The case the tab exists for. One expensive name up hard and a pile of
    /// cheap ones flat: price weighting hands the index to the expensive one,
    /// and the average member did nothing.
    #[test]
    fn a_move_carried_by_an_expensive_name_reads_as_narrow() {
        let members = vec![
            quote(880.0, 800.0, 400.0, 900.0),
            flat(50.0),
            flat(50.0),
            flat(50.0),
        ];
        let s = session(&members).unwrap();
        assert!(s.price_weighted > s.equal_weighted);
        assert_eq!(s.shape(), Some("narrow"));
        assert_eq!(s.advancing, 1);
        assert_eq!(s.declining, 0);
        assert_eq!(s.priced, 4);
    }

    /// The mirror image, and the one price weighting hides: every cheap name
    /// up five percent while the expensive one sits still. The index barely
    /// moves and the day was actually broad.
    #[test]
    fn a_move_the_expensive_name_sat_out_reads_as_broad() {
        let members = vec![
            flat(800.0),
            quote(52.5, 50.0, 20.0, 90.0),
            quote(52.5, 50.0, 20.0, 90.0),
            quote(52.5, 50.0, 20.0, 90.0),
        ];
        let s = session(&members).unwrap();
        assert!(s.price_weighted < s.equal_weighted);
        assert_eq!(s.shape(), Some("broad"));
        assert_eq!(s.advancing, 3);
    }

    /// Price weighting is about price, not size. Two companies worth the same
    /// move the index by different amounts, and that is the point.
    #[test]
    fn weighting_follows_share_price_and_ignores_market_value() {
        let mut cheap = quote(50.0, 50.0, 10.0, 90.0).unwrap();
        let mut dear = quote(500.0, 500.0, 100.0, 900.0).unwrap();
        // Same company size, tenfold different share price.
        cheap.market_cap = Some(1e12);
        dear.market_cap = Some(1e12);
        assert_eq!(contribution(&cheap, 0.2), Some(0.0));
        // A one percent move on each: the dear one moves the index ten times
        // as far.
        let cheap_up = quote(50.5, 50.0, 10.0, 90.0).unwrap();
        let dear_up = quote(505.0, 500.0, 100.0, 900.0).unwrap();
        let (a, b) = (
            contribution(&cheap_up, 0.2).unwrap(),
            contribution(&dear_up, 0.2).unwrap(),
        );
        assert!((b / a - 10.0).abs() < 1e-9, "{b} should be ten times {a}");
    }

    /// Two weightings of the same names differ by basis points on a quiet
    /// day. That is not a story and must not be labelled one.
    #[test]
    fn a_trivial_gap_gets_no_label() {
        let members = vec![
            quote(101.00, 100.0, 50.0, 150.0),
            quote(101.05, 100.0, 50.0, 150.0),
            quote(100.98, 100.0, 50.0, 150.0),
        ];
        let s = session(&members).unwrap();
        assert!(s.divergence().abs() < 0.15);
        assert_eq!(s.shape(), None);
    }

    #[test]
    fn median_position_and_near_high_count_the_band() {
        let members = vec![
            quote(190.0, 190.0, 100.0, 200.0), // 90%
            quote(150.0, 150.0, 100.0, 200.0), // 50%
            quote(110.0, 110.0, 100.0, 200.0), // 10%
        ];
        let s = session(&members).unwrap();
        assert_eq!(s.median_position, Some(50.0));
        assert_eq!(s.near_high, 1);
    }

    /// Concentration is measured on the movement, not on the prices: three
    /// names doing all the moving is the finding, whatever they cost.
    #[test]
    fn top_three_share_measures_the_days_movement() {
        let members = vec![
            quote(110.0, 100.0, 50.0, 200.0),
            quote(110.0, 100.0, 50.0, 200.0),
            quote(110.0, 100.0, 50.0, 200.0),
            flat(100.0),
            flat(100.0),
        ];
        let s = session(&members).unwrap();
        assert_eq!(s.top_three_share, Some(100.0));
    }

    #[test]
    fn median_averages_the_middle_pair_when_the_count_is_even() {
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), Some(25.0));
        assert_eq!(median(&[10.0, 20.0, 30.0]), Some(20.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn top_three_share_needs_three_values() {
        assert_eq!(top_three_share(&mut [1.0, 2.0]), None);
        assert_eq!(top_three_share(&mut [1.0, 1.0, 1.0, 1.0]), Some(75.0));
    }

    #[test]
    fn drift_is_zero_at_the_reviewed_divisor_and_grows_either_side() {
        let at_review = crate::catalog::DIVISOR_AT_REVIEW;
        assert!(divisor_drift(at_review).abs() < 1e-12);
        // A member gone missing shifts the sum by a whole share price, which
        // is percent-scale movement, not a split's fraction of one.
        assert!(divisor_drift(at_review * 1.03) > STALE_DRIFT);
        assert!(divisor_drift(at_review * 0.97) > STALE_DRIFT);
        // A split is comfortably inside the threshold.
        assert!(divisor_drift(at_review * 1.002) < STALE_DRIFT);
    }

    #[test]
    fn the_band_lights_one_cell_and_never_runs_off_the_track() {
        assert_eq!(band_index(0.0), 0);
        assert_eq!(band_index(50.0), 4);
        assert_eq!(band_index(100.0), BAND_CELLS - 1);
    }
}
