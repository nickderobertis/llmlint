# tests/real-oneharness/AGENTS.md

## Real-oneharness tier (`real_oneharness.rs`)

The hermetic suite doubles oneharness, so it can prove which `--config` files
llmlint forwards and in what order, but not that a later file's settings win.
`just test-oneharness` closes that gap without a model: it installs the released
`oneharness-cli` at the justfile's `oneharness-cli-version` pin — the multi-file
floor, `LAYERED_CONFIG_MIN_VERSION` — runs the real `llmlint` against the mock
(which records the argv), and hands the recorded `--config` list, verbatim and
from the same working directory, to the real `oneharness config --format json`.
Each journey asserts the forwarded list, that oneharness loaded exactly those
files, and that the resolved `mode` is the highest layer's, attributed to it:
the later of two configured files; a repository config over its plugin's; the
nearest of nested llmlint configs (with a path configured at both levels);
`LLMLINT_ONEHARNESS_CONFIG` over the config files; and a `--oneharness-config`
flag over both. The tests are `#[ignore]`-d; run without the recipe's
`LLMLINT_REAL_ONEHARNESS` they fail rather than skip. The one non-ignored test,
`the_tier_pins_the_multi_file_floor`, holds the pin to the constant in every run.

## Running

- `just test-oneharness` — the **real-oneharness tier** (this project's
  `network` target: `real_oneharness.rs`'s `#[ignore]`-d tests, via `network.sh`): installs the released `oneharness-cli` at the
  justfile's `oneharness-cli-version` pin (`tests/real-oneharness/install-oneharness.sh`, a venv
  under `.dev/`; needs PyPI), then feeds the `--config` layers llmlint forwards
  (recorded by the mock) to the real `oneharness config --format json` and asserts
  the highest layer's settings win. Free and model-free, but networked, so outside
  the gate tiers. The pin is the multi-file floor
  (`oneharness::LAYERED_CONFIG_MIN_VERSION`); an always-run test (this project's `test`
  target, in the gate tiers) holds them equal.
