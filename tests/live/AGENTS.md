# tests/live/AGENTS.md

<!-- llmlint: ignore-block[agents_md_durable_and_terse] the tier's journey list is the only reference for what this out-of-gate tier proves, since no gate run shows it, so it is kept complete rather than terse -->
## Live tier (`live-*.sh`)

The hermetic e2e suite (`tests/e2e/`) proves llmlint's logic against a mock oneharness. The
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
- **Journeys** (`live_run_journeys` in `tests/live/live-lib.sh`): scaffold a throwaway
  project with one crisp invariant (`no_todo_comments`) pinned to the harness, then
  (1) a clean `src/lib.rs` must pass → exit 0, rule `pass`; (2) a file with a
  planted `TODO` must be flagged → exit 1, rule `fail`; (3) a **fallback** journey
  (issue #146): a project that pins *no* harness plus a `oneharness.toml` with
  `run_mode = "fallback"` and an **absent primary** (`codex`, never installed on the
  runner) ahead of the canonical harness — oneharness skips the primary and runs the
  canonical one, naming it in `fallback.ran` while the skipped primary is
  `results[0]`. A clean file must still pass → exit 0. This is the real-stack
  regression guard: the pre-fix llmlint read the skipped `results[0]` and errored
  the run, so it validates llmlint consumes the *real* oneharness fallback JSON
  shape the hermetic mock only approximates. Exit 2 (the live stack could not
  complete) is also a failure.
- **Harness CLI + auth** (required; absent → fail): `claude-code` needs the
  `claude` CLI and `CLAUDE_CODE_OAUTH_TOKEN` (or `ANTHROPIC_API_KEY`). To drive a
  different harness ad hoc, call `live_run_journeys <id>` with that harness's CLI
  installed and authed (`tests/live/live-lib.sh` is harness-agnostic).
- **Overrides:** `CLAUDE_E2E_MODEL` picks the judge model (defaults to `haiku`);
  `LL_TIMEOUT` (default 120s) becomes the config's `oneharness.timeout`;
  `LLMLINT_BIN` / `LLMLINT_ONEHARNESS_BIN` override binary resolution.
<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->

## Running

`just live-claude` runs this project's `live` target, which builds the release
binary before `live-claude.sh` (the ad-hoc `just lint-live` drives the same stack
by hand).
