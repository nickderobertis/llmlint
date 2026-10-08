# tools/coverage/AGENTS.md — the coverage-driver and coverage projects

Two projects. `coverage-driver` (this directory) owns `coverage.sh` and its
self-tests; `coverage` (`gate/`) owns the gate's targets and depends on every
`coverage:profiles` project plus the driver. They are split so that the edge a
product edit must follow to re-run the gate does not also select the driver's
slow self-tests: only an edit here does.

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
