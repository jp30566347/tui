# macro-tui

[jp30566347.github.io/macro-tui](https://jp30566347.github.io/macro-tui)

A macro market overview and the news moving it, in your terminal.

Twenty-six instruments on one screen: US indices and volatility, Treasury
yields and the dollar, commodities, foreign exchange and crypto, and the G7
world indices. Each row carries a live price, its move, and a month of daily
closes as a trend line. Pick one and get a full chart plus the headlines that
mention it.

No API key, no signup, no configuration. It works the moment it starts.

![The board](docs/screenshot.png)


## Install

```sh
curl -fsSL https://jp30566347.github.io/macro-tui/install.sh | sh
```

Drops a single binary in `~/.local/bin` after verifying its checksum. On Arch
and Omarchy, `makepkg -si` in a clone builds a real package instead. There are
also prebuilt binaries on the [releases page][releases], and
`cargo install --git https://github.com/jp30566347/macro-tui` builds from
source.

[releases]: https://github.com/jp30566347/macro-tui/releases/latest

## Keys

| Key | Action |
|---|---|
| `1` `2`, `Tab` | switch between the board and news |
| `j` `k`, arrows | move the selection |
| `Ctrl-D` `Ctrl-U` | half page down / up |
| `g` `G`, Home/End | first / last row |
| `h` `l` | board: jump group. news: cycle source. detail: switch chart range |
| `Enter` | board: open the detail view. news: open the story |
| `n` `N` | scroll the board's news rail |
| `f` | rail: matched headlines, or the whole pool |
| `o` | open the selected story in a browser |
| `r` | refresh everything now |
| `Esc` | close the detail view or an overlay. Never quits |
| `q`, `Ctrl-C` | quit |

`macro-tui --list-symbols` prints the board. `--tab 2` starts on news, and
`--save-config` remembers it.

## Where the data comes from

| What | Source | Refresh |
|---|---|---|
| Quotes | CNBC, all 26 in one request | 15 s |
| Daily closes | MarketWatch timeseries, all 26 in one request | 15 min |
| Headlines | CNBC top, economy and finance; MarketWatch; the FT | 5 min |

All of them are public and keyless. Prices come from the exchanges' own feeds
via CNBC and are real-time for indices; treat them as indicative rather than as
something to trade against.

The app renders to stderr, so stdout stays free and piping it is safe.

## Notes for maintainers

Two things about the upstream endpoints are worth knowing before changing
`src/catalog.rs`.

**A single bad history key fails the whole batch.** The MarketWatch request
carries all twenty-six series and answers a 400 if any one key is
unrecognised. The app recovers by re-probing keys individually and
quarantining the bad one for the rest of the session, but a new row should be
verified first. Run the live checks:

```sh
cargo test -- --ignored --nocapture
```

To find a key, note the country code in MarketWatch's own URL for the
instrument (`/investing/index/dax?countrycode=dx`) and try
`INDEX/<cc>//<TICKER>` and `INDEX/<cc>/<MIC>/<TICKER>`. A wrong key answers in
about 200 ms, so a small grid finds it quickly.

**The quote endpoint's reported percent change is not consistent.** For the
2-year note it carries the change in the bond's *price*, which has the opposite
sign to the change in its yield, so a day when yields rose would render red.
The app recomputes every move from the last price and the previous close and
only falls back to the reported fields when there is no previous close.

The user agent string is also load-bearing: the quote endpoint's CDN answers a
default client string with a 403.

## License

MIT
