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
  `cargo install --git` + PyPI binary wheels (see "PyPI wheels" under Commits,
  releases, and merging), the `publish-crate` job in `release.yml` runs
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
  `just setup-check`). It also makes the **released llmlint binary** available
  for self-usage (dogfooding `llmlint lint-config` etc. without waiting for a
  source build): when `llmlint` is not on PATH it pip-installs `llmlint-cli`
  (the prebuilt-binary wheel) into `.dev/llmlint-venv` in the background —
  seconds, works where github.com is blocked but PyPI is not — and symlinks the
  binary onto PATH. Set `LLMLINT_NO_AUTO_SETUP=1` to only *advise* instead, or
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
- `just check-version-bump [base=origin/main]` — dogfood `check-version-bump` on
  llmlint's own versioned plugin (`assets/config_lint.yml`), failing if it changed
  vs the base without a `version:` bump. Out of `check` (it needs a base ref +
  network to resolve it); CI runs it against the PR base.
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
  `screenshots-tools`). The README **hero** is a separate animated GIF of the
  live-progress view (`docs/screenshots/demo.gif`, `just screenshots-gif`,
  `scripts/demo-gif.py`) — same real-binary-against-the-fixture approach, rendered
  to frames with the vendored font (Pillow, no `ttyd`/`ffmpeg`); it is *not*
  hash-gated (a GIF isn't byte-reproducible), so it is regenerated on demand.

## How llmlint drives oneharness

llmlint shells out to `oneharness run` once per `(agent, judge, batch)` (plus a
bounded corrective re-ask — see the scope bullet below), passing the rendered
template via `--system-file` (a temp file, not an inline argv string — the
briefing carries every changed file's inlined diff, so an inline `--system`
would trip the OS `Argument list too long` limit; this is why the floor is
oneharness >= 0.3.12), a generated JSON Schema via `--schema` (oneharness
validates it and re-prompts on failure), and reading the per-result `structured`
value. **oneharness is a runtime prerequisite** — found on PATH, overridable via
`--oneharness-bin` / `LLMLINT_ONEHARNESS_BIN` / config, with a **sibling
fallback**: when nothing is overridden and PATH has no `oneharness`, llmlint
probes for one beside its own executable (`Client::new` in `src/io/oneharness.rs`).
That is how tool-isolating installers (`uv tool install`, `pipx`) lay out the
llmlint-cli wheel and its oneharness-cli dependency — one private venv `bin/`
with only llmlint linked onto PATH — so those installs work with zero flags;
PATH always wins over the sibling so an environment's chosen oneharness is never
shadowed. `llmlint doctor` checks resolution and names the resolved path. The
harness reads target files on-demand with its own tools.

- **Read-only mode + system-by-file + minimum version:** llmlint is a judge,
  never an editor, so every `run` passes `--mode read-only` — the harness may
  read target files but can't edit them or run commands (needs oneharness >=
  0.3.0). It also passes the rendered system prompt by file (`--system-file`, so
  a large briefing never trips the OS argv limit — needs oneharness >= 0.3.12), so
  the floor is **oneharness >= 0.3.12** (`oneharness::MIN_VERSION`). Both `lint`
  (pre-flight, once per run) and `doctor` parse `oneharness --version` and fail
  with a clear exit-2 error when the binary is older (or its version can't be
  parsed) rather than letting a missing flag blow up mid-run. Bump `MIN_VERSION`
  in `src/io/oneharness.rs` (and the mock's default in
  `tests/support/mock_oneharness.rs`) together when the floor moves.

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
- **Line attribution (convention):** a rule's `require_line_attribution: true`
  declares that *every* violation it reports must cite a concrete `file` and
  `line` (off by default, since some findings — e.g. cross-cutting architectural
  drift — genuinely can't be pinned to one source line). Enforcement is layered,
  not a per-violation back-and-forth: the generated schema marks each violation's
  `file`/`line` **required** (so oneharness re-prompts the judge to localize the
  *whole* verdict object in one batched turn), and the default template asks for
  it up front. The deterministic backstop is post-vote in `commands/lint.rs`
  (`domain::attribution::unlocalized_errors`): a *failing* opted-in rule that
  still surfaces a violation without a file+line is one batched exit-2 error
  (listing all of that rule's unlocalized messages), never a silently-imprecise
  pass-through. Wired through `Rule` → `ResolvedRule` → `RuleSpec`/`SchemaRule`
  like `rationale`/`relevance`; inherited/overridable the same way.
- **Per-file scope + wrong-file validation (convention):** a judge call batches
  an agent's rules over the **union** of their files (fewer invocations than one
  call per distinct file set), so different rules apply to different files in the
  same prompt. The rendered template tells the judge, per file, exactly which
  rules apply — listing the apply-set or, when shorter, the skip-set (the
  token-cheaper spelling; see `domain::applicability::per_file`). After the judge
  answers, any violation pinned to a file **outside** that rule's scope (a "wrong
  rule in wrong file") is rejected: llmlint re-asks once (`MAX_REWORKS`) with the
  exact per-file rule lists (`applicability::rework_prompt`). If a wrong-file
  violation survives the rework it is dropped deterministically, and a fail whose
  *entire* basis was out-of-scope flips to a pass — a mislocated finding can never
  redden the build. The cleanup is pure (`applicability::clean_verdict`); the
  matching normalizes paths (`norm`) so a judge's `./src/a.rs` matches `src/a.rs`.
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
  **Honoring** them is now llmlint's own job, deterministic and layered by scope:
  `ignore::suppressions` parses each well-formed directive into per-rule line spans
  (`ignore-file` → whole file; `ignore` → its line and the one below;
  `ignore-block`…`ignore-end` → the spanned lines). A **whole-file `ignore-file`**
  is honored *up front in the planner* (`plan::build` via `PlanContext` +
  `Suppressions::is_file_scoped`): the file is dropped from that rule's **effective
  scope** before the judge runs, so the prompt never carries (nor pays tokens for)
  a file whose every verdict for that rule would be discarded anyway. A rule left
  with no effective file is reported **ignored** (`Outcome::Ignored`, a reasoned
  exemption distinct from an incidental `Skipped`), never judged; a file every
  declaring rule ignores leaves the batch union entirely (surfaced as an *excluded*
  file in the plan explanation). Line/block ignores (which leave judgeable lines)
  stay a *post-vote* drop in `clean_verdict` (flipping a fail to a pass when that
  removes its only basis) — the backstop that also catches any file-scoped
  violation the judge reports despite the exclusion. The default template still
  documents the line/block forms as a backstop (so the judge's verdict reads true)
  but no longer needs the file-scoped guidance — and a custom `prompt_template` can
  drop the ignore guidance entirely without changing behavior, since llmlint
  enforces it.
- **Token-weighted batching + counterfactual (convention):** within the fixed
  batch count `ceil(n / batch_size)`, `plan::build` assigns rules to batches to
  minimize a **lexicographic, token-weighted objective** (`src/domain/cost.rs`):
  (1) tokens *billed* — Σ over batches of the batch's file-token union (each file's
  content is re-billed in every batch it lands in); (2) per-rule *exposure* —
  Σ over rules of their batch's union (each rule is judged against its whole batch's
  files, so a big union shared by many rules is read many times); (3) a balanced-size
  tiebreak. **At a fixed batch count these never trade off** — you can't split a rule
  into its own call to shrink its prompt — so minimizing per-rule exposure is a free
  quality win over the billing-optimal-but-tied layouts (e.g. it parks a wide-scope
  rule in the *smaller* batch so fewer rules read its heavy files). `cost::Model::assign`
  is a **provable minimum** via branch-and-bound within a node budget, falling back to
  a deterministic greedy + local-search heuristic past it; the exhaustive
  `domain::cost` test suite brute-forces the optimum across a broad shape table and
  asserts `assign` achieves it. File weights are estimated tokens (≈ file bytes / 4,
  computed in `commands/lint.rs` from the text it already reads for ignore-scanning;
  a weightless context falls back to unit file counts, which the pure planner tests
  use). The order-based layout is costed too, only to report the `Optimization`
  counterfactual (billed + per-rule saved) in the explanation.
