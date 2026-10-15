#!/usr/bin/env bash
# Build each target once through maturin; archive and wheel must contain those exact bytes.
set -euo pipefail
# llmlint: ignore[code_lands_in_the_domain_that_owns_it] Production release builders stay in scripts, their existing owning domain; release-targets owns their artifact contract and on-demand journeys.
fail() {
  echo "release-packages: $*; fix the named input/tool and rerun scripts/release-packages.sh OUT TARGET" >&2
  exit 1
}
wheel_dir='' stage='' work=''
# llmlint: ignore-block[changed_behavior_has_e2e] Real native success and archive-collision recovery exercise this boundary; forcing individual installed-command or cleanup failures would replace the layer under test.
cleanup() {
  rm -rf "${wheel_dir:-}" "${stage:-}" "${work:-}" || {
    echo "release-packages: cleanup failed; remove scratch paths $wheel_dir $stage $work before rerunning" >&2
    return 1
  }
}
# llmlint: ignore-end[changed_behavior_has_e2e]
trap cleanup EXIT
out="${1:?usage: scripts/release-packages.sh OUT TARGET...}"
shift
case "$out" in -*) fail 'output path must not start with an option; use ./PATH or an absolute path' ;; esac
[ "$#" -gt 0 ] || fail 'pass at least one Darwin or Linux Rust target'
mkdir -p "$out" || fail "cannot create output directory $out"
out="$(cd "$out" && pwd)" || fail "cannot resolve output directory $out"
version="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["package"]["version"])')" || fail 'cannot read Cargo.toml package version'
[ "${GITHUB_REF_NAME:?set GITHUB_REF_NAME to the release tag}" = "v$version" ] || fail "tag does not match package version: got $GITHUB_REF_NAME, expected v$version; set GITHUB_REF_NAME=v$version"
for target in "$@"; do
  build_command=(maturin build --release --locked --target "$target")
  case "$target" in
    x86_64-apple-darwin | aarch64-apple-darwin) ;;
    x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) build_command+=(--compatibility linux) ;;
    *) fail "unsupported target $target; use x86_64/aarch64-apple-darwin or x86_64/aarch64-unknown-linux-gnu" ;;
  esac
  wheel_dir="$(mktemp -d)" || fail 'cannot create wheel scratch directory'
  stage="$(mktemp -d)" || fail 'cannot create package staging directory'
  # The backend owns Cargo-to-PEP-440 normalization, including prerelease spelling.
  maturin pep517 write-dist-info --locked --target "$target" --metadata-directory "$stage/metadata" >"$stage/metadata.log" 2>&1 || {
    cat "$stage/metadata.log" >&2
    fail "cannot prepare expected Python metadata for $target"
  }
  python_version="$(
    python3 - "$stage/metadata" <<'PYVERSION'
import email, glob, pathlib, sys
files = glob.glob(sys.argv[1] + '/*.dist-info/METADATA')
if len(files) != 1:
    sys.exit('expected one backend METADATA file')
version = email.message_from_bytes(pathlib.Path(files[0]).read_bytes())['Version']
if not version or any(c.isspace() for c in version):
    sys.exit('backend emitted no single-line Python version')
print(version)
PYVERSION
  )" || fail 'cannot read the backend Python version'
  "${build_command[@]}" --out "$wheel_dir" >"$stage/build.log" 2>&1 || {
    cat "$stage/build.log" >&2
    fail "maturin build failed for $target"
  }
  cp "target/$target/release/llmlint" README.md LICENSE CHANGELOG.md "$stage/" || fail "cannot stage $target binary and documentation"
  archive="$out/llmlint-$GITHUB_REF_NAME-$target.tar.gz"
  tar -czf "$archive" -C "$stage" llmlint README.md LICENSE CHANGELOG.md || fail "cannot pack $archive"
  (
    cd "$out" || exit 1
    shasum -a 256 "$(basename "$archive")" >"llmlint-$GITHUB_REF_NAME-$target.sha256"
  ) || fail "cannot checksum $archive"
  bash scripts/release-verify.sh "$wheel_dir" "$archive" "$stage/llmlint" "$out" "$target" "$version" "$python_version" || fail "artifact validation failed for $target"
  wheel=("$wheel_dir"/*.whl)
  platform="$(basename "${wheel[0]}" .whl)" || fail 'cannot determine staged wheel basename'
  platform="${platform##*-}"
  python3 -m venv "$stage/installer" || fail 'cannot provision isolated installer; install Python with venv/ensurepip support'
  "$stage/installer/bin/python" -m pip install --no-deps --only-binary=:all: --platform "$platform" --target "$stage/wheel-install" "${wheel[0]}" >"$stage/install.log" 2>&1 || {
    cat "$stage/install.log" >&2
    fail "cannot install ${wheel[0]}"
  }
  mkdir "$stage/archive-install" || fail 'cannot create archive installation directory'
  tar -xzf "$archive" -C "$stage/archive-install" || fail "cannot install $archive"
  for binary in "$stage/archive-install/llmlint" "$stage/wheel-install/bin/llmlint"; do
    command=("$binary")
    if [ "$target" = x86_64-apple-darwin ]; then command=(arch -x86_64 "$binary"); fi
    # llmlint: ignore-block[changed_behavior_has_e2e] Real native success and archive-collision recovery exercise this boundary; forcing individual installed-command or cleanup failures would replace the layer under test.
    installed="$("${command[@]}" --version)" || fail "cannot execute $binary; verify target toolchain/Rosetta installation"
    [ "$installed" = "llmlint $version" ] || fail "wrong installed version $installed; expected llmlint $version"
    "${command[@]}" --help >"$stage/help.log" 2>&1 || {
      cat "$stage/help.log" >&2
      fail "--help failed for $binary; inspect the captured diagnostic and rebuild the target"
    }
    work="$(mktemp -d)" || fail 'cannot create smoke workspace'
    (cd "$work" && "${command[@]}" init && "${command[@]}" config) >"$stage/smoke.log" 2>&1 || {
      cat "$stage/smoke.log" >&2
      fail "init/config failed for $binary"
    }
    # llmlint: ignore-end[changed_behavior_has_e2e]
    rm -rf "$work" || fail "cannot remove smoke workspace $work"
    work=
  done
  cp "${wheel[0]}" "$out/" || fail "cannot stage wheel into $out"
  cleanup
  wheel_dir='' stage=''
done
