# AGENTS.md

Durable instructions for humans and agents in this repo. Write for a future
maintainer, not as a session log. Put deterministic steps in scripts; keep this
file for constraints, tradeoffs, and judgment.

> `CLAUDE.md` is a symlink to this file (`ln -s AGENTS.md CLAUDE.md`). Edit
> `AGENTS.md` only; the two must never drift.

## What this repo is

`llmlint` is a Rust CLI that uses an **LLM as a judge** to enforce code-quality
checks deterministic linters can't express — architectural-pattern adherence,
coding-style intent, org-objective alignment. It is **additive** to deterministic
linters (use those wherever a check *can* be deterministic), never a replacement.
A YAML config declares rules, agents, file globs, and a prompt template; llmlint
drives real coding harnesses **through `oneharness`** and reads its validated
structured output. Consumers: developers and CI gating a repo's quality.

## Stack and composition

Built with the `create-repo` skill from one **product shape** (CLI), one
**language** (Rust), and the **CI** + **releasing** cross-cutting references —
pulling `shapes/cli.md` + `languages/rust.md` + `intersections/rust-cli.md`.
Deliberately excluded (so it isn't re-litigated):

- **No monorepo** — single binary crate; no Nx/affected wiring.
- **No `cargo-dist`** — `release.yml`'s native build matrix already ships
  checksummed cross-platform binaries; release-plz handles versioning.
- **crates.io publish** — alongside GitHub Releases + `install.sh` +
  `cargo install --git`, the `publish-crate` job in `release.yml` runs
  `cargo publish` whenever the `CARGO_REGISTRY_TOKEN` secret is set (a `guard`
  job exposes its presence as an output, since `secrets` can't be read in a job
  `if:`). release-plz never publishes (`publish = false` in `release-plz.toml`),
  so versioning/tagging stays decoupled from the registry push. `Cargo.toml`'s
  `include` keeps the published crate to sources + manifest + readme/license +
  `assets/`.
- **No heavy pre-commit framework, direnv, or `src`-layout shuffling** — the gate
  is `just check` + CI on the standard Cargo layout.
- **Coverage bar: 95% lines** (`cargo llvm-cov --fail-under-lines 95`).
- **MSRV (`rust-version`) is advisory** — `just msrv` checks it locally; not a CI
  gate (no strong downstream promise for a binary-only tool yet).

## Command surface

Use the `just` recipes; do not hand-roll equivalents.

- `just setup` — one command to provision a **bare machine** from a fresh clone:
  rustup + the pinned toolchain, `just` itself, the cargo dev tools
  (`cargo-nextest`, `cargo-llvm-cov`), then `just bootstrap`. Idempotent and
  stamped (`.dev/setup.stamp`). On a machine with no `just` yet, run the script
  directly: `./scripts/setup.sh`. The Claude Code **SessionStart hook**
  (`scripts/session-setup.sh`, wired in `.claude/settings.json`) runs the fast
  `setup-check` and, when the environment is not ready, launches `just setup`
  **detached in the background** — it never blocks the session on the
  multi-minute install; tools appear within a few minutes (verify with
  `just setup-check`). Set `LLMLINT_NO_AUTO_SETUP=1` to only *advise* instead, or
  `LLMLINT_SKIP_SETUP=1` to do nothing.
- `just setup-check` — fast, install-free readiness check (no network); exit 0
  when ready, exit 1 with the reason and the fix. Source of truth for "ready" is
  `scripts/setup-lib.sh` (`REQUIRED_BINS` + a fingerprint of the toolchain/tool
  pins); bump those pins and the stamp invalidates so `setup` re-runs.
- `just bootstrap` — the cargo-level step `setup` finishes with (toolchain
  components + `cargo fetch`); CI calls it directly after installing the
  toolchain + tools its own way. Use `just setup` for a bare machine.
- `just check` — full gate: fmt-check, clippy (`-D warnings`), tests, **e2e**,
  `cargo doc`. Must pass before any commit or PR.
- `just test` / `just test-e2e` / `just lint` / `just format` — individual steps.
- `just upgrade` — update dependencies, then re-run `just check`.
- `just deps-check` — `cargo deny` + `cargo machete` (separate; needs network).
- `just lint-live` — opt-in, ad-hoc live run against real oneharness + a real
  harness (`cargo run -- …`); never in the gate or CI.
- `just live-claude` — the **live e2e tier**: builds a release binary, then drives
  the real `llmlint` → real `oneharness` → the real claude-code harness through
  `scripts/live-claude.sh`, asserting a clean file passes (exit 0) and a planted
  `TODO` is flagged (exit 1). It runs on PRs in its own workflow
  (`.github/workflows/live.yml`) across **Linux, macOS, and Windows** — the point
  is to prove the built binary + oneharness + a real harness work on each OS.
  Harness *breadth* is oneharness's test surface (every harness is the same
  `--harness <id>` to llmlint), so one canonical harness is enough. The harness
  CLI + auth are configured in CI, so a missing CLI, auth, or oneharness — or any
  failure to complete the run — is a **hard failure** (red build); the tier never
  skips. Auth + the `CLAUDE_E2E_MODEL` override are documented in `tests/AGENTS.md`.
  Makes real (paid) model calls — out of `check`.
