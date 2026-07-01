# Design: interactive progress view

Status: accepted · Owner: llmlint maintainers

## Problem

During a `lint` run, llmlint drives one or more `oneharness` judge calls in
parallel and prints **nothing until every verdict is in** (`src/commands/lint.rs`
batches the runs across a `thread::scope` wave loop and only calls `emit()` at the
end). On a real run that is a multi-second silent gap — no feedback that anything
is happening.

We want a **test-runner-style live view** (think vitest/jest: rules resolving one
by one, a spinner, a running count) for humans at an interactive terminal —
**without** corrupting output when the same binary is run non-interactively: piped
to a file, in CI, or captured by an AI coding agent (Claude Code, Codex, Cursor).
A naïve progress bar emits cursor-movement and carriage-return control codes that,
in a captured stream, are not interpreted — they land as literal control-character
spam that wastes tokens and garbles logs (the "christmas tree in CI" problem).

## What the ecosystem does (and why)

Researched across the agent harnesses and the major test runners; full citations
at the end. The near-universal pattern:

> **Auto-detect the audience. Render a rich, ephemeral view on *stderr* for an
> interactive human; degrade to a plain, stable stream for everyone else. The
> machine-readable result is a separate channel.**

Concrete confirmations:

- **Vitest / Jest keep *one* reporter and gate only the live layer.** Jest's
  `DefaultReporter` gates its in-place status line on `isInteractive`
  (`!isCI && stdout.isTTY && TERM !== 'dumb'`); Vitest's default reporter simply
  omits the pinned live summary when interactivity is off. Neither swaps the whole
  reporter for non-TTY. They add a *distinct* reporter only for a distinct
  *consumer* — GitHub-Actions annotations, or Vitest 4.1's new **`agent` reporter**
  that auto-activates inside an AI agent and prints only failures to save tokens.
- **`indicatif` (Rust) and `ora`/`log-update` (Node) self-hide off-TTY.**
  indicatif's default draw target renders to a terminal and, per its docs, *"if the
  terminal is not user attended the entire progress bar will be hidden. This is done
  so that piping to a file will not produce useless escape codes."*
- **The result channel is separate.** ripgrep `--json`, ESLint `--format json`,
  pytest JUnit XML, pnpm `--reporter ndjson` — every tool exposes a structured
  stream as the machine counterpart to the pretty view. llmlint already has this:
  `--format json`.
- **TTY detection alone is not enough for agents.** Codex allocates a **PTY** for
  some exec paths (so `isTTY` can be *true* even though a model is reading), and
  Codex's `unified-exec` even sets `NO_COLOR=1` + `TERM=dumb`. Claude Code's Bash
  tool uses plain pipes *and* sets `CLAUDECODE=1`; Cursor sets `CURSOR_AGENT`.
  There is **no single signal** that catches every harness — so the detection is
  layered. `std-env`'s `isAgent` (what Vitest uses) checks `CLAUDECODE`,
  `CURSOR_AGENT`, `REPL_ID`, `GEMINI_CLI`, etc.

## Design

### Two layers, not three modes

llmlint's **default human report is already the lean, failures-only view** an agent
wants (only failing rules + a one-line summary; passes/skips are counted, not
itemized — `src/domain/report.rs`). So we do **not** need a separate agent report.
The feature is two independent pieces:

1. **The report renderer** (`report.to_human`) — unchanged. Same content for every
   audience; color already resolved by `anstream::AutoStream` at the I/O boundary.
2. **An ephemeral live-progress layer** — the only thing the interactive decision
   toggles. Drawn to **stderr** (so stdout stays the clean report / JSON channel),
   fully self-erasing, built on `indicatif`.

The machine channel (`--format json`) is untouched.

### The decision predicate

A new `--progress <auto|never|always>` flag, mirroring the existing `--color`:

```
show_live = match choice {
    Never  => false,
    Always => stderr_is_tty,                    // force on, but indicatif still
                                                // won't animate a non-terminal
    Auto   => stderr_is_tty && !is_ci && !is_agent && color_ok,
}
```

Notes:

- Gate on **stderr**'s TTY-ness, not stdout — the animation lives on stderr, so a
  human running `llmlint > report.txt` still gets progress while stdout is a file.
- `is_agent` is the extra layer TTY-detection can't cover (PTY-allocating agents).
  It checks the env vars `CLAUDECODE` / `CLAUDE_CODE` / `CURSOR_AGENT` / `CODEX_*` /
  `GEMINI_CLI` / `REPL_ID`.
- `is_ci` is `CI` set to anything but `false`, or a known vendor var
  (`GITHUB_ACTIONS`, `GITLAB_CI`, `CIRCLECI`, `TRAVIS`, `JENKINS_URL`, `TF_BUILD`,
  `BUILDKITE`).
