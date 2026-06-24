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
- `LLMLINT_MOCK_DUMP_ARGS=<file>` — record the raw `run` arg vector, to assert
  which flags llmlint passed (e.g. `--harness` omitted when an agent leaves it unset).

## Journeys covered

- All rules hold -> exit 0; a violation -> exit 1 with `file:line: message`.
- Multi-judge majority: a single dissent still passes; a majority dissent fails.
- `include` merges rules from another file; the bundled `llmlint:config-lint`
  plugin catches a bad rule in a config file.
- include/exclude globbing selects the right files; explicit CLI files override
  the config globs; per-rule and per-agent `files` override the global globs.
- `--config` replaces upward discovery and is repeatable (first entry supplies
  the top-level scalars, the rest contribute rules/agents); `config --config`
  honors a relative path resolved against `--cwd`.
- `--cwd` drives both config discovery and the directory forwarded to oneharness
  as its `--cwd`.
- `--rule` and `--agent` filters limit which rules run; an empty selection exits
  0; rules with no matching files are skipped.
- `--timeout` is forwarded to oneharness; the oneharness `model` is forwarded,
  with a per-agent `model` overriding the global default; multiple oneharness
  configs warn and use the first; `--oneharness-bin` resolves from the env.
- An agent's `harness` is forwarded as `--harness`; leaving it unset omits the
  flag so oneharness falls back to its own configured default harness.
- `init` scaffolds a config (and `--with-template`, `--output`, `--global` via
  XDG or the HOME fallback), refuses to clobber without `--force`; `init` then
  self-lint is clean.
- `config` prints the merged config + sources and rejects an invalid config;
  `doctor` reports the oneharness version and fails clearly when it is missing.
- Failure/recovery: missing config, malformed config, duplicate rule names (exit
  2); schema-invalid, missing-structured, unparseable, empty-results, and
  bad-verdict-shape oneharness output are surfaced (exit 2).

## Live tier (`scripts/live-*.sh`)

The hermetic e2e suite above proves llmlint's logic against a mock oneharness. The
**live tier** proves the *real* stack — the built `llmlint` binary → real
`oneharness` → a real, authenticated harness. It is opt-in (`just live-claude`),
makes real (paid) model calls, and is out of the `just check` gate — it runs on
PRs in its own workflow (`.github/workflows/live.yml`), not as part of `check`.

- **What it covers that the hermetic suite can't:** the built binary + the
  oneharness subprocess + a real harness round-trip, on **Linux, macOS, and
  Windows** (the workflow's OS matrix). That cross-OS proof is the point. Harness
  *breadth* (codex, cursor, …) is **oneharness's** test surface, not llmlint's —
  from llmlint's side every harness is the same `--harness <id>` forwarded to
  oneharness, so one canonical harness (claude-code) is enough here.
- **Fail on broken setup; skip only an unlaunchable harness.** A missing harness
  CLI, missing auth, or missing oneharness is a **hard failure** (red build) — a
  silent skip would let a broken setup pass unnoticed. The one exception, matching
  oneharness's own e2e: when oneharness reports it cannot *launch* the harness on
  this platform (a `spawn-error`/`skipped` status — e.g. on **Windows** it can't
  spawn the npm `claude.cmd` shim, Rust's batch-spawn guard), the run **skips**,
  because the built llmlint + oneharness still ran and there's simply no model
  round-trip to verify there. A harness that *did* run but returned bad output
  (schema-invalid, unparseable) is still a hard failure, not a skip.
- **Journeys** (`live_run_journeys` in `scripts/live-lib.sh`): scaffold a throwaway
  project with one crisp invariant (`no_todo_comments`) pinned to the harness, then
  (1) a clean `src/lib.rs` must pass → exit 0, rule `pass`; (2) a file with a
  planted `TODO` must be flagged → exit 1, rule `fail`. Exit 2 (the live stack
  could not complete) is also a failure.
- **Harness CLI + auth** (required; absent → fail): `claude-code` needs the
  `claude` CLI and `CLAUDE_CODE_OAUTH_TOKEN` (or `ANTHROPIC_API_KEY`). To drive a
  different harness ad hoc, call `live_run_journeys <id>` with that harness's CLI
  installed and authed (`scripts/live-lib.sh` is harness-agnostic).
- **Overrides:** `CLAUDE_E2E_MODEL` picks the judge model (defaults to `haiku`);
  `LL_TIMEOUT` (default 120s) becomes the config's `oneharness.timeout`;
  `LLMLINT_BIN` / `LLMLINT_ONEHARNESS_BIN` override binary resolution.

## Unit vs e2e

Pure domain logic (validation, planning, voting, schema, rendering, reporting)
and the oneharness client's process handling are unit-tested in-module. The
`#[cfg(unix)]` subprocess timeout/capture tests run on Linux/macOS; the coverage
threshold is therefore enforced on Linux CI (see `AGENTS.md`).
