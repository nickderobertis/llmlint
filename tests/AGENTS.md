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

Plugin-fetch journeys also set `LLMLINT_CACHE_DIR=<dir>` (an isolated cache),
and the one `http://` journey drives the real built-in HTTPS client (`ureq`)
against a localhost `HttpServer` (with the proxy env cleared). The version/cache
logic is also covered hermetically via `file://` plugins.

## Journeys covered

- All rules hold -> exit 0; a violation -> exit 1 with `file:line: message`.
- Multi-judge majority: a single dissent still passes; a majority dissent fails.
- `plugins` merges rules from another file and from a `file://` URL; a pinned
  `http://` URL is fetched once over HTTPS and reused from cache (not refetched);
  a version mismatch, the removed `llmlint:` scheme, and the renamed top-level
  `include` key are each clear exit-2 errors; the bundled config-lint plugin (a
  URL resolved offline from the embedded copy) catches a bad rule in a config.
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
**live tier** proves the *real* stack — real `llmlint` → real `oneharness` → a
real, authenticated harness — and is the llmlint analogue of oneharness's own
`scripts/e2e-*.sh`. It is opt-in (`just live-<harness>` / `just live-all`), makes
real (paid) model calls, and is out of the `just check` gate — it runs on PRs in
its own workflow (`.github/workflows/live-claude.yml`), not as part of `check`.

- **Fail, never skip:** because this tier runs where the harness is configured, a
  missing harness CLI, missing auth, or missing oneharness is a **hard failure**
  (red build), not a skip — a silent skip would let a broken live setup pass
  unnoticed. Run only the `live-<harness>` recipes for the harnesses you have set
  up; `live-all` requires them all.
- **Journeys per harness** (`live_run_journeys` in `scripts/live-lib.sh`): scaffold
  a throwaway project with one crisp invariant (`no_todo_comments`) pinned to that
  harness, then (1) a clean `src/lib.rs` must pass → exit 0, rule `pass`; (2) a
  file with a planted `TODO` must be flagged → exit 1, rule `fail`. Exit 2 (the
  live stack could not complete) is also a failure.
- **Harness CLI + auth** (required; absent → fail), matching oneharness:
  `claude-code`→`claude` + `CLAUDE_CODE_OAUTH_TOKEN`|`ANTHROPIC_API_KEY`;
  `codex`→`codex` + `OPENAI_API_KEY`;
  `opencode`→`opencode` + `ANTHROPIC_API_KEY`|`OPENAI_API_KEY`;
  `goose`→`goose` + `OPENAI_API_KEY`; `qwen`→`qwen` + `OPENAI_API_KEY`;
  `crush`→`crush` + `ANTHROPIC_API_KEY`|`OPENAI_API_KEY`;
  `copilot`→`copilot` + `COPILOT_GITHUB_TOKEN`;
  `cursor`→`cursor-agent` + `CURSOR_API_KEY`.
- **Overrides:** `<HARNESS>_E2E_MODEL` picks the judge model (claude defaults to
  `haiku`; others use the harness default unless set); `LL_TIMEOUT` (default 120s)
  becomes the config's `oneharness.timeout`; `LLMLINT_BIN` /
  `LLMLINT_ONEHARNESS_BIN` override binary resolution.

## Unit vs e2e

Pure domain logic (validation, planning, voting, schema, rendering, reporting)
and the oneharness client's process handling are unit-tested in-module. The
`#[cfg(unix)]` subprocess timeout/capture tests run on Linux/macOS; the coverage
threshold is therefore enforced on Linux CI (see `AGENTS.md`).
