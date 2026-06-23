#!/bin/sh
# llmlint installer.
#
# Detect the host platform, download the matching prebuilt binary from the
# GitHub release, verify its SHA-256 checksum, and install it onto your PATH.
#
# Install the latest release:
#   curl -fsSL https://raw.githubusercontent.com/nickderobertis/llmlint/main/scripts/install.sh | sh
#
# Pin a version or choose where it lands (flags win over the env vars):
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --to ~/.local/bin
#
# Equivalent environment variables: LLMLINT_VERSION, LLMLINT_INSTALL_DIR.
# Set GITHUB_TOKEN to lift the GitHub API rate limit when resolving "latest".
#
# Covers Linux and macOS (x86_64, arm64) and Windows x86_64 under a POSIX shell
# (Git Bash / MSYS / WSL). For native Windows PowerShell or unpublished targets,
# use `cargo install llmlint --locked`.
#
# Like the tool it installs, this script never weakens silently: it aborts
# rather than install a binary it cannot checksum-verify.

set -eu

REPO="nickderobertis/llmlint"
BIN="llmlint"
# Overridden to `llmlint.exe` on Windows targets by detect_target.
BIN_FILE="$BIN"

say() { printf '%s\n' "$*" >&2; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat >&2 <<EOF
Install the prebuilt llmlint binary.

Usage: install.sh [--version <tag>] [--to <dir>]

  --version <tag>   Release tag to install, e.g. v0.1.0 (default: latest).
  --to <dir>        Install directory (default: ~/.local/bin).
  -h, --help        Show this help.

Environment: LLMLINT_VERSION, LLMLINT_INSTALL_DIR, GITHUB_TOKEN.
EOF
}

# Map `uname` output to a published Rust target triple, archive extension, and
# (on Windows) the `.exe` binary name. The triples must match the targets the
# release workflow builds (.github/workflows/release.yml). Unsupported pairs
# abort with guidance.
detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux) os_part="unknown-linux-gnu"; ext="tar.gz" ;;
        Darwin) os_part="apple-darwin"; ext="tar.gz" ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            os_part="pc-windows-msvc"; ext="zip"; BIN_FILE="${BIN}.exe" ;;
        *) err "unsupported operating system: $os" ;;
    esac

    case "$arch" in
        x86_64 | amd64) arch_part="x86_64" ;;
        arm64 | aarch64) arch_part="aarch64" ;;
        *) err "unsupported architecture: $arch" ;;
    esac

    # The release matrix publishes Windows for x86_64 only.
    if [ "$ext" = "zip" ] && [ "$arch_part" != "x86_64" ]; then
        err "no prebuilt Windows binary for $arch; install with 'cargo install $BIN --locked'"
    fi

    TARGET="${arch_part}-${os_part}"
    EXT="$ext"
}

# Fetch a URL to stdout. Used only for the GitHub API call, so it carries the
# optional token; release-asset downloads stay tokenless to avoid sending an
# Authorization header to the redirected (signed) asset URL.
api_get() {
    _url="$1"
    if [ "$DL" = "curl" ]; then
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$_url"
        else
            curl -fsSL "$_url"
        fi
    else
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            wget --header="Authorization: Bearer $GITHUB_TOKEN" -qO- "$_url"
        else
            wget -qO- "$_url"
        fi
    fi
}

# Download a release asset (follows redirects) to a file.
download() {
    _url="$1"
    _out="$2"
    if [ "$DL" = "curl" ]; then
        curl -fsSL -o "$_out" "$_url"
    else
        wget -qO "$_out" "$_url"
    fi
}

# Resolve the latest release tag by reading "tag_name" from the GitHub API.
latest_tag() {
    _body="$(api_get "https://api.github.com/repos/$REPO/releases/latest")" \
        || err "could not query the latest release (set GITHUB_TOKEN if rate-limited)"
    _tag="$(printf '%s\n' "$_body" \
        | grep -m1 '"tag_name"' \
        | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')"
    [ -n "$_tag" ] || err "could not parse the latest release tag from the GitHub API"
    printf '%s\n' "$_tag"
}

