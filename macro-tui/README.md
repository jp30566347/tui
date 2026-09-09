# macro-tui

[jp30566347.github.io/macro-tui](https://jp30566347.github.io/macro-tui)

A macro market overview and the news moving it, in your terminal.

Twenty-six instruments on one screen: US indices and volatility, Treasury
yields and the dollar, commodities, foreign exchange and crypto, and the G7
world indices. Each row carries a live price, its move, and a month of daily
closes as a trend line. Pick one and get a full chart plus the headlines that
mention it. Pick a headline and read the whole story without leaving the
terminal, or turn it into an image ready to paste into a post.

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
| `h` `l` | board: jump group. news: cycle section. detail: switch chart range |
| `Enter` | board: open the detail view. news: read the story |
| `n` `N` | scroll the board's news rail |
| `f` | rail: matched headlines, or the whole pool |
| `o` | read the selected story |
| `c` | copy the selected story as an image, for a post |
| `r` | refresh everything now, or retry a story that failed to load |
| `Esc` | close the story, the detail view or an overlay. Never quits |
| `q`, `Ctrl-C` | quit |

`macro-tui --list-symbols` prints the board. `--tab 2` starts on news, and
`--save-config` remembers it.

## Reading and sharing a story

`Enter` on a headline opens the story in the terminal: CNBC's key points,
then the body, wrapped to a reading measure and scrolled with the usual keys.
Nothing is handed to a browser, so it works over SSH and in a terminal with
nothing else installed.

`c` renders the selected story as a 1600 by 900 image and copies it to the
clipboard, so it can be pasted straight into a post on X or anywhere else that
takes an image. The same file is written under `~/Pictures/macro-tui/` (or the
cache directory on a platform without a pictures folder) for attaching later.
The clipboard holds the image for as long as macro-tui runs; the file stays.

![A share card](docs/card.png)

The card is drawn as a terminal window running macro-tui: a plain frame,
the headline, the story's own key points (or the feed's one-line
summary before the story has been opened), and, when the story mentions one
of the board's instruments, that row's price, move and month of closes as a
ticker strip. A story opened from a row is tied to that row when it mentions
it; otherwise the first instrument it mentions, in board order.

## Where the data comes from

| What | Source | Refresh |
|---|---|---|
| Quotes | CNBC, all 26 in one request | 15 s |
| Daily closes | MarketWatch timeseries, all 26 in one request | 15 min |
| Headlines | CNBC top news, economy and finance | 5 min |
| Stories | The CNBC article page, when a headline is opened | cached per session |

All of them are public and keyless. Prices come from the exchanges' own feeds
via CNBC and are real-time for indices; treat them as indicative rather than as
something to trade against.

Only CNBC's feeds are carried because every source has to serve the whole
story to the app. MarketWatch and the FT answer their article pages with a
401 and a 403, so their headlines would have led nowhere.

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

**Stories are read from CNBC's embedded page data.** An article page carries
the document its front end hydrates from as `window.__s_data`, and the reader
takes the story's text from that tree rather than from the markup around it,
which changes with every redesign. The live checks above include one that
opens the newest story in every feed and fails if the body comes back empty.

**The share card's font is compiled in.** `assets/fonts/` holds JetBrains
Mono, regular and bold, subset to Latin plus the box-drawing, block and
geometric-shape ranges the card draws with. It is under the SIL Open Font
License; see the licence file beside it. The renderer sizes text by em, so a
size in the code is the cell height it would be in a terminal.

## License

MIT
