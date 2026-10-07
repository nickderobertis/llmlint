# tools/coverage/AGENTS.md — the coverage project

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
- `LLMLINT_COVERAGE=off` (the macOS/Windows `cross` jobs) runs the tests with
  plain nextest and skips the report; the floor is the Linux gate's.
- Never lower the floor, or drop a project from `coverage:profiles`, to make the
  number pass: cover the missed lines with a test that drives the real behaviour.
