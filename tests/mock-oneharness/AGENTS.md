# tests/mock-oneharness/AGENTS.md

The `llmlint-mock-oneharness` fixture: the deterministic oneharness wire double
the e2e journeys (and the screenshots, benchmarks and Windows color check) put
behind `--oneharness-bin`. It stands in for the one genuinely-external boundary
only; it never reimplements llmlint's own logic.

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
- `LLMLINT_MOCK_DUMP_HISTORY_LABELS=<dir>` — record the history-label environment
  seen by `--version` and `run` separately, to assert session-only label injection.
- `LLMLINT_MOCK_DUMP_SCHEMA=<file>` — copy the generated `--schema` JSON, to
  assert its shape (e.g. each rule's `name`/`rationale`/`holds` ordering).
- `LLMLINT_MOCK_RUNLOG=<dir>` — one file per invocation listing the rules it
  judged, to count oneharness calls and assert how rules were batched.
- `LLMLINT_MOCK_SPAWNLOG=<dir>` — one file per **process spawn** (the arg vector),
  written before any subcommand branching so it covers the `--version` pre-flight
  too. `RUNLOG`/`DUMP` only prove no *judge* ran; an empty spawn log proves the
  binary was never executed — what a guard that must cost nothing needs to show.
  Pair it with a resolving control invocation, so an empty log can't mean "never
  wired".
- `LLMLINT_MOCK_BARRIER=<dir>` (+ `_N`, `_MS`) — a rendezvous that releases only
  when `N` invocations are present at once, to prove `--max-parallel` overlapped
  them (a serial wave times out instead).
