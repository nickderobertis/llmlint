#!/bin/sh
# llmlint installer.
#
# Detect the host platform, download the matching prebuilt binary, verify it
# against a trust root independent of where it was downloaded, and install it
# onto your PATH.
#
# Install the latest release:
#   curl -fsSL https://raw.githubusercontent.com/nickderobertis/llmlint/main/scripts/install.sh | sh
#
# Pin a version or choose where it lands (flags win over the env vars):
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --to ~/.local/bin
#
# Behind a release-proxy mirror (a network that can reach a mirror but not
# github.com), point the archive download at it:
#   LLMLINT_RELEASE_BASE_URL=https://mirror.example/llmlint sh install.sh
# The archive comes from the mirror, but its integrity is still checked against
# a trust root that the mirror does not control (see "Verification" below).
#
# Equivalent environment variables: LLMLINT_VERSION, LLMLINT_INSTALL_DIR,
# LLMLINT_RELEASE_BASE_URL, LLMLINT_CHECKSUM_BASE_URL.
# Set GITHUB_TOKEN to lift the GitHub API rate limit when resolving "latest".
#
# Covers Linux and macOS (x86_64, arm64) and Windows x86_64 under a POSIX shell
# (Git Bash / MSYS / WSL). For native Windows PowerShell or unpublished targets,
# use `cargo install llmlint --locked`.
#
# Verification. Like the tool it installs, this script never weakens silently: it
# aborts rather than install a binary it cannot vouch for, and — crucially — it
# never trusts a mirror to attest its own download. Two independent roots, tried
# in order:
#   1. Sigstore build-provenance attestation (preferred). Each release ships a
#      `.sigstore.json` bundle beside the archive; when `cosign` (or `gh`) is
#      present the bundle is verified OFFLINE against the keyless signature bound
#      to this repo's release workflow. The trusted digest comes from the SIGNED
#      attestation itself — no checksum file is consulted — so a mirror cannot
#      forge it. Served alongside the archive (from the mirror), it needs no
#      GitHub API and works behind a mirror that can't reach github.com. No key
#      or secret required.
#   2. SHA-256 checksum from canonical GitHub (fallback, only when no verifier is
#      installed). The `.sha256` is fetched from the release on github.com, NOT
#      from the mirror. A checksum that shares the mirror's origin is no trust
#      root at all — the mirror would serve a matching tampered checksum — so the
#      installer REFUSES it (install `cosign` instead) rather than trust the
#      mirror to vouch for its own download.
# If nothing independent of the mirror can vouch for the archive, the install
# aborts.

set -eu

REPO="nickderobertis/llmlint"
BIN="llmlint"
# Overridden to `llmlint.exe` on Windows targets by detect_target.
BIN_FILE="$BIN"

# Canonical release host. The archive may be fetched from a mirror (see
# LLMLINT_RELEASE_BASE_URL), but checksums default to this host so the integrity
# root stays independent of the (possibly untrusted) mirror.
CANONICAL_BASE_URL="https://github.com/$REPO/releases/download"

# Expected keyless identity of the release workflow that signs the archives.
# A valid Sigstore bundle must carry this OIDC issuer and a signing-certificate
# identity for this repo's release workflow, so a mirror cannot substitute its
# own signed artifact.
OIDC_ISSUER="https://token.actions.githubusercontent.com"
PROVENANCE_IDENTITY_RE="^https://github.com/${REPO}/\\.github/workflows/release\\.yml@"
# actions/attest-build-provenance emits SLSA provenance v1; constrain cosign to
# that predicate so it can't match some other attestation over the same digest.
PROVENANCE_TYPE="https://slsa.dev/provenance/v1"

