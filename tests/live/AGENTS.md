# tests/live/AGENTS.md

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

## Running

- `just live-claude` — the **live e2e tier**: builds a release binary, then drives
  the real `llmlint` → real `oneharness` → the real claude-code harness through
  `tests/live/live-claude.sh`, asserting a clean file passes (exit 0), a planted
  `TODO` is flagged (exit 1), and a **fallback** run (issue #146 — an absent
  primary harness ahead of the canonical one via a `oneharness.toml`
  `run_mode = "fallback"`) still passes, proving llmlint reads the real
  `fallback.ran` winner and not the skipped `results[0]`. It runs on PRs in its own
  workflow
  (`.github/workflows/live.yml`) across **Linux, macOS, and Windows** — the point
  is to prove the built binary + oneharness + a real harness work on each OS.
  Harness *breadth* is oneharness's test surface (every harness is the same
  `--harness <id>` to llmlint), so one canonical harness is enough. The harness
  CLI + auth are configured in CI, so a missing CLI, auth, or oneharness — or any
  failure to complete the run — is a **hard failure** (red build); the tier never
  skips. Auth + the `CLAUDE_E2E_MODEL` override are documented above.
  Makes real (paid) model calls — out of `check`.
- A live tier (`just live-claude`, plus the ad-hoc `just lint-live`) hits real
  oneharness + a real harness; it is opt-in and out of the `just check` gate. It
  runs on PRs in its own workflow (`.github/workflows/live.yml`) across Linux,
  macOS, and Windows to prove the built binary + oneharness + a real harness work
  on each OS. It expects the harness CLI + auth configured, so a missing
  CLI/auth/oneharness is a **hard failure**, not a skip. The scripted journeys
  live in `tests/live/live-claude.sh` + `tests/live/live-lib.sh` and are described
  above.
