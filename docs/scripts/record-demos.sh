#!/usr/bin/env bash
# Record the two demo GIFs on the docs site, and a poster frame for each.
#
#   docs/scripts/record-demos.sh             # both
#   docs/scripts/record-demos.sh macro-tui   # just one
#
# Both apps are driven for real: the binaries run in a pty against the live
# feeds, so what the GIF shows is what the app draws. drive.py types the keys
# from the .keys.json script beside it and writes an asciicast; agg renders
# that to a GIF using the JetBrains Mono shipped in macro-tui/assets/fonts and
# the docs palette, which drive.py puts in the asciicast header.
#
# Needs: agg (cargo install --git https://github.com/asciinema/agg), python3
# and imagemagick. Re-record whenever the UI changes enough that a GIF lies.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$here/../.." && pwd)
docs="$root/docs"
fonts="$root/macro-tui/assets/fonts"
agg=${AGG:-$(command -v agg || echo "$HOME/.cargo/bin/agg")}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

[ -x "$agg" ] || { echo "agg not found; cargo install --git https://github.com/asciinema/agg" >&2; exit 1; }

echo "building release binaries"
cargo build --release --manifest-path "$root/Cargo.toml" -p macro-tui -p nhl-tui

# A throwaway HOME keeps a developer's own config file out of the recording.
export HOME="$work/home"
mkdir -p "$HOME"

record() {
  local app=$1 cols=$2 rows=$3
  shift 3
  echo "recording $app (${cols}x${rows})"
  python3 "$here/drive.py" \
    --out "$work/$app.cast" --cols "$cols" --rows "$rows" \
    --script "$here/$app.keys.json" --title "$app" \
    -- "$root/target/release/$app" --no-config "$@"

  echo "rendering $app.gif"
  "$agg" --quiet \
    --font-dir "$fonts" \
    --font-family "JetBrains Mono,Noto Sans Mono,DejaVu Sans Mono" \
    --font-size 15 --line-height 1.35 \
    --fps-cap 12 --idle-time-limit 2 --last-frame-duration 2.5 \
    "$work/$app.cast" "$docs/$app/demo.gif"

  # The poster is what the page shows before the GIF is swapped in, and all
  # the page shows when the reader has asked for less motion. Take a frame
  # from the middle, where the app is doing something, not the splash.
  local mid
  mid=$(($(magick identify "$docs/$app/demo.gif" | wc -l) / 2))
  magick "$docs/$app/demo.gif" -coalesce \
    -delete "0-$((mid - 1))" -delete "1--1" "$docs/$app/demo-poster.png"

  printf '  %-9s %s  %s\n' "$app" \
    "$(magick identify -format '%wx%h' "$docs/$app/demo.gif[0]")" \
    "$(du -h "$docs/$app/demo.gif" | cut -f1)"
}

only=${1:-}
if [ -z "$only" ] || [ "$only" = macro-tui ]; then record macro-tui 112 32; fi
if [ -z "$only" ] || [ "$only" = nhl-tui ]; then record nhl-tui 108 28 --team NYR; fi

echo "done"
