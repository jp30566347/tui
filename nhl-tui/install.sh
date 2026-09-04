#!/bin/sh
# Installs nhl-tui from the latest GitHub release.
#
#   curl -fsSL https://jp30566347.github.io/tui/nhl-tui/install.sh | sh
#
# Downloads the binary for this platform, verifies its checksum, and puts it in
# ~/.local/bin. Set NHL_TUI_BIN_DIR to install somewhere else, or
# NHL_TUI_VERSION to pin a tag such as nhl-tui-v0.1.0.
#
# POSIX sh on purpose: this has to run under dash and busybox ash, not just
# bash.
set -eu

REPO="jp30566347/tui"
BIN_DIR="${NHL_TUI_BIN_DIR:-$HOME/.local/bin}"
VERSION="${NHL_TUI_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
# Diagnostics go to stderr so that piping this script somewhere cannot swallow
# the reason it stopped.
err() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || err "$1 is required but not installed"
}

target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64) echo "x86_64-unknown-linux-musl" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-musl" ;;
                *) err "unsupported architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64) echo "aarch64-apple-darwin" ;;
                *) err "unsupported architecture: $arch" ;;
            esac
            ;;
        *)
            err "unsupported operating system: $os. Windows users can download the zip from https://github.com/$REPO/releases"
            ;;
    esac
}

need uname
need tar
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    err "either curl or wget is required"
fi

TARGET=$(target)
# Two applications share this repo, so releases are tagged per app as
# nhl-tui-vX.Y.Z. "latest" across the repo could be the other one, so the
# newest nhl-tui tag is looked up instead of assuming it.
if [ "$VERSION" = "latest" ]; then
    VERSION=$(fetch_stdout "https://api.github.com/repos/$REPO/releases?per_page=30" |
        tr ',' '\n' | grep '"tag_name"' | cut -d'"' -f4 |
        grep "^nhl-tui-v" | head -1)
    [ -n "$VERSION" ] || err "could not find a published nhl-tui release"
fi
BASE="https://github.com/$REPO/releases/download/$VERSION"
ARCHIVE="nhl-tui-$TARGET.tar.gz"

TMP=$(mktemp -d)
# Cleans up on success, failure and interrupt alike.
trap 'rm -rf "$TMP"' EXIT INT TERM

say "Downloading nhl-tui ($TARGET)..."
fetch "$BASE/$ARCHIVE" "$TMP/$ARCHIVE" ||
    err "could not download $BASE/$ARCHIVE"

# Verified when a checksum tool is available. A missing sha256sum should not
# block the install, but a mismatch always does.
if fetch "$BASE/$ARCHIVE.sha256" "$TMP/$ARCHIVE.sha256" 2>/dev/null; then
    expected=$(cut -d' ' -f1 <"$TMP/$ARCHIVE.sha256")
    actual=""
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$TMP/$ARCHIVE" | cut -d' ' -f1)
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$TMP/$ARCHIVE" | cut -d' ' -f1)
    fi
    if [ -n "$actual" ]; then
        [ "$expected" = "$actual" ] || err "checksum mismatch: expected $expected, got $actual"
        say "Checksum verified."
    fi
fi

tar xzf "$TMP/$ARCHIVE" -C "$TMP" || err "could not unpack $ARCHIVE"
[ -f "$TMP/nhl-tui" ] || err "archive did not contain a nhl-tui binary"

mkdir -p "$BIN_DIR"
# Written to a temporary name first and then moved, so a running copy of
# nhl-tui is never overwritten in place.
chmod +x "$TMP/nhl-tui"
mv -f "$TMP/nhl-tui" "$BIN_DIR/nhl-tui.new"
mv -f "$BIN_DIR/nhl-tui.new" "$BIN_DIR/nhl-tui"

say ""
say "Installed $("$BIN_DIR/nhl-tui" --version) to $BIN_DIR/nhl-tui"

case ":${PATH}:" in
    *":$BIN_DIR:"*)
        say "Run it with: nhl-tui"
        ;;
    *)
        say ""
        say "$BIN_DIR is not on your PATH. Add it with:"
        say ""
        say "    echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc && exec bash"
        say ""
        say "Or run it directly: $BIN_DIR/nhl-tui"
        ;;
esac