# Print the SHA-256 of a file using whichever tool is available. Aborts when
# none is found rather than skip verification.
sha256_of() {
    _f="$1"
    if have sha256sum; then
        sha256sum "$_f" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$_f" | awk '{print $1}'
    elif have openssl; then
        openssl dgst -sha256 "$_f" | awk '{print $NF}'
    else
        err "no SHA-256 tool (need sha256sum, shasum, or openssl); refusing to install unverified"
    fi
}

extract() {
    _archive="$1"
    _dest="$2"
    case "$_archive" in
        *.tar.gz) tar -xzf "$_archive" -C "$_dest" ;;
        *.zip)
            have unzip || err "need 'unzip' to extract $_archive"
            unzip -q "$_archive" -d "$_dest" ;;
        *) err "unknown archive type: $_archive" ;;
    esac
}

main() {
    version="${LLMLINT_VERSION:-}"
    bindir="${LLMLINT_INSTALL_DIR:-}"

    while [ $# -gt 0 ]; do
        case "$1" in
            --version) version="${2:?--version needs a value}"; shift 2 ;;
            --version=*) version="${1#*=}"; shift ;;
            --to | --bin-dir) bindir="${2:?--to needs a value}"; shift 2 ;;
            --to=* | --bin-dir=*) bindir="${1#*=}"; shift ;;
            -h | --help) usage; exit 0 ;;
            *) err "unknown option: $1 (try --help)" ;;
        esac
    done

    [ -n "$bindir" ] || bindir="${HOME}/.local/bin"

    if have curl; then
        DL="curl"
    elif have wget; then
        DL="wget"
    else
        err "need curl or wget to download"
    fi

    detect_target

    if [ -z "$version" ]; then
        say "resolving latest release..."
        version="$(latest_tag)"
    fi

    archive="${BIN}-${version}-${TARGET}.${EXT}"
    # The release action names the checksum asset by replacing the archive
    # extension with `.sha256` (not appending), e.g. llmlint-v0.1.0-<t>.sha256.
    sumfile="${BIN}-${version}-${TARGET}.sha256"
    base_url="https://github.com/$REPO/releases/download/${version}"

    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t llmlint)" \
        || err "could not create a temporary directory"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "downloading ${archive} (${version})..."
    download "${base_url}/${archive}" "${tmp}/${archive}" \
        || err "download failed: ${base_url}/${archive}"
    download "${base_url}/${sumfile}" "${tmp}/${sumfile}" \
        || err "checksum download failed: ${base_url}/${sumfile}"

    say "verifying checksum..."
    expected="$(awk '{print $1}' "${tmp}/${sumfile}")"
    actual="$(sha256_of "${tmp}/${archive}")"
    [ -n "$expected" ] || err "empty checksum file for ${archive}"
    [ "$expected" = "$actual" ] \
        || err "checksum mismatch for ${archive} (expected ${expected}, got ${actual})"

    mkdir -p "${tmp}/unpack"
    extract "${tmp}/${archive}" "${tmp}/unpack"

    src="${tmp}/unpack/${BIN_FILE}"
    if [ ! -f "$src" ]; then
        # taiki-e/upload-rust-binary-action keeps the binary at the archive
        # root, but fall back to a search so a leading-dir layout still works.
        src="$(find "${tmp}/unpack" -type f -name "$BIN_FILE" -print 2>/dev/null | head -n1)"
        [ -n "$src" ] && [ -f "$src" ] || err "binary '$BIN_FILE' not found in ${archive}"
    fi

    mkdir -p "$bindir" || err "could not create install directory: $bindir"
    dest="${bindir}/${BIN_FILE}"
    if have install; then
        install -m 0755 "$src" "$dest" || err "could not install to $dest"
    else
        cp "$src" "$dest" && chmod 0755 "$dest" || err "could not install to $dest"
    fi

    say "installed ${BIN} ${version} to ${dest}"

    case ":${PATH}:" in
        *":${bindir}:"*) ;;
        *)
            say ""
            say "NOTE: ${bindir} is not on your PATH. Add it to your shell profile:"
            say "  export PATH=\"${bindir}:\$PATH\""
            ;;
    esac

    say "run '${BIN} doctor' to confirm oneharness is reachable."
}

main "$@"
