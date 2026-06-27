# tests/AGENTS.md

The e2e suite (`tests/e2e/`) is the source of truth for what llmlint does. It
drives the **real `llmlint` binary** (via `assert_cmd`) against the deterministic
`llmlint-mock-oneharness` fixture (passed with `--oneharness-bin`), which stands
in for the one genuinely-external boundary. **Never mock llmlint's own logic**
(config load/merge/include, file globbing, template render, batching, voting,
reporting). Add a journey here when a user-facing behavior lands.

## Fixture control (env vars read by the mock)

- `LLMLINT_MOCK_VERDICTS=<path>` — JSON map `rule -> spec`; a spec is a bool
  (`holds`), an object (`{holds, violations}`, optionally `{relevant, rationale}`
  for a relevance-gated rule), or an array of specs (one per judge call).
- `LLMLINT_MOCK_STATE=<dir>` — per-rule call counter backing array specs; use
  `--max-parallel 1` so the sequence is deterministic.
- `LLMLINT_MOCK_DUMP=<file>` — record the rendered `--system` prompt, to assert
  which files/rules reached the judge (globbing + template render).
- `LLMLINT_MOCK_FAIL_SCHEMA` / `LLMLINT_MOCK_NO_STRUCTURED` / `LLMLINT_MOCK_GARBAGE`
  — force oneharness failure shapes.
- `LLMLINT_MOCK_DUMP_ARGS=<file>` — record the raw `run` arg vector, to assert
  which flags llmlint passed (e.g. `--harness` omitted when an agent leaves it unset).
