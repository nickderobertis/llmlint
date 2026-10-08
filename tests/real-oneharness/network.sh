#!/usr/bin/env bash
# The real-oneharness suite (the project's `network` target): install the
# released oneharness at the justfile's pin (install-oneharness.sh prints its
# path), then run the #[ignore]-d tests against it. Needs PyPI the first time.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.." || {
  echo "network.sh: cannot enter the repository root; run it from an intact checkout whose directories you can read and enter." >&2
  exit 1
}
bin="$(bash tests/real-oneharness/install-oneharness.sh)"
LLMLINT_REAL_ONEHARNESS="$bin" exec cargo nextest run -p llmlint-real-oneharness --locked --run-ignored only