say() { printf '%s\n' "$*" >&2; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat >&2 <<EOF
Install the prebuilt llmlint binary.

Usage: install.sh [--version <tag>] [--to <dir>] [--base-url <url>]

  --version <tag>   Release tag to install, e.g. v0.1.0 (default: latest).
  --to <dir>        Install directory (default: ~/.local/bin).
  --base-url <url>  Download the archive from this mirror instead of GitHub.
                    Integrity is still checked against a root the mirror does
                    not control (Sigstore attestation, or canonical checksum).
  -h, --help        Show this help.

Environment: LLMLINT_VERSION, LLMLINT_INSTALL_DIR, LLMLINT_RELEASE_BASE_URL,
LLMLINT_CHECKSUM_BASE_URL, GITHUB_TOKEN.
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

# Try to verify the archive from its Sigstore build-provenance bundle, using
# whichever standalone verifier is installed — `cosign` preferred (vendor-neutral,
# no GitHub API), then `gh` with `--bundle` (also offline). The bundle is served
# alongside the archive, so this works behind a mirror that can't reach github.com;
# the signature is bound to this repo's release workflow, so the mirror can't forge
# it. Returns 0 on a good verification; 1 to fall through to the checksum root (no
# verifier installed, no bundle published, or a tooling/soft failure). A real
# tamper still fails closed — the checksum root then rejects the archive.
verify_sigstore() {
    _archive="$1"       # local path to the downloaded archive
    _bundle_url="$2"    # bundle URL (served with the archive)
    _bundle="${_archive}.sigstore.json"

    have cosign || have gh || return 1

    download "$_bundle_url" "$_bundle" 2>/dev/null || {
        say "no attestation bundle at ${_bundle_url}; using the checksum root."
        return 1
    }

    if have cosign; then
        say "verifying build provenance with cosign (Sigstore, offline)..."
        if cosign verify-blob-attestation \
            --new-bundle-format \
            --bundle "$_bundle" \
            --type "$PROVENANCE_TYPE" \
            --certificate-oidc-issuer "$OIDC_ISSUER" \
            --certificate-identity-regexp "$PROVENANCE_IDENTITY_RE" \
            "$_archive" >/dev/null 2>&1; then
            say "verified: attested by ${REPO}'s release workflow (cosign)."
            return 0
        fi
        say "cosign could not verify the attestation; trying the next root."
    fi

    if have gh; then
        say "verifying build provenance with gh (Sigstore, offline)..."
        if gh attestation verify "$_archive" --bundle "$_bundle" --repo "$REPO" \
            >/dev/null 2>&1; then
            say "verified: attested by ${REPO}'s release workflow (gh)."
            return 0
        fi
        say "gh could not verify the attestation; trying the next root."
    fi

    return 1
}

# Verify the downloaded archive against a trust root that is INDEPENDENT of the
# (possibly mirrored) source it was downloaded from. Preferred: the Sigstore
# build-provenance bundle (see verify_sigstore) — there the trusted digest comes
# from the signed attestation itself, so no checksum file is trusted at all.
# Fallback (only when no verifier is installed): a SHA-256 checksum from a root
# that is NOT the mirror. A checksum that shares the mirror's origin is no trust
# root at all — a tampered mirror would serve a matching tampered checksum — so
# we refuse it rather than fetch it. Aborts if nothing independent vouches for
# the archive, so a tampered mirror can never yield an installed binary.
verify_archive() {
    _archive="$1"       # local path to the downloaded archive
    _bundle_url="$2"    # Sigstore bundle URL (served with the archive)
    _sum_url="$3"       # checksum URL on the independent trust root
    _sum_trusted="$4"   # "yes" iff _sum_url is independent of the mirror

    if verify_sigstore "$_archive" "$_bundle_url"; then
        return 0
    fi

    if [ "$_sum_trusted" != "yes" ]; then
        err "cannot verify $(basename "$_archive") independently of the mirror:\
 no Sigstore verifier vouched for it (install 'cosign'), and the only checksum\
 shares the mirror's origin so it is not an independent trust root. Install\
 cosign, or set LLMLINT_CHECKSUM_BASE_URL to a root the mirror does not control."
    fi

    say "verifying SHA-256 checksum from ${_sum_url}..."
    download "$_sum_url" "${_archive}.sha256" \
        || err "checksum download failed from the trust root: ${_sum_url}"
    _expected="$(awk '{print $1}' "${_archive}.sha256")"
    [ -n "$_expected" ] || err "empty checksum file at ${_sum_url}"
    _actual="$(sha256_of "$_archive")"
    [ "$_expected" = "$_actual" ] || err \
        "checksum mismatch for $(basename "$_archive") (expected ${_expected}, got ${_actual})"
    say "checksum OK."
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
    release_base="${LLMLINT_RELEASE_BASE_URL:-}"
    checksum_base="${LLMLINT_CHECKSUM_BASE_URL:-}"

    while [ $# -gt 0 ]; do
        case "$1" in
            --version) version="${2:?--version needs a value}"; shift 2 ;;
            --version=*) version="${1#*=}"; shift ;;
            --to | --bin-dir) bindir="${2:?--to needs a value}"; shift 2 ;;
            --to=* | --bin-dir=*) bindir="${1#*=}"; shift ;;
            --base-url) release_base="${2:?--base-url needs a value}"; shift 2 ;;
            --base-url=*) release_base="${1#*=}"; shift ;;
            -h | --help) usage; exit 0 ;;
            *) err "unknown option: $1 (try --help)" ;;
        esac
    done

    [ -n "$bindir" ] || bindir="${HOME}/.local/bin"

    # Archive comes from the mirror when set, else canonical GitHub. Checksums
    # default to canonical GitHub so the integrity root is independent of the
    # mirror; strip any trailing slash so URL joins stay clean.
    archive_base="${release_base:-$CANONICAL_BASE_URL}"
    archive_base="${archive_base%/}"
    checksum_base="${checksum_base:-$CANONICAL_BASE_URL}"
    checksum_base="${checksum_base%/}"

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
    # The release workflow publishes the Sigstore bundle beside the archive.
    bundlefile="${BIN}-${version}-${TARGET}.sigstore.json"
    archive_url="${archive_base}/${version}/${archive}"
    bundle_url="${archive_base}/${version}/${bundlefile}"
    sum_url="${checksum_base}/${version}/${sumfile}"

    # The checksum root is an independent trust root only when the archive did
    # not come from a mirror, or the checksum lives somewhere other than that
    # mirror. When it isn't, the Sigstore bundle (whose signed digest a mirror
    # can't forge) is the only trustworthy root; verify_archive refuses a
    # mirror-origin checksum rather than trust the mirror to vouch for itself.
    if [ -z "$release_base" ] || [ "$checksum_base" != "$archive_base" ]; then
        sum_trusted="yes"
    else
        sum_trusted="no"
    fi
    [ -z "$release_base" ] || say "archive source: ${archive_base} (mirror)"

    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t llmlint)" \
        || err "could not create a temporary directory"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "downloading ${archive} (${version})..."
    download "${archive_url}" "${tmp}/${archive}" \
        || err "download failed: ${archive_url}"

    verify_archive "${tmp}/${archive}" "${bundle_url}" "${sum_url}" "${sum_trusted}"

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
