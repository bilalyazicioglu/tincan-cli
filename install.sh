#!/bin/sh
# tincan installer — https://github.com/bilalyazicioglu/tincan-cli
#
#   curl -fsSL https://raw.githubusercontent.com/bilalyazicioglu/tincan-cli/main/install.sh | sh
#
# Downloads a prebuilt binary for your platform and drops it in ~/.local/bin.
# If no prebuilt binary matches, falls back to building from source with cargo.
#
# Environment:
#   TINCAN_VERSION      version tag to install (default: the latest release)
#   TINCAN_INSTALL_DIR  where to install (default: $HOME/.local/bin)

set -eu

REPO="bilalyazicioglu/tincan-cli"
INSTALL_DIR="${TINCAN_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${TINCAN_VERSION:-latest}"

BOLD=''; DIM=''; RED=''; GREEN=''; RESET=''
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m')
    RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m'); RESET=$(printf '\033[0m')
fi

say()  { printf '  %s\n' "$*"; }
ok()   { printf '  %s✔%s %s\n' "$GREEN" "$RESET" "$*"; }
warn() { printf '  %s!%s %s\n' "$BOLD" "$RESET" "$*" >&2; }
die()  { printf '  %s✘%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
tincan installer

  install.sh [--help]

Environment:
  TINCAN_VERSION       version tag to install, e.g. v0.1.0 (default: latest)
  TINCAN_INSTALL_DIR   install location (default: $HOME/.local/bin)
USAGE
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
    '') ;;
    *) usage >&2; die "unknown argument: $1" ;;
esac

# ── Platform detection ──────────────────────────────────────────────────────

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os/$arch" in
        Darwin/arm64)         echo "aarch64-apple-darwin" ;;
        Darwin/x86_64)        echo "x86_64-apple-darwin" ;;
        Linux/x86_64|Linux/amd64)  echo "x86_64-unknown-linux-gnu" ;;
        Linux/aarch64|Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) return 1 ;;
    esac
}

# ── Fallback: build from source ─────────────────────────────────────────────

build_from_source() {
    if ! command -v cargo >/dev/null 2>&1; then
        printf '\n' >&2
        warn "no prebuilt binary for this platform, and cargo is not installed."
        say "  Install Rust from https://rustup.rs, then run:" >&2
        say "" >&2
        say "      cargo install --git https://github.com/$REPO" >&2
        say "" >&2
        say "  You will also need Opus: 'brew install opus pkg-config' on macOS, or" >&2
        say "  'apt install libopus-dev pkg-config libasound2-dev' on Debian/Ubuntu." >&2
        printf '\n' >&2
        exit 1
    fi
    warn "no prebuilt binary for this platform; building from source instead."
    say "${DIM}This needs Opus (or autotools to build it) and takes a few minutes.${RESET}"
    printf '\n'
    cargo install --git "https://github.com/$REPO" --locked
    printf '\n'
    ok "tincan installed with cargo (in ~/.cargo/bin)"
    exit 0
}

# ── Download helpers ────────────────────────────────────────────────────────

fetch() {  # fetch <url> <destination>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        die "neither curl nor wget is available"
    fi
}

sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        echo ""
    fi
}

# ── Install ─────────────────────────────────────────────────────────────────

printf '\n  %stincan%s — serverless voice chat in your terminal\n\n' "$BOLD" "$RESET"

TARGET=$(detect_target) || build_from_source
say "platform: ${BOLD}${TARGET}${RESET}"

ASSET="tincan-${TARGET}.tar.gz"
if [ "$VERSION" = "latest" ]; then
    BASE="https://github.com/$REPO/releases/latest/download"
else
    BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

if ! fetch "$BASE/$ASSET" "$TMP/$ASSET" 2>/dev/null; then
    build_from_source
fi
ok "downloaded $ASSET"

# Verify the checksum when one is published and we have a tool to check it.
if fetch "$BASE/$ASSET.sha256" "$TMP/$ASSET.sha256" 2>/dev/null; then
    expected=$(cut -d' ' -f1 < "$TMP/$ASSET.sha256")
    actual=$(sha256_of "$TMP/$ASSET")
    if [ -z "$actual" ]; then
        warn "no sha256 tool found; skipping checksum verification"
    elif [ "$expected" != "$actual" ]; then
        die "checksum mismatch — refusing to install (expected $expected, got $actual)"
    else
        ok "checksum verified"
    fi
else
    warn "no published checksum for this release; skipping verification"
fi

tar -xzf "$TMP/$ASSET" -C "$TMP" || die "could not extract $ASSET"
[ -f "$TMP/tincan" ] || die "the archive does not contain a tincan binary"

mkdir -p "$INSTALL_DIR"
mv "$TMP/tincan" "$INSTALL_DIR/tincan"
chmod +x "$INSTALL_DIR/tincan"
ok "installed to ${BOLD}${INSTALL_DIR}/tincan${RESET}"

# ── Post-install notes ──────────────────────────────────────────────────────

# cpal links ALSA dynamically on Linux, so the library has to be present.
if [ "$(uname -s)" = "Linux" ] && ! ldconfig -p 2>/dev/null | grep -q libasound.so.2; then
    printf '\n'
    warn "libasound2 is missing — tincan needs it for audio."
    say "${DIM}Debian/Ubuntu: sudo apt install libasound2${RESET}"
    say "${DIM}Fedora:        sudo dnf install alsa-lib${RESET}"
fi

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf '\n'
        warn "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
        printf '\n      export PATH="%s:$PATH"\n' "$INSTALL_DIR"
        ;;
esac

printf '\n  Get started:\n\n'
printf '      %stincan host --name you%s        open a room\n' "$BOLD" "$RESET"
printf '      %stincan join <code>%s            join someone else'"'"'s\n' "$BOLD" "$RESET"
printf '      %stincan devices%s                list audio devices\n\n' "$BOLD" "$RESET"
