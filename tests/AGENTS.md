# tests/AGENTS.md

The e2e suite (`tests/e2e/`) is the source of truth for what llmlint does. It
drives the **real `llmlint` binary** (via `assert_cmd`) against the deterministic
`llmlint-mock-oneharness` fixture (passed with `--oneharness-bin`), which stands
in for the one genuinely-external boundary. **Never mock llmlint's own logic**
(config load/merge/include, file globbing, template render, batching, voting,
reporting). Add a journey here when a user-facing behavior lands.

## Fixture control (env vars read by the mock)

- `LLMLINT_MOCK_VERDICTS=<path>` — JSON map `rule -> spec`; a spec is a bool
  (`holds`), an object (`{holds, violations}`), or an array of specs (one per
  judge call).
- `LLMLINT_MOCK_STATE=<dir>` — per-rule call counter backing array specs; use
  `--max-parallel 1` so the sequence is deterministic.
- `LLMLINT_MOCK_DUMP=<file>` — record the rendered `--system` prompt, to assert
  which files/rules reached the judge (globbing + template render).
- `LLMLINT_MOCK_FAIL_SCHEMA` / `LLMLINT_MOCK_NO_STRUCTURED` / `LLMLINT_MOCK_GARBAGE`
  — force oneharness failure shapes.

## Journeys covered

- All rules hold -> exit 0; a violation -> exit 1 with `file:line: message`.
- Multi-judge majority: a single dissent still passes; a majority dissent fails.
- `include` merges rules from another file; the bundled `llmlint:config-lint`
  plugin catches a bad rule in a config file.
- include/exclude globbing selects the right files; explicit CLI files override
  the config globs.
- `--rule` filter limits which rules run; rules with no matching files are skipped.
- `init` scaffolds a config (and `--with-template`), refuses to clobber without
  `--force`; `init` then self-lint is clean.
- `config` prints the merged config + sources; `doctor` reports the oneharness
  version and fails clearly when it is missing.
- Failure/recovery: missing config, malformed config, duplicate rule names (exit
  2); schema-invalid, missing-structured, and unparseable oneharness output are
  surfaced (exit 2).

## Unit vs e2e

Pure domain logic (validation, planning, voting, schema, rendering, reporting)
and the oneharness client's process handling are unit-tested in-module. The
`#[cfg(unix)]` subprocess timeout/capture tests run on Linux/macOS; the coverage
threshold is therefore enforced on Linux CI (see `AGENTS.md`).