- `just win-color` — the **Windows color-rendering gate**: builds the release
  binary + mock oneharness and runs `scripts/win-console-color.ps1`, which drives
  llmlint against the mock-oneharness fixture with `--color always` into a real
  Windows console screen buffer, then reads the buffer back and asserts the
  `FAIL`/`PASS` labels carry the red/green console attributes (and no raw ESC
  survives). The hermetic e2e + screenshots only prove ANSI is *emitted*
  (platform-independent); this proves a Windows console *renders* it — the thing
  anstream's `AutoStream` exists to guarantee (enable VT, else translate to Win32
  console calls). Windows-only, no model/cost; CI runs it on `windows-latest`
  (`.github/workflows/win-color.yml`) as a real gate, separate from the paid live
  tier. See `tests/AGENTS.md`.
- **Performance suite** (`just bench`, `bench-cli`, `bench-allocs`,
  `bench-instructions`, `bench-compare`, `profile`) — *informational, never a
  gate*. See `benches/AGENTS.md`. The Criterion + allocation benches measure the
  pure engine (`benches/`); `scripts/bench.sh` (hyperfine) and
  `scripts/bench-instructions.sh` (cachegrind) measure the real binary end to end
  against the **mock-oneharness fixture**, so there's no model/network cost — just
  llmlint's own work plus one child spawn. The `Performance` workflow
  (`.github/workflows/bench.yml`) runs all of this on each PR and posts a sticky
  comment + job summary with a base-vs-PR delta; timings are noisy on shared
  runners, so it reports rather than blocks. The bench/profile tools (hyperfine,
  critcmp, samply) are *not* installed by `just setup` — `just bench-tools`
  installs them on demand; CI installs them via `taiki-e/install-action`.
