//! The static table of everything the board shows.
//!
//! Each row pins a display name to a CNBC quote symbol, a MarketWatch history
//! key, a group, and a formatting rule. Every symbol and every key here was
//! probed against the live endpoints.
//!
//! Finding a history key when one rots: MarketWatch's own quote page URL
//! carries the country code (`/investing/index/dax?countrycode=dx`), and the
//! key is `INDEX/<countrycode>//<TICKER>` with the exchange segment left
//! empty, or `INDEX/<countrycode>/<MIC>/<TICKER>` with it. A bad key answers
//! in about 200 ms, so a small grid over those two shapes finds it quickly.

/// How a value is rendered.
///
/// The feed hands back preformatted strings, but the app reparses them into
/// floats so it can align columns and draw charts, so it has to know how to
/// print them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    /// Thousands separators: 7,738.79
    Grouped,
    /// No separators, for values that never reach four digits: 1.1617
    Plain,
    /// A bond yield. Renders 4.768%, and moves in basis points.
    Yield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    UsEquity,
    Rates,
    Commodities,
    FxCrypto,
    World,
}

impl Group {
    pub const ALL: [Group; 5] = [
        Group::UsEquity,
        Group::Rates,
        Group::Commodities,
        Group::FxCrypto,
        Group::World,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::UsEquity => "US equity & vol",
            Group::Rates => "Rates & dollar",
            Group::Commodities => "Commodities",
            Group::FxCrypto => "FX & crypto",
            Group::World => "World indices",
        }
    }
}

pub struct Instrument {
    /// Our own display name. CNBC's `shortName` is unusable for a narrow
    /// column: the dollar index comes back as "ICE US Dollar Index".
    pub name: &'static str,
    /// Symbol for the CNBC quote endpoint.
    pub cnbc: &'static str,
    /// MarketWatch "Charting" series key. `Option` so a row can exist without
    /// a chart; every row today has one.
    pub history: Option<&'static str>,
    pub group: Group,
    pub fmt: Fmt,
    /// Decimal places. Per instrument rather than per format, because market
    /// convention differs inside a format: EUR/USD is quoted to four places
    /// and USD/JPY to two.
    pub decimals: u8,
    /// Lowercase terms matched against headline text on word boundaries.
    /// The display name is always matched and need not be repeated.
    pub aliases: &'static [&'static str],
}

