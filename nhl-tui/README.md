# nhl-tui

[jp30566347.github.io/nhl-tui](https://jp30566347.github.io/nhl-tui)

NHL scores, standings, schedule, and leaders in your terminal.

![A night's scores](docs/screenshot.png)

## Installation

```sh
curl -fsSL https://jp30566347.github.io/nhl-tui/install.sh | sh
```

Drops a single binary in `~/.local/bin` after verifying its checksum. On Arch
and Omarchy, `makepkg -si` in a clone builds a real package instead.

### Homebrew (macOS)

```sh
brew install jp30566347/tap/nhl-tui
```

### Download binary

Grab the latest binary for your platform from [GitHub Releases](https://github.com/jp30566347/nhl-tui/releases).

| Platform | File |
|----------|------|
| macOS (Apple Silicon) | `nhl-tui-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `nhl-tui-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `nhl-tui-x86_64-unknown-linux-musl.tar.gz` |
| Linux (ARM64) | `nhl-tui-aarch64-unknown-linux-musl.tar.gz` |
| Windows (untested) | `nhl-tui-x86_64-pc-windows-msvc.zip` |

### Build from source

```sh
git clone https://github.com/jp30566347/nhl-tui.git
cd nhl-tui
cargo build --release
```

## Usage

```sh
nhl-tui                    # Default: scores tab
nhl-tui --team TOR         # Highlight your favorite team, with a goal alert
nhl-tui --tab 2            # Open on a tab (1=Scores, 2=Standings, 3=Schedule,
                           #                4=Skaters, 5=Goalies)
```

## Keybindings

Press `?` in the app for this list.

| Key | Action |
|-----|--------|
| `1` - `5` | Jump to a tab |
| `Tab` / `Shift-Tab` | Cycle tabs |
| `j` `k` / `↓` `↑` | Move the selection |
| `Ctrl-D` / `Ctrl-U` | Half page down / up |
| `PgDn` / `PgUp` | Full page down / up |
| `g` `G` / `Home` `End` | First / last row |
| `h` `l` / `←` `→` | Previous/next day (Scores, Schedule), or cycle category |
| `H` / `L` | Jump back / forward one week |
| `t` | Back to today |
| `n` / `p` | Next / previous day with games |
| `d` | Go to a specific date |
| `Enter` | Open the boxscore (Scores tab) |
| `r` | Refresh everything now |
| `?` | Toggle help |
| `Esc` | Close an overlay |
| `q` / `Ctrl-C` | Quit |

## Configuration

Optional, so `--team` need not be retyped. Write the current options with:

```sh
nhl-tui --team TOR --tab 2 --save-config
```

That creates `~/.config/nhl-tui/config.toml`:

```toml
team = "TOR"
tab = 2
```

Command-line flags override the file; `--no-config` ignores it entirely. An unknown key is
reported as an error rather than silently ignored.

## Tabs

| Tab | Contents |
|-----|----------|
| Scores | One day's games, with live period and clock. `Enter` opens the boxscore: goals and shots by period, scoring with assists, penalties, three stars, and a team stat comparison. |
| Standings | Wild Card (division top threes, the two wild cards, and the playoff cut line), Conference, Division, or League. `●` marks a playoff spot. |
| Schedule | The game week around the current date. |
| Skaters | Points, goals, assists, +/-, PIM, PP and SH goals, faceoff %, TOI. |
| Goalies | Wins, GAA, save %, shutouts. |

Standings columns adapt to the terminal width, dropping GA, GF, then +/- and streak as it narrows.

## Data

Data comes from the [NHL API](https://api-web.nhle.com) and is fetched in the background, so the
interface stays responsive while requests are in flight.

Scores refresh every 30 seconds. Standings, the schedule, and season leaders change about once a
day, so they refresh every 5 minutes instead of on every cycle; `r` forces all of them immediately.

## License

MIT
