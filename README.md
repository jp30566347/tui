# tui

[jp30566347.github.io/tui](https://jp30566347.github.io/tui)

Small things that live in a terminal. One static binary each, no API keys, no
signup, no configuration.

| | |
|---|---|
| **[macro-tui](macro-tui/)** | Twenty-six macro instruments with live prices, a month of daily closes, and the headlines moving them. |
| **[nhl-tui](nhl-tui/)** | Live scores, the wild card race, the schedule, the leaders, and the whole boxscore. |

## Install

```sh
curl -fsSL https://jp30566347.github.io/tui/macro-tui/install.sh | sh
curl -fsSL https://jp30566347.github.io/tui/nhl-tui/install.sh | sh
```

Each drops a single binary in `~/.local/bin` after verifying its checksum. On
Arch and Omarchy, `makepkg -si` inside either app's directory builds a real
package. `nhl-tui` is also on Homebrew as
`jp30566347/tap/nhl-tui`.

## Layout

```
crates/tui-common/   the parts both apps need
macro-tui/           markets
nhl-tui/             hockey
docs/                the website, deployed to GitHub Pages
```

`tui-common` holds what was duplicated between the two before they moved into
one repo: the terminal lifecycle, the background event pump, the config
loader, the size-capped HTTP body reader, and the layout helpers. A bug fixed
there is fixed in both. What stayed in each app is the part that genuinely
differs, which is its state, its keys, its screens, and the loop that maps its
own actions onto them.

## Development

```sh
cargo test --workspace        # 131 tests, all offline
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo run -p macro-tui
```

Each app also has network-backed checks that are `#[ignore]`d so CI never
depends on a third party. Run them by hand when an upstream feed might have
moved:

```sh
cargo test -p macro-tui -- --ignored --nocapture
```

## Releasing

The repo holds two applications, so a release tag says which one:

```sh
git tag -a macro-tui-v0.1.2 -m "macro-tui v0.1.2" && git push origin macro-tui-v0.1.2
```

That builds only that package, for x86-64 and ARM Linux (static musl), Intel
and Apple silicon macOS, and Windows, and publishes the archives with a
SHA-256 beside each. The Windows build is produced but has never been run on
Windows; treat it as untested.

## License

MIT