- `color_ok` folds in `NO_COLOR` / `TERM=dumb` (a terminal the user asked to keep
  plain shouldn't sprout an animation either).
- `--progress always` forces the *decision* on, but indicatif's stderr draw target
  still refuses to animate a non-terminal — we deliberately never spam a pipe, even
  when asked. This is honest and protects captured output.

### Agent-detection also forces color off

Under agent detection, `--color auto` resolves to **off**. Rationale: a
PTY-allocating agent has `isTTY == true`, so `auto` would otherwise emit color; but
captured ANSI is unreliable (Claude Code strips/mangles it and ignores `NO_COLOR`).
Emitting plain text is the safe path. `--color always` still forces color (explicit
override wins). This is a one-line change to `ColorChoice::Auto::resolve`.

### The live view

Model **rules as tests**. A `ProgressView` (built on `indicatif::MultiProgress`)
shows one line per rule plus a header count, driven from the existing wave loop:
as each `JudgeRun` completes we mark its rules; when *all* of a rule's judge runs
are in, we tally it with the **same `vote::tally`** the report uses, so the live
✓/✗ can never disagree with the final report. On completion the whole block is
cleared from stderr and the normal report is written to stdout.

```
 llmlint · 4 rules · 12 judge calls

 ✓ modules_have_doc_comments      passed
 ⠙ handlers_delegate_to_services  held 1/2 judges
 ⠸ no_raw_sql                     running
 ⠿ error_messages_actionable      queued

 8/12 judge calls · 14s
```

### Where the code lives (respecting the architecture split)

- **`src/io/terminal.rs`** (new) — the impure gather: real `stderr().is_terminal()`
  + env reads. The *pure* helpers `is_ci(env)` / `is_agent(env)` / `no_color(env)`
  take an env accessor so they're exhaustively unit-testable without touching the
  process environment.
- **`src/cli.rs`** — `ProgressChoice` enum + a pure `resolve(is_tty, is_ci,
  is_agent, color_ok) -> bool` (like `ColorChoice::resolve`), unit-tested.
- **`src/commands/progress.rs`** (new) — the `ProgressView` wrapping indicatif. It
  takes a `ProgressDrawTarget`, so production passes `stderr()`/`hidden()` and tests
  pass `term_like(InMemoryTerm)`. Kept out of `domain/` because it does terminal
  I/O.
- **`src/commands/lint.rs`** — thin glue: build the view, tick it in the wave loop,
  clear it before `emit()`. The view is **always** constructed (a hidden target
  when not interactive) so the glue path is exercised on every run.

## Testing strategy

Interactive output is hard to test because the live path only activates under a
real TTY, and the test harness (`assert_cmd`) spawns children over pipes
([assert_cmd#138](https://github.com/assert-rs/assert_cmd/issues/138)). Every
mature tool therefore splits the feature into independently-testable halves:
**renderer correctness** (no terminal needed) and **mode selection** (a pure
function + a couple of real end-to-end checks). Jest does exactly this —
`isInteractive` is its own tiny tested module and `DefaultReporter` is
snapshot-tested with the flag forced both ways.

llmlint's layers map onto the existing tiers (`tests/AGENTS.md`):

| Layer | Tier | In `just check`? | New deps |
|---|---|---|---|
| Predicate (`ProgressChoice::resolve`, `is_ci`, `is_agent`) | unit, table-driven | ✅ | none |
| Renderer frames + self-erase | unit, `indicatif` `InMemoryTerm` (wraps `vt100`) | ✅ | `indicatif` (dev feature `in_memory`) |
| No-leak + report-unchanged when piped/agent | existing e2e (`assert_cmd` + mock-oneharness) | ✅ | none |
| Real interactive PTY round-trip (incl. Windows ConPTY) | new PTY tier / CI (like `win-color`) | separate workflow | `portable-pty` + `vt100` (dev) |

1. **Predicate → unit, table-driven.** One row per scenario: `CLAUDECODE=1` + TTY →
   off; `TERM=dumb` → off; Codex `NO_COLOR=1`+`TERM=dumb` → off; real TTY, nothing
   set → on; `CI=true` → off. This is the `isInteractive.ts` equivalent and covers
   the logic that a PTY test can only spot-check.

2. **Renderer → unit via `InMemoryTerm`.** indicatif ships an
   [`InMemoryTerm`](https://docs.rs/indicatif/latest/indicatif/trait.TermLike.html)
   (a thin wrapper around the `vt100` terminal emulator) usable as a draw target via
   `ProgressDrawTarget::term_like`. Feed the `ProgressView` synthetic rule/judge
   events, then assert on `contents_formatted()` — the *rendered screen grid*, not
   raw escape bytes — at each transition (queued → running → passed/failed) and,
   critically, that on completion **the grid is empty** (no leftover fragments). The
   cursor-up / erase-line sequences are interpreted by the emulator, so the assertion
   is on the final screen and is timing-independent. Determinism rules: no
   `enable_steady_tick` (drive `.tick()`/updates manually), keep elapsed time out of
   the asserted template, fix the terminal width.

3. **No-leak safety → existing e2e tier.** This is the most important guarantee and
   is *free* because `assert_cmd` already gives non-TTY pipes: a piped run must emit
   **zero cursor-control bytes** (`\x1b[`, bare `\r`) on stdout **and** stderr, and
   the stdout report must be byte-identical to today. A second journey sets an agent
   env var (`CLAUDECODE=1`) and asserts the run stays plain. See the journeys added
   to `tests/AGENTS.md`.

4. **Real interactive path → PTY tier.** To exercise the `isTTY == true` branch end
   to end, spawn the real binary under a pseudo-terminal (`portable-pty` — WezTerm's
   crate, cross-platform incl. **ConPTY on Windows**), capture the byte stream, parse
   it with `vt100`, and assert the live view appeared, advanced, and fully erased
   itself. This mirrors the existing **`win-color` gate**, which already reads a real
   Windows console buffer back — same philosophy, one more surface. It runs in its
   own workflow (like `win-color`/`live`), not the fast gate.

### Windows

The whole plan is cross-platform by construction: `std::io::IsTerminal` (Windows
msys/cygwin aware), `indicatif`/`console` (enables VT on modern consoles, falls
back to Win32 console calls on legacy ones), and `anstream` for the report (already
covered by the `win-color` gate). One caveat baked into the predicate: **do not gate
on `TERM` being *present*** — Windows often leaves `TERM` unset, so treating absent
as "not a terminal" would wrongly suppress the view on a real Windows console. The
PTY tier's Windows lane asserts the animation renders *and fully erases* on a real
Windows console — the only genuinely new Windows surface the feature adds.

## Demo GIF

The static screenshot tooling (`scripts/screenshots.sh`, deterministic SVGs) can't
capture an *animation*. A separate, informational GIF (like the screenshots, never
gated) is the README hero. `scripts/demo-gif.py` drives the **real release binary**
against the mock-oneharness fixture (same as the screenshots) for its data — the
rules, verdicts, and final report are genuine CLI output — then reconstructs the
exact frames the view draws (the same glyphs, words, and status colors as
`commands/progress.rs`) and renders them with the **vendored, pinned JetBrains Mono
font** the SVG screenshots already use. This avoids a heavyweight screen-recording
stack (no `ttyd`/`ffmpeg`): the only dependency is Pillow, so it is self-contained
and reproducible. The GIF is **not** hash-gated (a GIF isn't byte-reproducible
across Pillow versions); regenerate it on demand with `just screenshots-gif` and
commit `docs/screenshots/demo.gif`.

One rendering gotcha worth recording: JetBrains Mono has no distinct braille or
quadrant-circle glyphs (they collapse to a single fallback), so a braille spinner
would freeze in the GIF. The demo uses the quadrant-*block* spinner (`▖▘▝▗`), whose
four glyphs render distinctly.

## Non-goals / deferred

- **A separate token-lean agent report.** Unnecessary — the default report already
  is one. Documented here so it isn't re-litigated; Vitest's `agent` reporter is the
  precedent if we ever want passes suppressed harder than the default already does.
- **Per-judge streaming inside a rule.** The view resolves at rule granularity;
  finer streaming isn't worth the coupling to oneharness internals.

## Sources

Agent harnesses: Claude Code Bash output limits & `CLAUDECODE`
(<https://code.claude.com/docs/en/env-vars>, <https://github.com/anthropics/claude-code/issues/9881>,
<https://github.com/anthropics/claude-code/issues/48375>); Codex exec (stderr
progress / stdout result), `unified-exec` `NO_COLOR`+`TERM=dumb`, PTY
(<https://developers.openai.com/codex/noninteractive>,
<https://github.com/openai/codex/issues/6426>); Cursor terminal & `CURSOR_AGENT`
(<https://cursor.com/docs/agent/tools/terminal>). Test runners: Vitest reporters &
4.1 agent reporter (<https://vitest.dev/guide/reporters>,
<https://vitest.dev/blog/vitest-4-1.html>), Jest `isInteractive`
(<https://github.com/jestjs/jest/blob/main/packages/jest-util/src/isInteractive.ts>),
pnpm reporters (<https://pnpm.io/cli/install>), ripgrep/ESLint/pytest machine
formats. Rust: `indicatif` draw targets & `InMemoryTerm`
(<https://docs.rs/indicatif/latest/indicatif/struct.ProgressDrawTarget.html>),
`std::io::IsTerminal` (<https://doc.rust-lang.org/std/io/trait.IsTerminal.html>),
`anstream`/`anstyle-query` (<https://docs.rs/anstream>), `portable-pty` /
`expectrl` for PTY testing. Conventions: `NO_COLOR` (<https://no-color.org>),
`std-env` `isAgent` (<https://github.com/unjs/std-env>), clig.dev
(<https://clig.dev>).
