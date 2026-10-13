#!/usr/bin/env bash
# Each maturin build compiles once; archives reuse that wheel's compiled binary.
set -euo pipefail
out="${1:?output directory required}"
mkdir -p "$out"
for target in x86_64-apple-darwin aarch64-apple-darwin; do
  maturin build --release --locked --target "$target" --out "$out"
  stage="$(mktemp -d)"
  cp "target/$target/release/llmlint" README.md LICENSE CHANGELOG.md "$stage/"
  archive="$out/llmlint-${GITHUB_REF_NAME:?release tag required}-$target.tar.gz"
  tar -czf "$archive" -C "$stage" llmlint README.md LICENSE CHANGELOG.md
  shasum -a 256 "$archive" >"$archive.sha256"
  if [ "$target" = x86_64-apple-darwin ]; then
    arch -x86_64 "$stage/llmlint" --version
    arch -x86_64 "$stage/llmlint" --help >/dev/null
  else
    "$stage/llmlint" --version
    "$stage/llmlint" --help >/dev/null
  fi
  rm -rf "$stage"
done
