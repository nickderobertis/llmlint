# tools/coverage/AGENTS.md — the coverage-driver, coverage and shell-coverage projects

Three projects. `coverage-driver` (this directory) owns both drivers —
`coverage.sh` (Rust, cargo-llvm-cov) and `shcov.sh`/`shcov.rb` (shell, bashcov) —
and their self-tests; `coverage` (`gate/`) owns the Rust gate's targets and
depends on every `coverage:profiles` project plus the driver; `shell-coverage`
(`shell/`) owns the shell gate and depends on every `coverage:shell` project plus
the driver. The Rust gate is split from the driver so that the edge a product
edit must follow to re-run it does not also select the driver's self-tests. The
shell gate merges the driver's own record (its tests run both drivers' scripts),
so its `dependsOn` runs those self-tests — a few seconds over scratch workspaces.

The repo-level coverage gate (`coverage.sh`): `coverage-clear` empties
`target/llvm-cov-target`, each coverage-measured project's `test` target (tag
`coverage:profiles`: the `llmlint` crate and `llmlint-e2e`) runs
`coverage.sh test <crate>` (`cargo llvm-cov --no-report nextest -p <crate>`), and
`coverage` merges every profile and enforces the 95% line floor once, over the
`llmlint` crate's `src/` — the code the single-crate gate measured. The e2e
journeys count toward it because `coverage.sh` builds instrumented copies of the
binary and the fixture they drive.

- The test and coverage targets are uncached: cargo-llvm-cov names every profile
  after the workspace, not the crate, so no replay could restore one crate's
  profiles alone, and a report over a partial set would mean nothing.
- `LLMLINT_COVERAGE=off` (`just check-portable`, which the macOS/Windows `cross`
  jobs run) runs the tests with plain nextest and skips the report; the floor is
  the Linux gate's. `check-portable` excludes `coverage-driver`: its self-tests
  drive cargo-llvm-cov, which the cross jobs do not install.
- `tests/coverage.test.mjs` (`coverage-driver`'s `test`) drives `coverage.sh`
  against a scratch Cargo workspace under the real cargo-llvm-cov.
- Never lower the floor, or drop a project from `coverage:profiles`, to make the
  number pass: cover the missed lines with a test that drives the real behaviour.

## Shell coverage (`shcov.sh`, `shcov.rb`)

- Each `coverage:shell` project's `test` is `shcov.sh run <project> -- <its test
  command>`. That writes `target/shcov/<project>.json`, a declared output, so a
  cached test restores its record. `shell-coverage:coverage` is `shcov.sh report`
  over exactly those projects (a `tools/tests/gate.test.mjs` journey holds the
  list to the tag) and enforces `MIN_LINES` over every script `tools/shell/shell.sh
  files` finds. A project whose test drives scripts gets the tag and joins that
  list.
- How a deep child is measured: bashcov's own runner passes one pipe descriptor,
  which a parent that closes inherited descriptors drops (Python's `subprocess`
  does, by default), and that leaves
  the child tracing onto the stderr the journeys assert on. So `BASH_ENV` points
  every bash at a snippet that opens a trace file of its own, and bashcov's
  `Xtrace` parses the files after the run. A copy is credited only when it is
  byte-identical to the script, recorded while the copy still exists. A stand-in
  that shares the name never counts.
- `LLMLINT_COVERAGE=off` runs the command unmeasured and skips `install` and
  `report`, as it does for `coverage.sh`.
[//]: # "llmlint: ignore-block[agents_md_durable_and_terse] the manager who approved the 50% shell floor required this measurement and its per-script table to be recorded in AGENTS.md beside the floor, as create-repo's bash reference requires the reason for a lowered shell bar to be: it is the evidence the lowered floor rests on, not a report the driver regenerates, and it moves only with the floor"
- **The measurement the 50% floor rests on**, re-taken on the finished tree of
  the change that set it (aarch64 Linux, all 9 projects, `bash scripts/nx run
  shell-coverage:coverage`): 51.85%, 1064/2052.

  | script | lines | % |
  |---|---|---|
  | `.githooks/pre-push` | 66/89 | 74.2 |
  | `.github/scripts/ci-gate.sh` | 101/123 | 82.1 |
  | `benches/bench-instructions.sh` | 16/92 | 17.4 |
  | `benches/bench.sh` | 21/77 | 27.3 |
  | `benches/profile.sh` | 26/82 | 31.7 |
  | `screenshots/bless-baseline.sh` | 10/23 | 43.5 |
  | `screenshots/ci-install-freeze.sh` | 70/90 | 77.8 |
  | `screenshots/host-arch.sh` | 9/11 | 81.8 |
  | `screenshots/screenshots.sh` | 47/172 | 27.3 |
  | `scripts/bun.sh` | 81/94 | 86.2 |
  | `scripts/install-actionlint.sh` | 95/98 | 96.9 |
  | `scripts/install.sh` | 0/205 | 0.0 |
  | `scripts/lint-workflows.sh` | 31/31 | 100.0 |
  | `scripts/nx` | 17/29 | 58.6 |
  | `scripts/nx-base.sh` | 16/18 | 88.9 |
  | `scripts/nx-tier.sh` | 8/10 | 80.0 |
  | `scripts/release-probe.sh` | 57/61 | 93.4 |
  | `scripts/session-setup.sh` | 0/46 | 0.0 |
  | `scripts/setup-check.sh` | 9/14 | 64.3 |
  | `scripts/setup-lib.sh` | 43/51 | 84.3 |
  | `scripts/setup-llmlint.sh` | 0/23 | 0.0 |
  | `scripts/setup.sh` | 0/65 | 0.0 |
  | `tests/live/live-claude.sh` | 0/12 | 0.0 |
  | `tests/live/live-lib.sh` | 84/180 | 46.7 |
  | `tests/real-oneharness/install-oneharness.sh` | 9/47 | 19.1 |
  | `tests/real-oneharness/network.sh` | 0/6 | 0.0 |
  | `tools/coverage/coverage.sh` | 51/77 | 66.2 |
  | `tools/coverage/shcov.sh` | 45/57 | 78.9 |
  | `tools/shell/install-shell-tools.sh` | 79/91 | 86.8 |
  | `tools/shell/shell.sh` | 73/78 | 93.6 |

  Re-take this table whenever the floor moves.
[//]: # "llmlint: ignore-end[agents_md_durable_and_terse]"

