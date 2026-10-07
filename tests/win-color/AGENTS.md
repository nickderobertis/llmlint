# tests/win-color/AGENTS.md

## Windows color-rendering tier (`win-console-color.ps1`)

Color has two separable questions: does llmlint **emit** the right ANSI, and does
a terminal **render** it? The first is platform-independent and already covered —
the hermetic e2e (`color_is_off_when_piped_but_forced_by_color_always`) and the
screenshot tooling both assert the escape bytes. The second is the one that can
actually break on Windows: a legacy console (no virtual-terminal processing)
prints bare ANSI as `<-[31m` garbage. llmlint routes its report through anstream's
`AutoStream` (enable VT, else translate to Win32 console attribute calls) so it
renders; this tier proves that end result.

- **What it covers that nothing else does:** a *real Windows console* interpreting
  llmlint's color. `tests/win-color/win-console-color.ps1` drives the **release binary**
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

## Running

`just win-color` runs this project's `win-color` target: it builds the release
binary and the mock, then runs `win-console-color.ps1`. It is Windows-only, so
the Linux gate tiers never run it; `.github/workflows/win-color.yml` does, on
`windows-latest`, on every PR.