- **Terminal screenshots** (`just screenshots`, `screenshots-tools`,
  `screenshots-bless`) — *informational, never a gate*. See `screenshots/AGENTS.md`.
  `scripts/screenshots.sh` drives the real binary against the **mock-oneharness
  fixture** (`screenshots/fixture/`) — one scene per command (`lint`, with a
  `view` toggle over `default`/`-v` `verbose`/`-v` `debug` (the stderr oneharness
  debug view), plus `init`, `config`, `doctor`) — and renders the real output to
  **deterministic SVGs** via `freeze` + a vendored, pinned font, all at one fixed
  width (`--width`/`--wrap`) so on-page text size is uniform (the `default`/
  `verbose` lint views are colorized via `--color always`; the rest are plain
  text) — byte-identical on every machine (no container), so [screencomp](https://github.com/nickderobertis/screencomp)
  can hash-gate them. The `Visual docs` workflow (`.github/workflows/visual-docs.yml`,
  screencomp's reusable workflow) classifies against the committed baseline
  (`shots/baseline/<arch>.json`), publishes a GitHub Pages gallery, and posts a
  sticky before/after PR comment; `fail-on-drift` makes unexpected drift a red
  build. The local pre-push guard (`.githooks/pre-push`) regenerates the baseline
  on drift. `freeze` is *not* installed by `just setup` — `just screenshots-tools`
  installs the pinned version; screencomp is installed separately (CI installs
  both). Keep the three `freeze` version pins in sync (justfile, `visual-docs.yml`,
  `screenshots-tools`).

## How llmlint drives oneharness

llmlint shells out to `oneharness run` once per `(agent, judge, batch)`, passing
the rendered template via `--system`, a generated JSON Schema via `--schema`
(oneharness validates it and re-prompts on failure), and reading the per-result
`structured` value. **oneharness is a runtime prerequisite** — found on PATH,
overridable via `--oneharness-bin` / `LLMLINT_ONEHARNESS_BIN` / config;
`llmlint doctor` checks it. The harness reads target files on-demand with its own
tools.

- **Read-only mode + minimum version:** llmlint is a judge, never an editor, so
  every `run` passes `--mode read-only` — the harness may read target files but
  can't edit them or run commands. That mode requires **oneharness >= 0.3.0**
  (`oneharness::MIN_VERSION`); both `lint` (pre-flight, once per run) and `doctor`
  parse `oneharness --version` and fail with a clear exit-2 error when the binary
  is older (or its version can't be parsed) rather than letting `--mode read-only`
  blow up mid-run. Bump `MIN_VERSION` in `src/io/oneharness.rs` (and the mock's
  default in `tests/support/mock_oneharness.rs`) together when the floor moves.

- **Verdict polarity (convention):** rules are authored as positive invariants.
  `holds=true` = property holds (pass); `holds=false` = **violation** (fail).
  llmlint exits non-zero when any rule's final verdict is `false`.
- **Relevance (convention):** a rule's `relevance` declares when it should be
  evaluated — `true` (default, always evaluate; the judge may not opt out),
  `false` (never; reported not relevant with no judge call), or a natural-language
  condition the judge decides *before* the verdict. A conditional rule's schema
  inserts a `relevant` boolean before `holds` (gated so `holds` is required only
  when `relevant=true`), so a not-applicable rule is distinguishable from a true
  one instead of every `description` carrying its own "or not applicable" clause.
  A not-relevant outcome is neither pass nor fail — it never fails the build.
- **Ignore directives (convention):** target files may carry inline
  `llmlint: ignore[rule, ...] <reason>` (line-scoped),
  `llmlint: ignore-file[...] <reason>` (file-scoped), or the block-scoped pair
  `llmlint: ignore-block[...] <reason>` … `llmlint: ignore-end[...]` (the close
  names the same rule(s) and carries no reason) comments. llmlint validates only
  their *structure* deterministically — specific configured rule(s) + a reason
  (except `ignore-end`), plus block pairing (every `ignore-block` closes, every
  `ignore-end` matches an open block, no double-open of a rule; blocks track each
  rule independently, so two opened together may close separately and blocks for
  different rules may overlap), else exit 2. The parser is `src/domain/ignore.rs`;
  the file-resolution + scan wiring is shared in `commands/ignores.rs`
  (`io::files::read_text` per target file) and used by both the `lint` pre-flight
  and the standalone, model-free `check-ignores` command (`commands/check_ignores.rs`),
  so the fast static check and the full run can never disagree about what's valid.
  Keep that one shared path — don't reimplement the scan in a command.
  **Honoring** them is the judge's job, specified in the default template; there
  is no separate suppression pass in llmlint, so a custom `prompt_template` must
  carry the same guidance to keep the behavior.
- **oneharness `--config` is single-file** today; llmlint forwards the first
  `--oneharness-config` and warns on extras. *Follow-up:* make oneharness
  `--config` repeatable, then drop the warning.

## Commits, releases, and merging

- **Squash-merge only, via PR, with auto-merge.** Default branch is protected:
  merge/rebase commits disabled, so one PR is one squash commit whose subject is
  the PR title. Queue with `gh pr merge --auto --squash`; merged heads auto-delete.
  Admins may break-glass.
- **All gating checks required**: `check` (full e2e gate), `deny`, `install`, and
  `pr-title`, plus linear history, conversation resolution, no force-push/deletion.
- **PRs follow `.github/pull_request_template.md`** (What / Why; the squash body).
- **Releases**: Conventional Commits drive release-plz (pre-1.0: `feat`→minor,
  `fix`/`perf`→patch, `!`/`BREAKING`→minor; `docs`/`test`/`chore`/`ci`→no release).
  release-plz opens a release PR, auto-merges it on green, tags `vX.Y.Z`, and cuts
  the GitHub Release, which fires `release.yml` to build+attach checksummed
  binaries and, when opted in, `cargo publish` the crate. Needs the
  `RELEASE_PLZ_TOKEN` PAT (a `GITHUB_TOKEN` tag won't retrigger `release.yml`);
  the workflow no-ops until the secret exists. Don't hand-bump the version or
  `CHANGELOG.md`.
- **crates.io publish**: `release.yml`'s `publish-crate` job runs
  `cargo publish --locked` whenever the `CARGO_REGISTRY_TOKEN` secret is set (the
  `guard` job gates it). It is gated on the release `test` job but independent of
  the binary `upload` matrix, so a flaky per-platform upload never blocks the
  immutable crate publish and vice versa. A `verify-crate` job then polls the
  crates.io sparse index for the new version and `cargo install`s + smoke-tests
  it from the registry — a post-publish sanity check (a failure means a broken
  release, not a blocked publish).

## Invariants (non-negotiable)

- The gate is strict: no warnings-only mode. A diagnostic is an error or is
  suppressed with a documented, tracked rationale.
- **Tests are realistic, not mocked, and complete, not minimal** (see below).
- Validate all external / IO inputs (CLI args, config files, subprocess output)
  at the boundary; a bad config is a clear exit-2 error, never a silent skip.
- Keep the artifact portable across Linux, macOS, and Windows.
- Do not commit secrets, credentials, PII, or customer data.

## Architecture

- **`src/domain/` is pure** — config model + validation, template render, schema
  generation, judge/batch planning, vote aggregation, violation model, output
  formatting, exit-code mapping. No process/filesystem/env I/O.
- **`src/io/`** owns all I/O: config discovery + merge + `plugins` resolution
  (local files and remote/versioned URLs, fetched over HTTPS with `ureq`/rustls
  and cached on disk — see `src/io/plugins.rs`), file globbing, the oneharness
  subprocess client, embedded assets. Never hide I/O in a helper that looks pure.
- **`src/commands/`** wires domain + io for `lint` (default), `check-ignores`,
  `init`, `config` (`--sources` adds per-item provenance), `where` (locate one
  config item's source), `doctor`. `commands/ignores.rs` holds the
  ignore-directive resolution + scan shared by `lint` and `check-ignores`.

## Tests are context engineering

This is an agent-driven repo: the test suite is the *only* QA loop. Realism and
coverage are a rule, not a preference.

- **The layer under test is llmlint.** The genuinely-external boundary is the
  `oneharness` subprocess — e2e drives the **real `llmlint` binary** against a
  **mock-oneharness fixture** (feature `mock-oneharness`, `--oneharness-bin`
  override), exactly as oneharness mocks the real agent CLIs. Never mock
  llmlint's own logic (config/render/batch/vote/output).
- **Done means complete, not minimal:** every user journey, happy path *and*
  failure/recovery. The e2e journey list lives in `tests/AGENTS.md` and is the
  source of truth for what's covered; a feature isn't done until its journey lands.
- A live tier (`just live-claude`, plus the ad-hoc `just lint-live`) hits real
  oneharness + a real harness; it is opt-in and out of the `just check` gate. It
  runs on PRs in its own workflow (`.github/workflows/live.yml`) across Linux,
  macOS, and Windows to prove the built binary + oneharness + a real harness work
  on each OS. It expects the harness CLI + auth configured, so a missing
  CLI/auth/oneharness is a **hard failure**, not a skip. The scripted journeys
  live in `scripts/live-claude.sh` + `scripts/live-lib.sh` and are described in
  `tests/AGENTS.md`.

## Scripts and output are context

- Recipes/scripts are quiet on success — a line or nothing. On failure, preserve
  the exact error (paths, rule names, exit codes) and suggest the next action.

## Keeping the allowlist current

The agent command allowlist lives in `.claude/settings.json`; the tool enforces
it. When a new routine command joins the build/test/release workflow, add it
(kept narrow) instead of re-approving it each session.

## After the main task: refine and hand off

After the requested task, propose only materially-helpful follow-ups (scripts,
`AGENTS.md` constraints, shared skills, tests/fixtures), each with its likely
impact. Skip busywork; if nothing helps, say so.
