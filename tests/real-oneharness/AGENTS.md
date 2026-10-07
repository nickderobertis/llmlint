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

`just test-oneharness` runs this project's `network` target (`network.sh`:
`install-oneharness.sh` puts `oneharness-cli` at the pin in a venv under `.dev/`
and prints its path, then the `#[ignore]`-d tests run with it as
`LLMLINT_REAL_ONEHARNESS`). Free and model-free but networked, so it is outside
the gate tiers; `the_tier_pins_the_multi_file_floor` is this project's `test`
target, in them.
