#!/bin/sh
# jet installer for macOS / Linux.
#   curl -fsSL https://github.com/y6861977-ui/jet-dist/releases/latest/download/install.sh | sh
#
# Downloads the right prebuilt jet binary for your OS/arch from the latest GitHub
# release, verifies its SHA-256, and installs it to ~/.local/bin (no sudo).
#
# Binaries live in the PUBLIC releases-only repo (jet-dist); the source stays private.
#
# Overrides (env):
#   JET_REPO=owner/repo         which GitHub repo to pull from
#   JET_VERSION=v1.2.3          a specific tag instead of latest
#   JET_INSTALL_DIR=/path/bin   where to install (default: ~/.local/bin)
set -eu

REPO="${JET_REPO:-y6861977-ui/jet-dist}"
INSTALL_DIR="${JET_INSTALL_DIR:-$HOME/.local/bin}"

red()   { printf '\033[31m%s\033[0m\n' "$1" >&2; }
green() { printf '\033[32m%s\033[0m\n' "$1"; }
dim()   { printf '\033[2m%s\033[0m\n' "$1"; }
die()   { red "error: $1"; exit 1; }

# --- 1. Detect platform ---------------------------------------------------
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)
case "$os" in
  Linux)  OS=linux ;;
  Darwin) OS=darwin ;;
  *) die "unsupported OS: $os (jet ships macOS and Linux; on Windows use install.ps1)" ;;
esac
case "$OS-$arch" in
  linux-x86_64|linux-amd64)   ASSET_ARCH=x86_64 ;;
  linux-aarch64|linux-arm64)  ASSET_ARCH=aarch64 ;;
  darwin-x86_64)              ASSET_ARCH=x86_64 ;;
  darwin-arm64|darwin-aarch64) ASSET_ARCH=arm64 ;;
  *) die "unsupported architecture: $arch" ;;
esac
ASSET="jet-${OS}-${ASSET_ARCH}"

# --- 2. Resolve download URLs --------------------------------------------
if [ -n "${JET_VERSION:-}" ]; then
  BASE="https://github.com/${REPO}/releases/download/${JET_VERSION}"
else
  BASE="https://github.com/${REPO}/releases/latest/download"
fi

# --- 3. Fetcher (curl or wget) -------------------------------------------
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  die "need curl or wget to download jet"
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t jet)
trap 'rm -rf "$tmp"' EXIT INT TERM

dim "Downloading ${ASSET} from ${REPO}…"
fetch "${BASE}/${ASSET}" "${tmp}/jet" || die "download failed: ${BASE}/${ASSET}"

# --- 4. Verify SHA-256 (if SHA256SUMS is published) ----------------------
if fetch "${BASE}/SHA256SUMS" "${tmp}/SHA256SUMS" 2>/dev/null; then
  want=$(grep " ${ASSET}\$" "${tmp}/SHA256SUMS" 2>/dev/null | awk '{print $1}' | head -n1)
  if [ -n "$want" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      got=$(sha256sum "${tmp}/jet" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
      got=$(shasum -a 256 "${tmp}/jet" | awk '{print $1}')
    else
      got=""
    fi
    if [ -n "$got" ] && [ "$got" != "$want" ]; then
      die "checksum mismatch — refusing to install (expected $want, got $got)"
    fi
    [ -n "$got" ] && dim "checksum ok"
  fi
else
  dim "no SHA256SUMS published — skipping checksum verification"
fi

# --- 5. Install -----------------------------------------------------------
mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
chmod +x "${tmp}/jet"
mv -f "${tmp}/jet" "${INSTALL_DIR}/jet" || die "cannot write to $INSTALL_DIR"

green "✓ jet installed to ${INSTALL_DIR}/jet"

# --- 6. PATH check --------------------------------------------------------
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) : ;;
  *)
    echo ""
    dim "${INSTALL_DIR} is not on your PATH. Add it:"
    printf '  echo '\''export PATH="%s:$PATH"'\'' >> ~/.profile && . ~/.profile\n' "$INSTALL_DIR"
    ;;
esac

echo ""
green "Done. Open a new terminal and run:  jet help"