- `LLMLINT_MOCK_DUMP_SCHEMA=<file>` — copy the generated `--schema` JSON, to
  assert its shape (e.g. each rule's `name`/`rationale`/`holds` ordering).
- `LLMLINT_MOCK_RUNLOG=<dir>` — one file per invocation listing the rules it
  judged, to count oneharness calls and assert how rules were batched.
- `LLMLINT_MOCK_BARRIER=<dir>` (+ `_N`, `_MS`) — a rendezvous that releases only
  when `N` invocations are present at once, to prove `--max-parallel` overlapped
  them (a serial wave times out instead).

Plugin-fetch journeys also set `LLMLINT_CACHE_DIR=<dir>` (an isolated cache),
and the one `http://` journey drives the real built-in HTTPS client (`ureq`)
against a localhost `HttpServer` (with the proxy env cleared). The version/cache
logic is also covered hermetically via `file://` plugins.

## Journeys covered

- All rules hold -> exit 0 (default output is just the `N rules: …` summary);
  a violation -> exit 1.
- Output by default lists failing rules with `file:line: message` plus the
  summary; passed/skipped rules are only counted. `-v` additionally itemizes
  every passed/skipped rule on stdout and prints the oneharness debug view
  (exact command + raw result per judge) to stderr — including when a judge
  errors, so a bad run can be debugged. Verbosity never changes the exit code,
  surfaces operational (exit-2) errors at every level, and leaves `--format
  json` untouched (the debug view stays on stderr; stdout stays pure JSON).
- Multi-judge majority: a single dissent still passes; a majority dissent fails.
- `plugins` merges rules from another file and from a `file://` URL; a pinned
  `http://` URL is fetched once over HTTPS and reused from cache (not refetched),
  while `LLMLINT_PLUGIN_REFRESH` forces a refetch of the same pin;
  a version mismatch, the removed `llmlint:` scheme, and the renamed top-level
  `include` key are each clear exit-2 errors; the bundled config-lint plugin (a
  URL resolved offline from the embedded copy) catches a bad rule in a config.
- A rule with `override: true` extends a same-named plugin rule, inheriting every
  field it leaves unset (including `description`) and replacing only those it
  sets — asserted via the merged `config` dump, and proven to reach the planner
  by a real run where an override bumps `judges` 1 → 3 and three judges execute.
  A duplicate rule name *without* `override`, and an `override` with no base rule
  to extend, are each clear exit-2 errors.
- YAML anchors, `<<` merge keys, and `x-` stash keys resolve end to end: an
  aliased anchor reaches the rendered prompt and a merged field reaches oneharness.
- A custom top-level `prompt_template` drives the prompt, and an agent's
  `prompt_template` is appended for its rules (both asserted via the dumped prompt).
- Rules for one agent share a single oneharness call by default; a per-agent
  `batch_size` splits them into one call per batch. When a split is needed the
  batches are *balanced*, not packed: 4 rules at `batch_size` 3 run as 2+2 (the
  fewest batches that respect the cap, sizes within one), never 3+1.
- `--max-parallel` overlaps judges in a wave (proven via a rendezvous barrier);
  a serial wave fails to rendezvous, the negative control.
- include/exclude globbing selects the right files; explicit CLI files override
  the config globs; per-rule and per-agent `files` override the global globs.
- `--config` replaces upward discovery and is repeatable (first entry supplies
  the top-level scalars, the rest contribute rules/agents); `config --config`
  honors a relative path resolved against `--cwd`.
- `--cwd` drives both config discovery and the directory forwarded to oneharness
  as its `--cwd`.
- `--rule` and `--agent` filters limit which rules run: `--rule` is repeatable
  (selects exactly the named rules) and `--agent default` targets the
  unassigned rules. A valid-but-empty selection (real names that don't
  intersect) exits 0; a `--rule`/`--agent` name that matches nothing in the
  config is a clear exit-2 error listing the available names — even when mixed
  with a valid name — so a typo isn't a silent false green; rules with no
  matching files are skipped.
- `--timeout` is forwarded to oneharness; a config `oneharness.timeout` is
  forwarded when no CLI flag is given; `schema_max_retries` is forwarded as
  `--schema-max-retries`; the oneharness `model` is forwarded, with a per-agent
  `model` overriding the global default; multiple oneharness configs warn and use
  the first; `--oneharness-bin` resolves from the env, and a config
  `oneharness.bin` resolves the binary with no flag or env at all.
- Rationales (on by default): the generated schema requires each rule to emit
  `name` -> `rationale` -> `holds` -> `violations` in that order, with `name`
  pinned to the rule, and the default template renders the terse-rationale
  guidance into the prompt (absent under `--no-rationales`); the human report
  shows a rule's rationale for every failure by default and for every evaluated
  rule at `-v`, and `--format json` carries it (name-first) for every rule.
  `--no-rationales` and config `rationales: false` (with no flag) both drop
  `rationale` from the schema and the report — and llmlint suppresses a rationale
  even when the harness leaks one; a CLI `--rationales` overrides config
  `rationales: false`; a per-rule `rationale` overrides the session default in
  both directions within one batch (opt-in under a disabled session, opt-out
  under an enabled one). For a multi-judge rule, the report and `--format json`
  itemize *each* judge's result (`held`/`violated`) and rationale — at every
  failure and for every evaluated rule at `-v` — so judge disagreement is
  visible, not collapsed to one representative.
- Relevance gating: a rule with a `relevance` condition makes the judge decide
  applicability first — its schema inserts a `relevant` boolean between the
  `rationale` and the verdict, requiring `holds` only via an if/then on
  `relevant == true`, and the default template renders the relevance guidance +
  the per-rule condition (absent for always-evaluated rules). A judge ruling a
  rule not relevant reports it distinctly (a dim `N/A … (not relevant)` line at
  `-v`, its own `not relevant` summary segment, `outcome: "not_relevant"` in
  `--format json`) and exits clean — never conflated with a pass; a relevant
  rule still evaluates its verdict normally. `relevance: false` disables a rule
  deterministically (reported not relevant with no oneharness call at all). For a
  multi-judge conditional rule, relevance is decided by majority first (a majority
  of not-relevant judges skips the verdict, the lone violation never failing the
  build) and otherwise the verdict is tallied over the relevant judges only (the
  held fraction is `held/relevant`, not `held/total`); the per-judge breakdown
  shows each judge's `not relevant`/`held`/`violated`. An empty relevance
  condition is a deterministic config error (exit 2).
- Every top-level setting also has a CLI override that wins over the config:
  `--model`, `--schema-max-retries`, and `--prompt-template` (a file whose
  contents replace the config's template) are each asserted to override their
  config counterparts. Across plugins, the nearest config to the root wins for
  top-level scalars (template, files, oneharness, rationales) and a deeper plugin
  only fills what shallower configs left unset — asserted end to end via
  `llmlint config` on a root -> mid -> leaf chain.
- An agent's `harness` is forwarded as `--harness`; leaving it unset omits the
  flag so oneharness falls back to its own configured default harness.
- Every `run` carries `--mode read-only` (llmlint judges, never edits), asserted
  via the dumped arg vector. The minimum-oneharness-version gate (>= 0.3.0,
  needed for read-only mode) is exercised both ways: `doctor` and `lint` reject a
  too-old oneharness with a clear exit-2 "too old" error (the mock's reported
  version is driven by `LLMLINT_MOCK_VERSION`), a version string with no parseable
  number is a distinct exit-2 "could not determine" error, and `lint`'s gate
  fires *before* any judge runs (no oneharness `run` is recorded).
- Inline `llmlint: ignore[rule, ...] <reason>` (line-scoped),
  `llmlint: ignore-file[...] <reason>` (file-scoped), and the block-scoped pair
  `llmlint: ignore-block[...] <reason>` / `llmlint: ignore-end[...]` (the close
  names the same rule(s) and needs no reason) directives in target files pass
  validation when well-formed (in any comment style; a prose mention of the
  marker is not a directive), and the default prompt documents how a judge should
  honor them. Their *structure* is enforced deterministically: a directive with
  no brackets, an empty rule list, an unknown/invalid rule name, or (where one is
  required) no reason is a clear exit-2 error located as `file:line:` — honoring
  them is the judge's job, llmlint never suppresses anything itself. Block pairing
  is checked deterministically too: an unclosed `ignore-block` (reported at its
  opening line), an `ignore-end` with no matching open block, and re-opening a
  rule already in an open block are all exit-2 errors. Scope and timing: only
  resolved *target* files are scanned (a malformed directive in an excluded file
  is ignored), the known-rule set is the full config (a directive may name a
  configured rule this run didn't `--rule`-select), and every malformed directive
  across files reports in one error *before* any judge runs (no wasted oneharness
  call). The finer parsing variants (invalid rule name, unterminated bracket,
  multiple problems on one directive, block-comment-terminator stripping,
  per-rule block tracking with overlapping/independently-closed blocks,
  binary-file skip) are unit-tested in `domain::ignore` / `io::files`.
- `check-ignores` runs that same structural validation as a **standalone,
  model-free command**: wired with no `--oneharness-bin`, it never spawns a
  harness yet validates well-formed directives (exit 0, "ignore directives OK")
  and rejects malformed ones (exit 2, located `file:line:`, all problems in one
  error) — proving it belongs in the fast static-check loop. It shares `lint`'s
  file resolution: explicit `FILES` scope the scan (a malformed directive in an
  unlisted file isn't caught), the known-rule set is the full config, `--cwd` is
  the discovery + glob root, and an invalid config is a clear exit-2 error before
  any scan.
- `init` scaffolds a config (and `--with-template`, `--output`, `--global` via
  XDG or the HOME fallback), refuses to clobber without `--force`; `init` then
  self-lint is clean. The scaffold leads with a `# yaml-language-server: $schema=…`
  modeline pointing at the published config schema (`assets/llmlint.schema.json`,
  pinned to `domain::config_schema::build()` so it can't drift from the model).
- `config` prints the merged config + sources and rejects an invalid config;
  `doctor` reports the oneharness version and fails clearly when it is missing or
  older than the minimum required for read-only mode.
- `--format json` is a stable machine contract: a passing run lists rule names; a
  failing run carries `summary` counts and located `violations` (exit 1); a
  run-error carries the `errors` array (exit 2).
- Failure/recovery: missing config, malformed config, and each deterministic
  validation error — duplicate rule names, an even `judges` count, an invalid
  rule name, an empty description, an empty relevance condition, `judges: 0`,
  `batch_size: 0`, and a rule referencing an unknown agent (exit 2);
  schema-invalid, missing-structured,
  unparseable, empty-results, and bad-verdict-shape oneharness output are
  surfaced (exit 2).

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
- **Never skips.** A missing harness CLI, missing auth, or missing oneharness — or
  any exit 2 (the stack couldn't complete) — is a **hard failure** (red build). A
  silent skip would let a broken live setup pass unnoticed, so the live tier has no
  skip path at all (matching oneharness's own e2e, which fails rather than skips).
  This runs the full round-trip on Linux, macOS, **and Windows**.
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

## Windows color-rendering tier (`scripts/win-console-color.ps1`)

Color has two separable questions: does llmlint **emit** the right ANSI, and does
a terminal **render** it? The first is platform-independent and already covered —
the hermetic e2e (`color_is_off_when_piped_but_forced_by_color_always`) and the
screenshot tooling both assert the escape bytes. The second is the one that can
actually break on Windows: a legacy console (no virtual-terminal processing)
prints bare ANSI as `<-[31m` garbage. llmlint routes its report through anstream's
`AutoStream` (enable VT, else translate to Win32 console attribute calls) so it
renders; this tier proves that end result.

- **What it covers that nothing else does:** a *real Windows console* interpreting
  llmlint's color. `scripts/win-console-color.ps1` drives the **release binary**
  against the **mock-oneharness fixture** (`screenshots/fixture/`, no model/network/
  cost — deterministic) with `--color always` into a freshly created console screen
  buffer, then reads the buffer back with `ReadConsoleOutput` and asserts the per
  -cell *attributes*: the `FAIL` label is red, `PASS` is green, and no cell holds a
  raw ESC (0x1b). A pre-`AutoStream` build (bare ANSI to a fresh buffer, VT off)
  leaves raw escapes in the cells and fails here.
- **It is a gate, not informational.** A Windows rendering regression is a hard
  failure. Run it with `just win-color`; CI runs it on `windows-latest`
  (`.github/workflows/win-color.yml`). It needs no harness CLI, auth, or
  oneharness — only the binary + the fixture — so unlike the live tier it is free
  and runs on every PR.

## Unit vs e2e

Pure domain logic (validation, planning, voting, schema, rendering, reporting)
and the oneharness client's process handling are unit-tested in-module. The
`#[cfg(unix)]` subprocess timeout/capture tests run on Linux/macOS; the coverage
threshold is therefore enforced on Linux CI (see `AGENTS.md`).