pub const INSTRUMENTS: &[Instrument] = &[
    // --- US equity and volatility ----------------------------------------
    Instrument {
        name: "S&P 500",
        cnbc: ".SPX",
        history: Some("INDEX/US/S&P US/SPX"),
        group: Group::UsEquity,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["s&p", "sp500", "spx"],
    },
    Instrument {
        name: "Nasdaq 100",
        cnbc: ".NDX",
        history: Some("INDEX/US/XNAS/NDX"),
        group: Group::UsEquity,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["nasdaq", "ndx"],
    },
    Instrument {
        name: "Dow Jones",
        cnbc: ".DJI",
        history: Some("INDEX/US/DOW JONES GLOBAL/DJIA"),
        group: Group::UsEquity,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["dow", "djia"],
    },
    Instrument {
        name: "Russell 2000",
        cnbc: ".RUT",
        history: Some("INDEX/US/FTSE RUSSELL/RUT"),
        group: Group::UsEquity,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["russell", "small cap", "small-cap"],
    },
    // The empty exchange segment is deliberate; no populated variant resolves.
    Instrument {
        name: "VIX",
        cnbc: ".VIX",
        history: Some("INDEX/US//VIX"),
        group: Group::UsEquity,
        fmt: Fmt::Plain,
        decimals: 2,
        aliases: &["vix", "volatility index", "fear gauge"],
    },
    // --- Rates and dollar ------------------------------------------------
    Instrument {
        name: "US 2-year",
        cnbc: "US2Y",
        history: Some("BOND/BX//TMUBMUSD02Y"),
        group: Group::Rates,
        fmt: Fmt::Yield,
        decimals: 3,
        aliases: &["2-year", "two-year", "2 year treasury", "short end"],
    },
    Instrument {
        name: "US 10-year",
        cnbc: "US10Y",
        history: Some("BOND/BX//TMUBMUSD10Y"),
        group: Group::Rates,
        fmt: Fmt::Yield,
        decimals: 3,
        aliases: &["10-year", "ten-year", "10 year treasury", "benchmark yield"],
    },
    Instrument {
        name: "US 30-year",
        cnbc: "US30Y",
        history: Some("BOND/BX//TMUBMUSD30Y"),
        group: Group::Rates,
        fmt: Fmt::Yield,
        decimals: 3,
        aliases: &["30-year", "long bond"],
    },
    Instrument {
        name: "Dollar index",
        cnbc: ".DXY",
        history: Some("INDEX/US/IFUS/DXY"),
        group: Group::Rates,
        fmt: Fmt::Plain,
        decimals: 3,
        aliases: &["dollar index", "dxy", "greenback"],
    },
    // --- Commodities -----------------------------------------------------
    Instrument {
        name: "WTI crude",
        cnbc: "@CL.1",
        history: Some("FUTURE/US/XNYM/CL00"),
        group: Group::Commodities,
        fmt: Fmt::Plain,
        decimals: 2,
        aliases: &["wti", "crude", "oil prices"],
    },
    Instrument {
        name: "Brent crude",
        cnbc: "@BZ.1",
        history: Some("FUTURE/UK/IFEU/BRN00"),
        group: Group::Commodities,
        fmt: Fmt::Plain,
        decimals: 2,
        aliases: &["brent"],
    },
    Instrument {
        name: "Gold",
        cnbc: "@GC.1",
        history: Some("FUTURE/US/XNYM/GC00"),
        group: Group::Commodities,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["gold", "bullion"],
    },
    Instrument {
        name: "Silver",
        cnbc: "@SI.1",
        history: Some("FUTURE/US/XNYM/SI00"),
        group: Group::Commodities,
        fmt: Fmt::Plain,
        decimals: 2,
        aliases: &["silver"],
    },
    Instrument {
        name: "Copper",
        cnbc: "@HG.1",
        history: Some("FUTURE/US/XNYM/HG00"),
        group: Group::Commodities,
        fmt: Fmt::Plain,
        decimals: 4,
        aliases: &["copper"],
    },
    Instrument {
        name: "Natural gas",
        cnbc: "@NG.1",
        history: Some("FUTURE/US/XNYM/NG00"),
        group: Group::Commodities,
        fmt: Fmt::Plain,
        decimals: 3,
        aliases: &["natural gas", "nat gas", "henry hub"],
    },
    // --- FX and crypto ---------------------------------------------------
    Instrument {
        name: "EUR/USD",
        cnbc: "EUR=",
        history: Some("CURRENCY/US/XTUP/EURUSD"),
        group: Group::FxCrypto,
        fmt: Fmt::Plain,
        decimals: 4,
        aliases: &["euro", "eurusd", "eur/usd"],
    },
    // Two decimals, not four: the yen is quoted near 155, not near 1.
    Instrument {
        name: "USD/JPY",
        cnbc: "JPY=",
        history: Some("CURRENCY/US/XTUP/USDJPY"),
        group: Group::FxCrypto,
        fmt: Fmt::Plain,
        decimals: 2,
        aliases: &["yen", "usdjpy", "usd/jpy"],
    },
    Instrument {
        name: "GBP/USD",
        cnbc: "GBP=",
        history: Some("CURRENCY/US/XTUP/GBPUSD"),
        group: Group::FxCrypto,
        fmt: Fmt::Plain,
        decimals: 4,
        aliases: &["sterling", "pound", "gbpusd", "gbp/usd", "cable"],
    },
    Instrument {
        name: "Bitcoin",
        cnbc: "BTC.CM=",
        history: Some("CRYPTOCURRENCY/US/CoinDesk/BTCUSD"),
        group: Group::FxCrypto,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["bitcoin", "btc"],
    },
    Instrument {
        name: "Ethereum",
        cnbc: "ETH.CM=",
        history: Some("CRYPTOCURRENCY/US/Kraken/ETHUSD"),
        group: Group::FxCrypto,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["ethereum", "ether", "eth"],
    },
    // --- World -----------------------------------------------------------
    // Country code DX, not DE: that is MarketWatch's own code for Deutsche
    // Boerse, and it is what its quote page URL carries.
    Instrument {
        name: "DAX",
        cnbc: ".GDAXI",
        history: Some("INDEX/DX//DAX"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["dax", "german stocks"],
    },
    Instrument {
        name: "FTSE 100",
        cnbc: ".FTSE",
        history: Some("INDEX/UK/FTSE UK/UKX"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["ftse 100", "uk stocks"],
    },
    Instrument {
        name: "CAC 40",
        cnbc: ".FCHI",
        history: Some("INDEX/FR/XPAR/PX1"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["cac 40", "french stocks"],
    },
    Instrument {
        name: "Nikkei 225",
        cnbc: ".N225",
        history: Some("INDEX/JP/XTKS/NIK"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["nikkei", "japanese stocks"],
    },
    Instrument {
        name: "S&P/TSX",
        cnbc: ".GSPTSE",
        history: Some("INDEX/CA/XTSE/GSPTSE"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["s&p/tsx", "tsx", "canadian stocks"],
    },
    // Note the asymmetry: the quote endpoint rejects `.FTSEMIB` and wants
    // `.FTMIB`, while the history key ends in the longer spelling.
    Instrument {
        name: "FTSE MIB",
        cnbc: ".FTMIB",
        history: Some("INDEX/IT/XMIL/FTSEMIB"),
        group: Group::World,
        fmt: Fmt::Grouped,
        decimals: 2,
        aliases: &["ftse mib", "italian stocks"],
    },
];

/// The `symbols` query parameter for a single batched quote request.
pub fn all_symbols() -> String {
    INSTRUMENTS
        .iter()
        .map(|i| i.cnbc)
        .collect::<Vec<_>>()
        .join("|")
}

impl Instrument {
    /// The value itself: "7,738.79", "4.768%", "1.1617".
    pub fn level(&self, value: f64) -> String {
        let body = format!("{:.*}", self.decimals as usize, value);
        match self.fmt {
            Fmt::Grouped => group_thousands(&body),
            Fmt::Plain => body,
            Fmt::Yield => format!("{body}%"),
        }
    }

    /// A move, always explicitly signed.
    ///
    /// Yields move in basis points, because "+4.3 bp" is how a rate move is
    /// read and because it cannot be confused with the percent change in the
    /// next column.
    pub fn change(&self, delta: f64) -> String {
        let sign = if delta < 0.0 { '-' } else { '+' };
        match self.fmt {
            Fmt::Yield => format!("{sign}{:.1} bp", (delta * 100.0).abs()),
            Fmt::Grouped => format!(
                "{sign}{}",
                group_thousands(&format!("{:.*}", self.decimals as usize, delta.abs()))
            ),
            Fmt::Plain => format!("{sign}{:.*}", self.decimals as usize, delta.abs()),
        }
    }
}

/// The percent change, which reads the same for every instrument.
pub fn format_percent(pct: f64) -> String {
    let sign = if pct < 0.0 { '-' } else { '+' };
    format!("{sign}{:.2}%", pct.abs())
}

/// Inserts thousands separators into an already-formatted decimal string.
fn group_thousands(s: &str) -> String {
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let (sign, digits) = match int.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", int),
    };

    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (n, c) in digits.chars().enumerate() {
        // A separator goes before every digit whose distance from the end is
        // a multiple of three, except at the very start.
        if n > 0 && (digits.len() - n) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }

    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn find(name: &str) -> &'static Instrument {
        INSTRUMENTS.iter().find(|i| i.name == name).unwrap()
    }

    #[test]
    fn every_instrument_has_a_unique_cnbc_symbol() {
        let mut seen = HashSet::new();
        for i in INSTRUMENTS {
            assert!(seen.insert(i.cnbc), "duplicate CNBC symbol {}", i.cnbc);
        }
    }

    #[test]
    fn every_instrument_has_a_unique_name() {
        let mut seen = HashSet::new();
        for i in INSTRUMENTS {
            assert!(seen.insert(i.name), "duplicate name {}", i.name);
        }
    }

    /// A history key has four slash-separated segments; the exchange one is
    /// sometimes empty, which is exactly why this checks the count and not
    /// that every segment is non-empty.
    #[test]
    fn every_history_key_has_four_segments() {
        for i in INSTRUMENTS {
            let Some(key) = i.history else { continue };
            assert_eq!(
                key.split('/').count(),
                4,
                "{} has a malformed history key: {key}",
                i.name
            );
        }
    }

    /// Two instruments sharing an alias would silently pull each other's
    /// headlines, which looks like a matching bug rather than a data one.
    #[test]
    fn no_alias_string_is_claimed_by_two_instruments() {
        let mut seen: HashSet<&str> = HashSet::new();
        for i in INSTRUMENTS {
            for a in i.aliases {
                assert!(seen.insert(a), "alias {a:?} is claimed twice");
            }
        }
    }

    #[test]
    fn every_alias_is_lowercase_because_matching_is_done_lowercased() {
        for i in INSTRUMENTS {
            for a in i.aliases {
                assert_eq!(*a, a.to_lowercase(), "alias {a:?} on {} is not", i.name);
            }
        }
    }

    #[test]
    fn every_group_has_at_least_one_instrument() {
        for g in Group::ALL {
            assert!(
                INSTRUMENTS.iter().any(|i| i.group == g),
                "group {} is empty",
                g.as_str()
            );
        }
    }

    /// The board draws groups in `Group::ALL` order and walks the catalog
    /// once, so the catalog has to be stored that way or rows would appear
    /// under the wrong heading.
    #[test]
    fn instruments_are_stored_grouped_in_display_order() {
        let positions: Vec<usize> = INSTRUMENTS
            .iter()
            .map(|i| Group::ALL.iter().position(|g| *g == i.group).unwrap())
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] <= w[1]),
            "catalog is not grouped in Group::ALL order"
        );
    }

    #[test]
    fn all_symbols_joins_every_instrument_with_a_pipe() {
        let joined = all_symbols();
        assert_eq!(joined.matches('|').count(), INSTRUMENTS.len() - 1);
        assert!(joined.contains(".SPX"));
    }

    /// `.FTSEMIB` is what every other vendor calls it, and it is what the
    /// history key ends with, but the quote endpoint rejects it.
    #[test]
    fn italy_uses_the_symbol_the_quote_endpoint_accepts() {
        let italy = find("FTSE MIB");
        assert_eq!(italy.cnbc, ".FTMIB");
        assert_eq!(italy.history, Some("INDEX/IT/XMIL/FTSEMIB"));
    }

    #[test]
    fn thousands_separators_land_in_the_right_places() {
        assert_eq!(group_thousands("7738.79"), "7,738.79");
        assert_eq!(group_thousands("999.00"), "999.00");
        assert_eq!(group_thousands("1000.00"), "1,000.00");
        assert_eq!(group_thousands("29601.20"), "29,601.20");
        assert_eq!(group_thousands("-1234567.89"), "-1,234,567.89");
        assert_eq!(group_thousands("79562"), "79,562");
    }

    #[test]
    fn an_index_level_gets_separators_and_two_decimals() {
        assert_eq!(find("S&P 500").level(7738.79), "7,738.79");
    }

    #[test]
    fn a_yield_renders_with_a_trailing_percent_sign() {
        assert_eq!(find("US 10-year").level(4.768), "4.768%");
    }

    /// A 4.3 bp move must not read as "+0.043", which is ambiguous next to a
    /// percent column, nor as "+1.04%", which is the price change.
    #[test]
    fn a_yield_move_is_expressed_in_basis_points() {
        assert_eq!(find("US 2-year").change(0.045), "+4.5 bp");
        assert_eq!(find("US 10-year").change(-0.012), "-1.2 bp");
    }

    /// Market convention differs inside a single format.
    #[test]
    fn euro_dollar_shows_four_decimals_but_dollar_yen_shows_two() {
        assert_eq!(find("EUR/USD").level(1.16155), "1.1616");
        assert_eq!(find("USD/JPY").level(155.8), "155.80");
    }

    #[test]
    fn copper_keeps_the_precision_it_trades_at() {
        assert_eq!(find("Copper").level(6.6745), "6.6745");
    }

    #[test]
    fn a_change_always_carries_an_explicit_sign() {
        assert_eq!(find("S&P 500").change(-8.92), "-8.92");
        assert_eq!(find("Nasdaq 100").change(1180.4), "+1,180.40");
        assert_eq!(format_percent(-0.12), "-0.12%");
        assert_eq!(format_percent(0.4548), "+0.45%");
    }
}
