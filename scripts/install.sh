#!/bin/sh
# Install a prebuilt llmlint release binary on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/nickderobertis/llmlint/main/scripts/install.sh | sh
#
# Honors:
#   LLMLINT_VERSION       release tag to install (default: latest)
#   LLMLINT_INSTALL_DIR   install directory (default: ~/.local/bin)
#
# Downloads the archive on the same `<bin>-<tag>-<target>` naming contract that
# .github/workflows/release.yml produces, verifies its sha256, and installs the
# binary. Windows users: use `cargo install --git` or a Releases archive.
set -eu

REPO="nickderobertis/llmlint"
BIN="llmlint"
INSTALL_DIR="${LLMLINT_INSTALL_DIR:-$HOME/.local/bin}"

err() {
    echo "install.sh: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || err "required tool '$1' not found"
}

need curl
need tar

# --- detect target triple -------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) err "unsupported OS '$os'; use 'cargo install --git https://github.com/$REPO'" ;;
esac
case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) err "unsupported architecture '$arch'" ;;
esac
target="${arch_part}-${os_part}"

# --- resolve version ------------------------------------------------------
tag="${LLMLINT_VERSION:-}"
if [ -z "$tag" ]; then
    tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | grep '"tag_name"' | head -n1 | cut -d'"' -f4)"
    [ -n "$tag" ] || err "could not resolve the latest release; set LLMLINT_VERSION"
fi

archive="${BIN}-${tag}-${target}.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading $archive ($tag)..."
curl -fsSL "$base/$archive" -o "$tmp/$archive" || err "download failed: $base/$archive"
curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256" || err "checksum download failed"

# --- verify checksum ------------------------------------------------------
echo "verifying checksum..."
expected="$(cut -d' ' -f1 <"$tmp/$archive.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$archive" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$archive" | cut -d' ' -f1)"
else
    err "no sha256 tool (sha256sum/shasum) found"
fi
[ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"

# --- install --------------------------------------------------------------
tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/$BIN" ] || err "archive did not contain '$BIN'"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/$BIN" "$INSTALL_DIR/$BIN" 2>/dev/null \
    || { cp "$tmp/$BIN" "$INSTALL_DIR/$BIN" && chmod 0755 "$INSTALL_DIR/$BIN"; }

echo "installed $BIN $tag to $INSTALL_DIR/$BIN"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: add $INSTALL_DIR to your PATH" ;;
esac
echo "run '$BIN doctor' to confirm oneharness is reachable."
