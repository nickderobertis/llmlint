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

## Two standing goals on every task

The user drives product features and their request is the priority — but carry
two goals into *every* task. When either is the lowest-error path to what the
user asked, fold it into the same task without asking first; surface the rest as
follow-ups (see "After the main task").

1. **Engineer the context for next time.** Make the next agent (and you) see
   more for less: realistic end-to-end tests that drive the real binary the way a
   user does (the e2e suite against the mock-oneharness fixture) — especially when
   a reported bug slipped past the existing ones — scripts and `just` recipes that
   automate repetitive steps and shrink their output to signal, and terse
   `AGENTS.md` notes capturing what the code doesn't make obvious.
2. **Engineer the codebase and environment.** Be the engineer the user isn't:
   prioritize the technical initiatives that keep the codebase clean,
   maintainable, and repeatable, and keep environment setup automated and
   consistent (`just setup` on a bare machine, `just bootstrap` in CI). The strict
   `just check` gate plus local/CI parity (same recipes, the toolchain pinned in
   `rust-toolchain.toml`) make results repeatable — not "works on my machine." A
   clean base and a reproducible environment are usually how the user's feature
   ships with a low error rate.

## Stack and composition

Composes the `create-repo` skill's `base.md` + `project-graph.md` +
`shapes/cli.md` + `languages/rust.md` + `languages/bash.md` +
`intersections/rust-cli.md` + `ci.md` + `releasing.md` + `llmlint.md`
(`llmlint.yml` pins each one's rule fragment). Bash is a composed language
because the scripts (`scripts/`, and those in the screenshots, live, bench and
CI projects) and `.githooks/pre-push` carry real logic; three other languages
are **supporting tooling** only:

- **PowerShell** — `tests/win-color/win-console-color.ps1`, one script: it must drive a
  real Windows console buffer, which only PowerShell can read back.
- **Python** — `screenshots/demo-gif.py`, one on-demand helper that renders the README
  GIF with Pillow; it is neither built, shipped, nor gated.
- **JavaScript (bun)** — Nx runs on Node, installed by bun from `package.json` +
  the one `bun.lock` (bun pinned in `.tool-versions`); `tools/` holds the one
  project-boundary checker and its `bun test` suite. Nothing JavaScript is built
  or shipped.

**Projects in the graph** (`nx.json` + a `project.json` per project; each Rust
one beside its `Cargo.toml`, all members of one Cargo workspace with one
`Cargo.lock`; the tag after each name is its boundary type):

- `llmlint` (`type:app`, the root) — the published crate and its unit tests; it
  also owns every repo-root file no other project claims.
- `config-lint-plugin` (`type:contract`, `assets/`) — the versioned plugin and the
  config schema consumers fetch from `main` by path, plus the templates the binary
  embeds; it depends on nothing.
- `llmlint-mock-oneharness` (`type:fixture`, `tests/mock-oneharness/`) — the
  oneharness wire double behind `--oneharness-bin`.
- `llmlint-e2e` (`type:e2e`, `tests/e2e/`) — the binary journeys; its `test`
  depends on the builds of `llmlint` and the fixture.
- `release-targets` (`type:external`, `tests/release-targets/`) — the release
  declaration's offline checks (`test`) and network checks (`network`).
- `real-oneharness` (`type:external`, `tests/real-oneharness/`) — the PyPI-installed
  oneharness suite (`network`), plus its offline pin check (`test`).
- `live` (`type:live`, `tests/live/`) — the paid live tier.
- `win-color` (`type:e2e`, `tests/win-color/`) — the Windows console rendering check.
- `screenshots` (`type:capture`, `screenshots/`) — the capture, and the pre-push
  guard's and freeze installer's journeys.
- `bench` (`type:bench`, `benches/`) — the informational performance suite.
- `repo-tooling` (`type:tooling`, `scripts/`) — setup, the Nx and gate plumbing,
  the actionlint installer and workflow lint, with their journeys.
- `ci-workflows` (`type:tooling`, `.github/`) — the workflows, the CI routing and
  release-verdict script, and their drift gates; actionlint is its `lint-workflows`.
- `git-hooks` (`type:tooling`, `.githooks/`) — the pre-push hook (its journeys are
  the screenshots project's).
- `workspace` (`type:workspace`, `tools/`) — the boundary check and the
  supply-chain check; `coverage` (`type:workspace`, `tools/coverage/`) — the
  aggregate coverage gate.

Every project declares the repo-uniform target names that apply to it: `format`
(a check; `--configuration=write` writes), `lint` (clippy, or the boundary
check), `lint-sh` (shellcheck), `lint-workflows` (actionlint), `build`, `test`,
`doc`. A target that must never fan out into a gate tier gets a name of its own —
`network` (release-targets, real-oneharness), `live`, `win-color`, `capture`,
`bless`, `gif`, the `bench*` targets, `check-version-bump`, `supply-chain` — so
`nx run-many -t test` can never reach it. Tags bound the edges
(`tools/project-boundaries.json`, checked by the `workspace` project's `lint`):
the crate, the contract, the fixture and the tooling may never depend on an
expensive project (`e2e`, `external`, `live`, `capture`, `bench`), and no
expensive project on another.

Deliberately excluded or deviating (so it isn't re-litigated):

- **The root crate stays a root package, not a virtual manifest** (`rust.md`
  prefers one): consumers fetch `assets/config_lint.yml` and
  `assets/llmlint.schema.json` from `main` by path, `include_str!` and
  `CARGO_MANIFEST_DIR` read `assets/` beside the manifest, and maturin's
  `pyproject.toml` and `cargo install --path .` / `--git` name the root. So the
  root `Cargo.toml` is both the `llmlint` package and the `[workspace]`, and the
  `llmlint` Nx project is rooted at `.` — Nx gives a file to the deepest project
  root containing it, so every unclaimed repo-root file (the justfile, README,
  `AGENTS.md`, `.claude/`, `docs/`, `shots/`, the root configs)
  belongs to `llmlint` and an edit there selects every project downstream of it.
  That is why the workflows (`.github/`), the git hooks (`.githooks/`), the
  scripts (`scripts/`) and each suite's own scripts live in project directories
  of their own.
- **Moved from the requested split, with reasons**: actionlint is
  `ci-workflows`' `lint-workflows`, not `repo-tooling`'s, because the workflow
  files are that project's (`repo-tooling` keeps the script it runs);
  `ci-workflows`, `git-hooks` and `workspace`/`coverage` are projects the split
  did not name, for the ownership reason above and the aggregate gates; the pre-push guard's
  journeys sit in `screenshots`, whose capture scripts the hook drives.
- **No remote Nx cache** — the repo runs on the Nx local cache only
  (`.nx/cache`, gitignored), kept per checkout: `scripts/nx` pins it there,
  since Nx 23 otherwise shares the main git worktree's cache across worktrees.
  CI starts every run from a cold cache and persists
  none, so a cached result can never stand in for a CI verdict; locally the cache
  replays unchanged targets (inputs are declared per target, so an edit outside
  them replays and an edit inside reruns).
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
  (`cargo-nextest`, `cargo-llvm-cov`), the pinned `actionlint`, a check that Node
  is present (Nx runs on it; setup does not install it), then `just bootstrap`.
  Idempotent and
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
- `just bootstrap` — the step `setup` finishes with (toolchain components +
  `cargo fetch`, then the `.tool-versions` bun and the locked Nx install); CI
  calls it directly after installing the
  toolchain + tools its own way — the jobs that run the gate or its tests (`gate`,
  `cross`) install the channel pinned in `rust-toolchain.toml`
  (`actions-rust-lang/setup-rust-toolchain`), never a floating `stable`; only
  `install`, which stands in for an end user's machine, uses a stock stable. Use
  `just setup` for a bare machine.
- `just check` — the gate, delegated to Nx (`scripts/nx-tier.sh` picks the
  tier): the targets `format` (check), `lint` (clippy `-D warnings`, the
  boundary check), `lint-sh`, `lint-workflows`, `build`, `test` (unit, **e2e**,
  the offline release-targets and script journeys, coverage-measured ones under
  cargo-llvm-cov), `doc` and `coverage` (the 95% floor over their union). With no flag it runs the
  **affected tier** — `nx affected` from an explicit base: `NX_BASE` when set
  (only a plain ref name or a commit SHA, else refused before any target runs),
  otherwise the merge base with `origin/main`. `just check --all` runs the **full
  sweep** (`nx run-many --all`). Must pass before any commit or PR. `just test`,
  `just lint`, `just lint-sh`, `just lint-workflows`, `just fmt-check`,
  `just format` (writes) and `just doc` take the same flag and run that one
  target; `just test-e2e` and `just coverage` run one project's.
- `just lint-sh` — shellcheck over every project's scripts and the git hooks
  (each project's `lint-sh` target). Fix a finding at its
  site; a `# shellcheck disable=` is site-scoped and carries its reason. `just
  setup` does not install shellcheck yet (CI's ubuntu runner ships it).
- `just lint-workflows` — the pinned actionlint (`actionlint-version` in the
  justfile) over every workflow in `.github/workflows/` (the ci-workflows
  project's target); part of `check`. A finding is fixed, not suppressed. `just
  setup` / `just actionlint-tools` install the pin from the prebuilt release,
  digest-checked against `scripts/actionlint.sha256` (refresh it with the pin); a
  missing or off-pin actionlint fails naming that command.
- `just upgrade` — update dependencies, then re-run the gate as a full sweep
  (`just check --all`).
- `just check-version-bump [base=origin/main]` — the config-lint-plugin
  project's version-bump dogfood; see `assets/AGENTS.md`.
- `just deps-check` — `cargo deny` + `cargo machete` (the workspace project's
  `supply-chain`; separate from the gate tiers, needs network).
- `just test-release-targets` — the release-targets project's `network` target,
  outside the gate tiers (the `Release targets` workflow runs it); its offline
  half is the project's `test`. `release-targets.toml` is the canonical
  release-target declaration other repositories wait on; see
  `tests/release-targets/AGENTS.md`.
- `just test-oneharness` — the real-oneharness project's `network` target
  (installs the released oneharness from PyPI), outside the gate tiers; see
  `tests/real-oneharness/AGENTS.md`.
- **LLM-judge tier (dogfood)** — `just lint-llm-validate` (model-free: config
  structure, ignore directives, version bumps) and `just lint-llm-diff <base>`
  (the judge over the branch's changes) run the released llmlint
  (`just setup-llmlint`, floor `LLMLINT_MIN` in `scripts/setup-llmlint.sh`)
  against `llmlint.yml` + `oneharness.toml`. CI's `llmlint` job runs validate, then
  the judge, and must provision and authenticate the harness `oneharness.toml`
  selects first (codex, keyed by `OPENAI_API_KEY`); a missing key fails the job,
  never a green no-op. The pre-push hook runs `just lint-llm-validate` too (see
  `.githooks/AGENTS.md`).
- `just lint-live` — opt-in, ad-hoc live run against real oneharness + a real
  harness (`cargo run -- …`); never in the gate or CI.
- `just live-claude` — the paid **live e2e tier** (the live project): runs on PRs
  in its own workflow (`.github/workflows/live.yml`) across Linux, macOS and
  Windows, out of `check`; a missing CLI, auth or oneharness is a hard failure.
  See `tests/live/AGENTS.md`.
- `just win-color` — the **Windows color-rendering gate** (the win-color
  project): Windows-only, so it runs in its own workflow
  (`.github/workflows/win-color.yml`) on `windows-latest`, not in the Linux gate
  tiers. See `tests/win-color/AGENTS.md`.
- **Performance suite** (`just bench`, `bench-cli`, `bench-allocs`,
  `bench-instructions`, `bench-compare`, `profile`) — *informational, never a
  gate*. See `benches/AGENTS.md`. The Criterion + allocation benches measure the
  pure engine (`benches/`); `benches/bench.sh` (hyperfine) and
  `benches/bench-instructions.sh` (cachegrind) measure the real binary end to end
  against the **mock-oneharness fixture**, so there's no model/network cost — just
  llmlint's own work plus one child spawn. The `Performance` workflow
  (`.github/workflows/bench.yml`) runs all of this on each PR and posts a sticky
  comment + job summary with a base-vs-PR delta; timings are noisy on shared
  runners, so it reports rather than blocks. The bench/profile tools (hyperfine,
  critcmp, samply) are *not* installed by `just setup` — `just bench-tools`
  installs them on demand; CI installs them via `taiki-e/install-action`.
- **Terminal screenshots** (`just screenshots`, `screenshots-tools`,
  `screenshots-bless`) — *informational, never a gate*. See `screenshots/AGENTS.md`.
  `screenshots/screenshots.sh` drives the real binary against the **mock-oneharness
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
  build. **Two lanes** are declared in `[capture].arches` — `x86_64` and `arm64`
  (CI runs the arm64 one on `ubuntu-24.04-arm`) — each with its own committed
  baseline, because the local pre-push guard (`.githooks/pre-push`) classifies and
  re-blesses the lane of the **host it runs on** and refuses a host arch no lane
  declares; llmlint is developed on arm64 and released from CI's x86_64, so both
  are real hosts. The SVGs are identical across arches, so the two baselines are
  the same bytes (a screenshots journey holds them equal) and one host's
  `just screenshots-bless` — which rewrites **its own** lane only, named by
  `screenshots/host-arch.sh` — is checked by CI's job for the other lane. `freeze` is
  *not* installed by `just setup` — `just screenshots-tools` installs the pinned
  version; screencomp is installed separately (CI installs both, `freeze` via
  `screenshots/ci-install-freeze.sh`, which picks the prebuilt release matching the
  runner's arch). Keep the two `freeze` version pins in sync (`freeze-version` in
  the justfile, `freeze_version` in `screenshots/ci-install-freeze.sh`; a screenshots
  journey gates them against each other). The capture renders through `freeze`,
  so the screenshots project's `capture` target is left to the Visual docs
  workflow and the pre-push guard; its `lint-sh` and `test` are in the gate tiers. The README **hero** is a separate animated GIF of the
  live-progress view (`docs/screenshots/demo.gif`, `just screenshots-gif`,
  `screenshots/demo-gif.py`) — same real-binary-against-the-fixture approach, rendered
  to frames with the vendored font (Pillow, no `ttyd`/`ffmpeg`); it is *not*
  hash-gated (a GIF isn't byte-reproducible), so it is regenerated on demand.

## How llmlint drives oneharness

llmlint shells out to `oneharness run` once per `(agent, judge, batch)` (plus a
bounded corrective re-ask — see the scope bullet below), passing the rendered
template via `--system-file` (a temp file, not an inline argv string — the
briefing carries every changed file's inlined diff, so an inline `--system`
would trip the OS `Argument list too long` limit; `--system-file` needs
oneharness >= 0.3.12), a generated JSON Schema via `--schema` (oneharness
validates it and re-prompts on failure), and `--format json` (oneharness's
default output is a human text view), then reading the per-result `structured`
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
  a large briefing never trips the OS argv limit — needs oneharness >= 0.3.12).
  A deferred builtin tool is reported as a named `failure_kind: "tool_deferred"`
  since 0.3.21, which is what lets llmlint give the specific deferred-tool
  diagnostic (below) instead of an opaque schema error. The floor is
  **oneharness >= 0.14.0** (`oneharness::MIN_VERSION`): every `run` passes
  `--format json` (beside `--compact`) because oneharness's default output is a
  human text view, and an older binary refuses `--format` as an unknown
  argument. Both `lint` (pre-flight, once per run) and
  `doctor` parse `oneharness --version` and fail with a clear exit-2 error when
  the binary is older (or its version can't be parsed) rather than letting a
  missing flag blow up mid-run. Bump `MIN_VERSION` in `src/io/oneharness.rs` and
  the `oneharness-cli` floor in `pyproject.toml` together when the floor moves; a
  unit test beside `MIN_VERSION` gates them against each other, and the mock
  oneharness reports `MIN_VERSION` as its default version rather than restating it.

- **Deferred-tool diagnostic (convention, issue #142):** the judge *inherently*
  uses tools — it reads the code it judges — so a harness deployment that
  **defers** builtin tools to an external controller instead of executing them
  inline (a bridged/managed Claude Code session, empty
  `tengu_non_deferrable_builtins`) makes every judge call dead-end with no
  verdict. oneharness (since 0.3.21) names this as `failure_kind: "tool_deferred"`
  on the result (status may be `ok`; `structured` null) with an actionable
  `error`. `parse_verdicts` (`src/io/oneharness.rs`) checks that **before** the
  schema/no-structured branches and raises `Error::ToolDeferred`, surfacing
  oneharness's detail inside a pointed message (run from a standalone shell / CI),
  never the generic "failed schema validation / no JSON value could be extracted"
  the issue chased. `llmlint doctor --probe` catches it up front: an opt-in,
  billed probe that asks the harness to read a temp file and confirms the tool
  *executed* (`Client::probe` → `ProbeOutcome`), rather than only that the binary
  answered. The requirement (harness must execute tools inline) is documented in
  the README.

- **Fallback-winner selection (convention):** in oneharness **fallback** mode the
  `results` array lists every *attempted* harness in priority order — so
  `results[0]` may be one skipped as unavailable, not the one that ran. llmlint
  reads the verdict from the harness oneharness names in the top-level
  `fallback.ran` (falling back to the first `results` entry that produced
  structured output), never blindly `results[0]` (`select_winner_index` in
  `src/io/oneharness.rs`). A non-fallback run has one result, so `results[0]` still
  wins. Only when a whole chain left no successful harness does llmlint error, and
  then the message names the entire chain (`fallback_chain_error`) rather than a
  single skipped harness's "no structured output".
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
  `lint --no-ignore-check` skips **only** this structural pre-flight (honoring below
  is untouched), for a pipeline that already runs the structural check as its own
  step: our `lint-llm-diff` recipe passes it so the LLM judge can review files that
  legitimately hold *example* directives (docs, the parser, `tests/e2e/main.rs`)
  without the gate tripping, while `lint-llm-validate` still runs the structural
  check with the `ignore-scan-exclude` denylist. The gate is skipped, not weakened —
  `check-ignores`/`validate` remain the enforcing path.
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
- **Explicit `FILES` intersect the globs (convention):** a rule's globs say which
  files the *rule* is about; the positional `FILES` say which files *this run* is
  about; a file must satisfy **both** to be judged (`ignores::resolve_files`, over
  `files::keep_included`). Naming a subset therefore narrows every rule, including
  one with its own `files` — which used to win outright, discarding the passed set
  and pulling its whole glob match back in, so "judge exactly these files" was
  unexpressible and the only way to bound a run was to `--exclude` the complement.
  A rule's *effective* filter is its own `files` when it declares one, else its
  config's (the `RuleScope` fallback), and the intersection applies to both, so a
  passed file no rule is about is judged by none of them rather than by the
  subset that happen to lack a `files` block. An **empty `include`** still means
  every file, so a config with no `files` block judges exactly what was passed
  (the common case, unchanged). An empty intersection leaves the rule with no
  files: it is **skipped for this run** — counted in the summary, named at `-v`,
  listed under "not judged" in the plan — never an error and never a pass over
  files it never saw. `exclude` stays a denylist over the result, winning even
  over an explicitly-named file. With no `FILES` at all, resolution is exactly
  what it was.
- **Explicit `FILES` must be readable (convention):** a positional `FILES` entry
  is a per-invocation assertion that *this file is there to be judged*, so one
  llmlint cannot read as a file is an exit-2 usage error (`check_cli_files` in
  `commands/lint.rs`, over `files::unresolved`) — reported in `read_text`'s own
  `reading <path>: <os error>` wording so `lint` and `check-ignores` read as one
  tool (both raise it through `ignores::reject_unresolved`), and raised before rule
  selection, planning, or any judge call, so a bad invocation costs nothing.
  Silently accepting one narrows what is judged without saying so — under
  intersection semantics a mistyped path simply falls out of every rule's set, so
  nothing downstream would ever try to read it — and the run then reports a
  confident pass over a fraction of the ruleset — the same false green
  `validate_filters` guards for `--rule`/`--agent`. It lives in the shared
  `lint::run_loaded`, so `lint` and `lint-config` get it from one place, and in
  `check_ignores::run` for the standalone scan.
  **Existence is not the bar — readability is:** `unresolved` decides by
  attempting `read_text`'s own read rather than stat-ing, so a *directory*
  (present, yet not a file any judge could read) is rejected like an absent path,
  while a **binary file stays a valid target** (`read_text` decodes it to `None`;
  only the decode, not the read, distinguishes it). Doing the identical read is
  what keeps this from ever disagreeing with the ignore scan moments later.
  Two deliberate non-errors: a path that **resolves but selects nothing** (no
  `--diff` overlap, an `exclude`, a scope it falls outside) is a legitimate empty
  selection, and under `--diff` a tracked file **deleted** from the work tree is
  accepted (it is what a `git diff --name-only` wrapper passes, and
  `restrict_to_changed` already drops it). That exemption is held to *absence*
  (`Unresolved::missing`) — only an absent path can be a deleted file, so the
  backend is asked only about those, and a present-but-unreadable path is never
  waved through even when the backend reports changes for it (git answers a
  directory pathspec with its whole subtree's diff). A **glob** that matches
  nothing is *not* an error either — the decision is that a glob and a passed
  path are different speech acts. A glob is a declarative pattern whose empty
  match is a normal state here (a subtree in the cascade, a `--diff`
  intersection, an `exclude`, a repo-wide config shared across areas), and —
  unlike the `FILES` case — the narrowing is **already reported on three
  surfaces**: the default summary counts the skip with no flag, `-v` names the
  rule with `no files matched`, and the plan explanation lists it under "not
  judged" (so `--format json` and history carry it too). What made the `FILES`
  bug a false green was invisibility, not narrowing, and that is absent here. A
  bare warning is rejected for the same reason the repo has no warnings-only
  mode: it would fire on every legitimate empty match and train readers to
  ignore it. Pinned by
  `a_config_glob_that_matches_nothing_is_reported_not_rejected`.
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
  a silent empty diff. **Hook-proof spawns (convention):** git's
  repository-selection environment variables **outrank `-C`**, and every git hook
  exports `GIT_DIR` for the repository it fired in (`pre-push` adds
  `GIT_INDEX_FILE`) — so an inherited environment would make `--diff` silently
  judge the hook's repository instead of the root it was given, a false clean
  with a green exit code. llmlint's flagship deployment *is* a pre-push hook, so
  every git spawn goes through one helper, `diff::git_command`, which clears them
  (the list is `AMBIENT_REPOSITORY_VARS` in `src/io/diff.rs` — that constant is
  the only place it is written down) and leaves `-C` as the only repository
  selector. Add no bare `Command::new(git_bin)` — including in tests, where a
  scratch repo built with an ambient `GIT_DIR` is not a scratch repo at all.
  **Base selection:** `--diff-base <REF>` (clap `requires`
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
- **Uniform settings precedence (convention):** every top-level (session) setting
  resolves through one chain — **CLI flag > `LLMLINT_` env var > config file >
  built-in default** — mirroring oneharness's `ONEHARNESS_` env convention. The env
  layer is `io::env::apply_overrides` (`ENV_SETTINGS` is the key↔var table; a test
  pins it against `SETTING_KEYS` so a new setting can't silently miss env support).
  It runs **after** the nearest-wins config merge and **before** `apply_cli_overrides`
  (so CLI still wins), folding each set `LLMLINT_*` var into the merged config and
  recording its provenance as `env:<VAR>` (so `config --sources` / `where` stay
  honest). Env is **process-wide**, not cwd-and-up — it tunes the effective run, not
  one directory's config. A var name is the setting path uppercased, `.`→`_`,
  prefixed `LLMLINT_`; bools are `1/true/yes` vs `0/false/no` (case-insensitive); a
  malformed value is an exit-2 `Error::Env` **located to the variable**, never a
  silent skip (validate at the boundary). Only `version` is config-only. The
  structured `files` setting is reported (and env-overridden) at **sub-field**
  granularity — `files.include` / `files.exclude` are their own `SETTING_KEYS`
  entries (like `oneharness.*` / `history.*`), so `where files.exclude` resolves and
  each has its own `LLMLINT_FILES_*` var (a `PATH`-separated glob list). Their merge
  differs by kind: **include replaces** (highest layer that sets it wins — env >
  config), **exclude accumulates** (config ∪ env ∪ `--exclude`), so a per-run
  exclude never drops a config safety exclude. Positional CLI files are *not* a
  layer of `files.include`: they **intersect** whatever include set wins (and each
  rule's own `files`), per the explicit-`FILES` convention above. The exclude
  denylist wins even over an **explicitly-passed** file — a named file matching any
  exclude is dropped (`files::drop_excluded` in the `resolve_files` CLI-files
  branch), the same
  "an include never resurrects an excluded path" rule (issue #128) the glob path
  follows. A session-level `files` override reaches the per-rule scopes captured at
  load via `ignores::retarget_session_scopes` (cwd-rooted scopes only), so a
  `files.include` env/CLI override actually changes what session rules target, not
  just the reported config. The `--exclude` flag exists
  on `lint`/`lint-config` **and `validate`** (so the static ignore-scan sees the same
  target set). Reached by every command that reads the settings — `lint`/`lint-config`
  (via `run_loaded`), `config`, `where`, `validate`; `history` and `doctor` read the
  relevant env var directly. **Back-compat:** the canonical
  `LLMLINT_HISTORY_ENABLED` supersedes the legacy `LLMLINT_NO_HISTORY=1` off-switch
  (honored in `history::resolve` only when the canonical var is unset);
  `LLMLINT_HISTORY_DIR` and `LLMLINT_ONEHARNESS_BIN` keep working, now folded into
  the same scheme. When a new session setting lands, add its `LLMLINT_` var here.
- **oneharness configs are layered (convention, issue #210):** every resolved
  oneharness config file is forwarded as its own `--config`, lowest layer first,
  because oneharness layers repeated `--config`s (a later file overrides an
  earlier one per field). The one list is `oneharness.config` — concatenated
  across plugins and nested llmlint configs, most distant first and nearest last,
  an exact duplicate kept at its nearest position (`OneharnessCfg::merge_under`,
  the one exception to first-writer-wins session settings) — then
  `LLMLINT_ONEHARNESS_CONFIG`'s `PATH`-separated paths (appended by the env layer),
  then the `--oneharness-config` flags (`resolve_oneharness_config` in
  `commands/lint.rs`), so the command line is the top layer. More than one file
  needs `oneharness::LAYERED_CONFIG_MIN_VERSION` (repeatable `--config`); below it
  `lint` exits 2 naming the found version and that floor, never falling back to
  the first file. One file keeps the `MIN_VERSION` floor and the argv it always
  had, which is why the `pyproject.toml` floor stays at `MIN_VERSION`. The hermetic
  e2e journeys pin the argv; `just test-oneharness` proves the released oneharness
  resolves those layers as intended.

## Commits, releases, and merging

- **Squash-merge only, via PR, with auto-merge.** Default branch is protected:
  merge/rebase commits disabled, so one PR is one squash commit whose subject is
  the PR title. Queue with `gh pr merge --auto --squash`; merged heads auto-delete.
  Admins may break-glass.
- **Required status checks** — the fixed context contract: `gate` (the full
  `just check` gate), `deny`, `pr-title`, `cross (macos-latest)`,
  `cross (windows-latest)`, `install (ubuntu-latest)`, `install (macos-latest)`,
  `install (windows-latest)`, `visual-docs / report (x86_64)` and
  `visual-docs / report (arm64)` (the Visual docs diff check, one per declared
  capture lane); plus linear history, conversation resolution, no
  force-push/deletion. `llmlint` (the judged tier, `ci.yml`) is the blocking PR
  check the create-repo skill requires to be required too — else auto-merge lands
  a PR past a red judge run. Branch protection is applied outside this repo (the
  `gov-llmlint` governance step requires `llmlint`); `setup_github_governance.py
  --verify` reconciles this list with the live settings. A ci-workflows journey
  (`every_required_context_is_reported_on_every_pull_request`) holds the
  workflows to these names: renaming a job or adding a filter, `if:` or `needs`
  that could leave one unreported on a PR fails the gate. `notignored`
  (`notignored.yml`, the PR comment listing added suppressions) is deliberately
  not required: it skips fork PRs.
- **PRs follow `.github/pull_request_template.md`** (What / Why; the squash body).
- **Where each gate tier runs** (`ci.md` "Staged gates";
  `.github/scripts/ci-gate.sh tier` decides it for every run of `ci.yml`): a pull
  request and a push to `main` (merge-to-main) run the **affected tier** —
  `NX_BASE=<base> just check`, from the PR's merge base with its base branch, or
  from the previous tip of `main` — in `gate`, and the affected tests (plus fmt
  and clippy) in `cross`. The **full sweep** (`just check --all` in `gate`, every
  test in `cross`) runs at **release-prep, on the release-plz release PR**, checked
  out at its head. Why there: release-plz batches merged changes behind one
  release PR that can accumulate several merges, so the commit that ships is not
  one any merge job swept; sweeping at merge-to-main would sweep trees that never
  ship and still miss the one that does. A manual `workflow_dispatch` of `ci.yml`
  is also a full sweep — the recovery when a release finds no verdict. Each tree
  is gated once per tier: `release.yml` re-runs no lint or test target. Its
  `verdict` job (`ci-gate.sh verdict`) reads GitHub for the newest sweep run whose
  tested commit has the released commit's **tree** and needs its `gate` and both
  `cross` jobs green; a red, missing, or different-tree verdict stops the release,
  and `upload`, `publish-crate` and `build-wheels` (hence `publish-pypi`) all
  `need` it. No gate-time threshold is recorded yet: measuring and deriving one
  (`ci.md` "Measure, then derive") is deferred by the 2026-10-06 budgets ruling.
- **Releases**: Conventional Commits drive release-plz (pre-1.0: `feat`→minor,
  `fix`/`perf`→patch, `!`/`BREAKING`→minor; `docs`/`test`/`chore`/`ci`→no release).
  release-plz opens a release PR, auto-merges it on green, tags `vX.Y.Z`, and cuts
  the GitHub Release, which fires `release.yml` to build+attach checksummed
  binaries and, when opted in, `cargo publish` the crate — once its `verdict` job
  finds a green full sweep of the released tree (above). Needs the
  `RELEASE_PLZ_TOKEN` PAT (a `GITHUB_TOKEN` tag won't retrigger `release.yml`);
  the workflow no-ops until the secret exists. Don't hand-bump the version or
  `CHANGELOG.md`.
- **crates.io publish**: `release.yml`'s `publish-crate` job runs
  `cargo publish --locked` whenever the `CARGO_REGISTRY_TOKEN` secret is set (the
  `guard` job gates it). It is gated on the release `verdict` job but independent of
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
- **Plugin cache keyed by the resolved version (convention):** a pin (`@1`) is a
  *range*, so it must never be the cache key — keying by it freezes a host on the
  first version it ever fetched, silently and undiagnosably. An entry is
  `<url-hash>/v<version>.yml`, keyed by the version the fetched **document
  declares**, with its metadata beside it in `v<version>.json` — `CacheMeta` is
  that shape, `CACHE_SCHEMA` versions it, and the committed golden
  (`tests/fixtures/plugin_cache/`) pins the persisted form byte for byte — change
  the shape and all three move in the same commit, rather than restating the
  fields here. Resolution takes the newest
  entry satisfying the pin, admits it only while its document still declares the
  version its metadata claims, and revalidates once it is older than
  `LLMLINT_PLUGIN_TTL` (seconds, default 3600; it and `LLMLINT_PLUGIN_REFRESH`
  read the same grammar every other `LLMLINT_*` setting does, and reject the rest). A revalidation that cannot be made
  reuses the entry and never fails the run: a cache is a speed-up, not a network
  dependency. A previous-layout file (named for a *pin*, no metadata) is
  invisible. Keep the diagnosis path working — `llmlint plugins` / `plugins clear`
  and the resolved versions named in the unknown-rule ignore error.
- **`src/commands/`** wires domain + io for `lint` (default), `check-ignores`,
  `check-version-bump`, `validate`, `lint-config`, `init`, `config` (`--sources`
  adds per-item provenance), `where` (locate one config item's source), `doctor`,
  `history` (inspect logged run results), `plugins` (report / clear the plugin
  cache). `commands/ignores.rs` holds the ignore-directive resolution + scan
  shared by `lint`, `check-ignores`, and `lint-config`.
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
  only, in `SETTING_KEYS` + provenance. **Run labels** are not a setting (no
  config key, so absent from `SETTING_KEYS`/`ENV_SETTINGS`): their grammar lives
  only in `domain::labels`, and their user-facing contract is the README's
  "Labelling runs" section, which callers (onejudge) parse against — so change it
  deliberately, never as a side effect. The persisted record is pinned by the
  goldens in `tests/fixtures/history_record/` (its README says why `labels` is an
  optional additive field rather than a version bump); change the shape and the
  goldens move in the same commit. The e2e harness points
  `LLMLINT_HISTORY_DIR` at a per-project temp dir so runs never touch the real data
  dir.
- **config-lint (`assets/config_lint.yml`) is llmlint's own dogfood** — a bundled
  plugin whose rules lint llmlint config files themselves (a clear/unambiguous
  description, a descriptive name that matches what the rule checks, `relevance`
  over inline "not applicable", `files` globs over `relevance` for a rule scoped
  to a file type or location, a description that doesn't restate what its own
  scope already excludes, a `relevance` that scopes the rule without deciding its
  verdict; each rule is phrased to pass its own checks): the
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
  out so both share the whole engine). Versioning the plugin: see
  `assets/AGENTS.md`.

## Tests are context engineering

This is an agent-driven repo: the test suite is the *only* QA loop. Realism and
coverage are a rule, not a preference.

- **The layer under test is llmlint.** The genuinely-external boundary is the
  `oneharness` subprocess — e2e drives the **real `llmlint` binary** against a
  **mock-oneharness fixture** (the `llmlint-mock-oneharness` crate behind the
  `--oneharness-bin` override), exactly as oneharness mocks the real agent CLIs. Never mock
  llmlint's own logic (config/render/batch/vote/output).
- **Done means complete, not minimal:** every user journey, happy path *and*
  failure/recovery. The e2e journey list lives in `tests/e2e/AGENTS.md` and is the
  source of truth for what's covered; a feature isn't done until its journey lands.
- A live tier (`just live-claude`, and the ad-hoc `just lint-live`) hits real
  oneharness + a real harness, out of the gate tiers; see `tests/live/AGENTS.md`.

## Scripts and output are context

- Recipes/scripts are quiet on success — a line or nothing. On failure, preserve
  the exact error (paths, rule names, exit codes) and suggest the next action.

## Keeping the allowlist current

The agent command allowlist lives in `.claude/settings.json`; the tool enforces
it. When a new routine command joins the build/test/release workflow, add it
(kept narrow) instead of re-approving it each session.

## After the main task: refine and hand off

After completing the user's requested task, act on the two standing goals above:
look for ways to make future work easier and propose follow-ups — but only ones
that are materially helpful, and note each one's likely impact:

- **Scripts** — a repeatable step you did by hand that should be a `just` recipe
  or a script.
- **`AGENTS.md`** — a constraint, gotcha, or decision worth recording here.
- **Skills** — guidance general enough to belong in a shared skill.
- **Other context** — e2e journeys, fixtures, or docs that would improve
  visibility.

Skip busywork. If nothing is materially helpful, say so and stop.