- **Plan explanation + `--plan-only` (convention):** `plan::build` returns, beside
  the runs, a `PlanExplanation` built *while deciding* (so it can never drift): per
  agent → judge index → batch, the batched rule set, the effective file union, the
  files reused across the batch's rules (the grouping's justification), any files
  excluded because every declaring rule `ignore-file`s them, plus the rules left
  unjudged with their reason and the batching counterfactual. It also states the
  **actual lint set** up front: `linted_files` (the distinct union across every
  batch — computed while planning, so it can't drift) drives the header's
  "linting N file(s)", making clear what gets judged without counting across
  batches. Under `--diff` the header also names `diff_excluded_files` — files that
  matched the globs but were dropped as unchanged/deleted vs the base (set by the
  `lint` command after building, since the planner is diff-unaware) — so a smaller
  lint set is explained, not a mystery. It renders as a
  readable tree (`to_human`) and serializes (`Serialize`). At `-v` the `lint`
  command **narrates it up front — before the judges run** (to stdout, then the
  results follow), so a reader sees what will be linted and how it batches, then
  watches it execute, rather than meeting the plan only at the end of the report;
  the human `Report` deliberately does *not* re-render it (no duplication). It is
  still attached to the `Report` (`with_plan`), so `--format json` carries it under
  `plan` and the history record persists it — one source, no drift. `--plan-only`
  prints the explanation and exits before any oneharness call or history write — a
  zero-cost batching-debug view. **Agents are
  the hard isolation boundary:** the planner never batches rules across agents even
  when their harness/model/template are identical and merging would save tokens —
  an agent split is user intent (isolating rules that interfere when judged
  together), asserted in `plan.rs` tests.
- **Diff context + changed-file filter (convention):** `--diff [<backend>]`
  **restricts the run to the changed files** and adds each one's diff to the judge
  prompt so it reviews only the changed lines (bare `--diff` defaults to `git`,
  compared against `HEAD`). The default target set becomes the **intersection of
  the changed files with the configured globs** (and any explicit `FILES`): a file
  with an empty diff vs the base is dropped from planning with no model call, a
  deleted path (a diff but no file on disk) is dropped too, and a rule left with no
  files is skipped — so an empty intersection is a clean, model-free exit 0. This
  is `restrict_to_changed` in `commands/lint.rs`, applied right after the diffs are
  computed (once, at the I/O boundary, over every glob-resolved target) and before
  the ignore/suppression scan and planning, so the whole engine downstream sees
  only the changed set. The capability is
  **backend-agnostic**: `src/io/diff.rs` defines a `DiffProvider` trait and a
  `DiffBackend` value enum; `GitDiff` is the first impl (`git diff`, with an
  unborn-HEAD `--cached` fallback) and `provider()` is the only place that maps a
  backend to an impl, so a new VCS/range source is a variant + impl with no
  call-site changes — `lint` only talks to the trait. The kept files' diffs are
  **inlined per file in the prompt's "Target files" section**: each changed file's
  unified diff is shown right under its applicability line (rules + diff together),
  so the judge sees a changed file's scope and change in one place.
  **Ignore-aware trimming (`src/domain/diffmodel.rs`):** before a file's diff goes
  into the prompt, it is parsed into *change runs* (maximal contiguous `+`/`-`
  blocks, bounded by context) keyed by new-file line; a run whose every added line
  is ignored (line/block directives) for *every* rule that still applies to the
  file is replaced with an honest one-line marker, never a line pulled from the
  middle of a run (that would misrepresent its neighbors) and never a pure deletion
  (no new-file line to match). This trims tokens for wholly-ignored changes while
  the post-vote cleanup stays the actual enforcement. The
  same diffs stay available to a custom `prompt_template` as the `diffs` context
  block (and per-file as `file_rules[i].diff`), so a `{% if diffs %}…{% endfor %}`
  block still works. An untracked never-added file has no `git diff` output, so it
  counts as unchanged and is skipped (stage or commit it to review it). A
  `--diff git` run outside a git work tree is a clear exit-2 `Error::Diff`, never
  a silent empty diff. **Base selection:** `--diff-base <REF>` (clap `requires`
  `--diff`) sets `GitDiff.base` to any git revision or range — a branch, tag,
  commit, or `A..B`/`A...B` — so `--diff --diff-base main` reviews what the
  current branch changed versus `main`. The default (`base: None`) keeps the
  `HEAD` working-tree diff with the unborn-HEAD `--cached` fallback; an explicit
  base is trusted as-is (a bad ref is git's own exit-2 error, never a silent
  fallback). `provider(backend, base)` threads it from `lint` to the impl. A
  top-level config `diff_base:` sets the default base (a cwd-and-up **session
  setting** — `fold_session_settings`, so a subtree never retunes it; in
  `SETTING_KEYS` + provenance); `apply_cli_overrides` lets `--diff-base` win over
  it, and the effective `config.diff_base` is what reaches `provider`. It only
  tunes the base — `--diff` is still the on switch — so `diff_base` without
  `--diff` is inert.
- **oneharness `--config` is single-file** today; llmlint forwards the first
  `--oneharness-config` and warns on extras. *Follow-up:* make oneharness
  `--config` repeatable, then drop the warning.

## Commits, releases, and merging

- **Squash-merge only, via PR, with auto-merge.** Default branch is protected:
  merge/rebase commits disabled, so one PR is one squash commit whose subject is
  the PR title. Queue with `gh pr merge --auto --squash`; merged heads auto-delete.
  Admins may break-glass.
- **All gating checks required**: `check` (full e2e gate), `deny`, `install`,
  `pr-title`, and the Visual docs diff check (`visual-docs / report (x86_64)`),
  plus linear history, conversation resolution, no force-push/deletion.
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
- **PyPI wheels**: maturin `bin` bindings (`pyproject.toml`) wrap the prebuilt
  binary in per-platform wheels (the ruff/uv pattern) so `pip install llmlint-cli`
  is a seconds-fast binary install — the quickest trustworthy path where package
  registries are reachable but github.com is not. The PyPI *package* is
  `llmlint-cli` (PyPI rejected `llmlint` as too similar to an existing project);
  the installed *binary* is still `llmlint` (named by the Cargo bin target). It
  **depends on `oneharness-cli`** (the same prebuilt-wheel pattern for the
  oneharness runtime prerequisite), so one pip install is a complete working
  setup; the dependency floor mirrors `oneharness::MIN_VERSION`
  (`src/io/oneharness.rs`) — bump both together when the floor moves. `build-wheels` in `release.yml`
  mirrors the binary `upload` matrix (manylinux via `PyO3/maturin-action`) and
  runs unconditioned so a packaging break reddens the release even before
  publishing is activated; `publish-pypi` + `verify-pypi` gate on the
  `PYPI_PUBLISH` repository **variable** (Trusted Publishing is keyless, so
  there is no secret whose presence could self-activate it like
  `CARGO_REGISTRY_TOKEN`). One-time setup: create the PyPI project with this
  repo + `release.yml` as its Trusted Publisher, then set `PYPI_PUBLISH=true`.
  Trusted Publishing auto-generates PEP 740 attestations (the same Sigstore
  provenance as the release assets). Name/version/description stay
  single-sourced from `Cargo.toml` (`dynamic = ["version"]`); release-plz
  remains the only version driver.
- **Release signing + mirror-configurable install**: the `upload` job attaches a
  keyless [Sigstore](https://www.sigstore.dev/) build-provenance attestation to
  each archive (`actions/attest-build-provenance`, bound to the GitHub Actions
  OIDC identity — `id-token: write` + `attestations: write`, no secret/key) **and
  publishes the bundle as a release asset** (`llmlint-<tag>-<target>.sigstore.json`,
  from the step's `bundle-path` output). Shipping the bundle — not relying on
  GitHub's attestation API — is what lets `scripts/install.sh`, pointed at a
  release-proxy mirror (`LLMLINT_RELEASE_BASE_URL` / `--base-url`) for the archive,
  verify integrity **offline** against a root the mirror does not control:
  `cosign verify-blob-attestation --new-bundle-format --bundle …` (preferred,
  vendor-neutral, no GitHub API — the trusted digest is the *signed* attestation
  subject, so no checksum file is consulted on this path), else
  `sigstore verify github --offline --repository …` (the official Python client,
  `pip install sigstore` — the registry-only bootstrap for hosts that cannot
  reach github.com at all; repo-pinned rather than workflow-pinned), else
  `gh attestation verify … --bundle …`, else the `.sha256` fetched from an
  independent root (default **canonical GitHub**; `LLMLINT_CHECKSUM_BASE_URL`
  overrides it). The checksum fallback **refuses a mirror-origin checksum**
  (`sum_trusted`): a checksum sharing the archive's mirror origin is no trust root
  — the mirror would serve a matching tampered checksum — so with no verifier and
  no independent checksum root the install aborts rather than trust the mirror to
  vouch for itself. Verification otherwise fails safe: any verifier/tooling error
  falls through to the next root, and it aborts only when nothing independent can
  vouch for the archive (a real tamper is still rejected). The cosign identity is
  pinned to the release workflow (`PROVENANCE_IDENTITY_RE` + `OIDC_ISSUER` +
  `PROVENANCE_TYPE` = SLSA provenance v1, in `install.sh`). The `verify-attestation`
  job in `release.yml` keeps those invocations honest: on every real release it
  installs cosign (`sigstore/cosign-installer`, pinned `>= 2.4.0` for
  `--new-bundle-format`) and sigstore-python (`pip install sigstore`, pinned) and
  runs the **exact** `install.sh` commands against a just-published archive +
  bundle, so a flag/predicate mismatch reddens the release instead of silently
  degrading users to the checksum fallback. The
  attestation `subject-path` names the archive the
  `taiki-e/upload-rust-binary-action` step leaves in the workspace
  (`llmlint-<tag>-<target>.<ext>`), so keep the matrix `ext` in sync with the
  targets when the build matrix changes.

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
  generation, judge/batch planning, per-file applicability + wrong-file/ignore
  cleanup (`applicability`), vote aggregation, violation model, output formatting,
  exit-code mapping. No process/filesystem/env I/O.
- **`src/io/`** owns all I/O: config discovery + merge + `plugins` resolution
  (local files and remote/versioned URLs, fetched over HTTPS with `ureq`/rustls
  and cached on disk — see `src/io/plugins.rs`), file globbing, the oneharness
  subprocess client, embedded assets. Never hide I/O in a helper that looks pure.
  Discovery is **nested** in both directions (`configfs::load_discovered`).
  **Up:** `discover_all` walks from `cwd` to the filesystem root, merging *every*
  config found (one per directory), nearest first — the most-local config is the
  include root and wins, each more distant config (and its `plugins`) filling only
  what nearer ones leave unset (same nearest-root-wins precedence as `plugins`),
  so user/project configs layer for free. **Down (cascade):** `discover_subtree`
  walks into `cwd`'s subtree, and each rule is scoped to **its own config's
  directory** (`Loaded::scopes` → `files::resolve_scoped`), so a subtree config's
  `files` globs root at that directory (`frontend/`'s `*.txt` → `frontend`'s
  files) while resolved paths stay relative to `cwd`. An **empty `files.include`**
  (no `files` block, at any config in the chain) means **every file under that
  config's resolving root** — the repo-wide default in `files::resolve_scoped`, so
  a config with rules but no `files` lints the whole tree from `cwd` rather than
  nothing; `exclude` and the gitignore-aware walk still narrow it. Session settings
  (model/timeout/template/rationales/default `files`) come from `cwd`-and-up only —
  a leaf scopes *rules*, never the whole run; its agents/rules are still
  contributed. Provenance (`Loaded::provenance`) tracks each item's source the
  same way: a subtree rule traces to its own file, and a descendant's settings
  never appear as a session setting's source (they don't take effect). Rule names
  share one namespace (override spans the chain; a real duplicate is an error).
  Agents share one namespace too, but a **subtree agent may only be used by rules
  under its own directory**: a rule whose config sits *outside* the agent's
  directory picking up that agent (its harness/model/prompt) would let a nested
  folder silently retune how an outside rule is judged, so `load_discovered`
  rejects it with an exit-2 error (`agent_origin` tracks each winning agent's
  defining dir + descendant flag). This closes the same descendant-vs-session leak
  for agents that the settings gate closes for scalars. (There is **no
  `agent.files`** — an agent scopes reviewer context/harness/model, not files;
  per-rule `files` is the one file-scoping knob.)
  The cascade is **relevance-gated by the linted files** (`load_with_targets`):
  with explicit `FILES` on the command line, a subtree config is loaded only when a
  passed file lives under its directory — so linting one area never loads (nor
  fetches the plugins of, nor trips a name clash in) an unrelated subtree, and a
  rule's CLI targets are bounded to its own directory (a passed file outside a
  subtree rule's scope is not judged by it; with none under it the rule is
  skipped). No explicit files keeps the full cascade (each subtree config decides
  what its own area lints). The "is the project configured?" check still uses the
  full discovered set, so a project whose only config sits in an unrelated subtree
  is a clean zero-rule run, not a `ConfigNotFound`. `--config` replaces the whole
  walk with no cascade (`load_explicit`, globs rooted at `cwd`).
- **`src/commands/`** wires domain + io for `lint` (default), `check-ignores`,
  `check-version-bump`, `validate`, `lint-config`, `init`, `config` (`--sources`
  adds per-item provenance), `where` (locate one config item's source), `doctor`,
  `history` (inspect logged run results). `commands/ignores.rs` holds the
  ignore-directive resolution + scan shared by `lint`, `check-ignores`, and
  `lint-config`.
- **Deterministic (model-free) checks + `validate`:** llmlint's static checks —
  config structure (`domain::config::validate`, at load), inline `llmlint: ignore`
  directive structure (`check-ignores`), and **version bumps**
  (`check-version-bump`) — each have a standalone command that spends **no** model
  or oneharness call, and **`llmlint validate`** (`commands/validate.rs`) runs all
  three in one pass — the fast static gate that sits next to fmt/clippy.
  `validate` routes each step through the *same* shared function the standalone
  command uses, so it can never disagree with running them one by one.
  **`check-version-bump`** (`commands/version_bump.rs` + the pure
  `domain::versionbump`) enforces that a **versioned config** (one declaring a
  top-level `version:`, i.e. a published plugin consumers pin with `@`) that
  changed vs a base **also bumped its `version:`** — otherwise a consumer silently
  gets new behavior under a fixed pin. It decides from a file's own text (does it
  declare a version?) and its unified diff (was the top-level `version:` line
  changed to a different value, or newly added?) alone, reusing the same
  backend-agnostic `io::diff` provider `lint --diff` uses (default base `HEAD`;
  `--diff-base <REF>` for a branch/tag/commit/range). Its target set is the
  discovered llmlint config files, **or** the explicit `FILES` named on the command
  line — the escape hatch for an oddly-named plugin config no standard glob matches
  (e.g. this repo's own `assets/config_lint.yml`; guard it with `just
  check-version-bump`, which diffs it against the PR base). A project with no
  versioned config never needs a git work tree; a versioned config with no repo is
  a clear exit-2 `Diff` error, never a silent pass.
- **Results logging** is a session setting (`history:` — `enabled`/`max_runs`/`dir`,
  default on / last 100 / platform **data** dir). `lint::run_loaded` writes each
  completed run's full results (the pure `Report` JSON plus run metadata) as one
  time-sortable-id JSON record via `io::history`, best-effort (a write failure is a
  stderr warning, never a change to the exit code); only for the human report is the
  id hinted on stderr (stdout stays the clean report/JSON channel). Env overrides:
  `LLMLINT_HISTORY_DIR` (dir, wins over config), `LLMLINT_NO_HISTORY` /
  `--no-history` (off). `llmlint history` reads records back (list / show-by-id /
  `latest`, `--status`/`--rule` filters, `--path`, `--format json`); the store,
  id/clock generation, and record shape live in `io::history` (pure logic
  unit-tested there). Like the other session settings it comes from `cwd`-and-up
  only, in `SETTING_KEYS` + provenance. The e2e harness points
  `LLMLINT_HISTORY_DIR` at a per-project temp dir so runs never touch the real data
  dir.
- **config-lint (`assets/config_lint.yml`) is llmlint's own dogfood** — a bundled
  plugin whose rules lint llmlint config files themselves (a clear/unambiguous
  description, a descriptive name that matches what the rule checks, `relevance`
  over inline "not applicable", `files` globs over `relevance` for a rule scoped
  to a file type or location, a description that doesn't restate what its own
  scope already excludes; each rule is phrased to pass its own checks): the
  README's "Writing good rules" guidance, enforced. It is
  **structural checks' complement** — unique names, valid identifiers, resolvable
  agents stay deterministic in `validate` and are deliberately not re-checked
  here. Every rule sets `require_line_attribution: true`, so a finding always cites
  the offending rule's file+line (and, dogfooding the plugin, demonstrates that
  best practice). The rules run on the **default agent** (no dedicated agent): a
  dedicated agent would force a separate judge invocation (batching is per-agent),
  doubling model usage for a small consumer — on the default agent they batch with
  the consumer's own rules into one call. Each rule scopes itself to config files
  with its own `files` filter (shared via a `&config_files` YAML anchor, since
  `agent.files` no longer exists), so the plugin always lints configuration, not
  source. Two entry points, one rule set: consumers **include it as a
  plugin** (the `CONFIG_LINT_URL`, on by default in `llmlint init`; resolves
  offline from the embedded copy via `assets::bundled_url`, so no network/cache and
  no pin bump to stay current), **or** run **`llmlint lint-config`**
  (`commands/lint_config.rs`) — the `lint` engine with that config force-loaded by
  `configfs::load_config_lint` (no discovery, so it works with no project config),
  which first runs the deterministic comment (ignore-directive) check, then the
  judge pass via `lint::run_loaded` (the post-load half of `lint::run`, factored
  out so both share the whole engine). Bump the plugin's `version` and the `@1`
  pins (`init.llmlint.yml`, README, `CONFIG_LINT` in the e2e suite) together when
  its checks change incompatibly.

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
