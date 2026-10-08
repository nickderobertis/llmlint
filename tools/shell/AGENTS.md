# tools/shell/AGENTS.md — the shell-tools project

The shell toolchain every project owning shell scripts runs:

- `shell.sh format [--write] FILE...` and `shell.sh lint FILE...` are what each
  such project's `format` and `lint` targets call, beside cargo fmt and clippy.
  They run shfmt (with no style flags, so the `.editorconfig` style applies) and
  shellcheck, and refuse a tool off the justfile pin.
- `shell.sh files` is the tree's one shell-script discovery: tracked or
  unignored files ending `.sh`/`.bash`, or starting with a sh-family shebang.
  The coverage gate measures exactly that set, and `tools/tests/gate.test.mjs`
  holds every file in it to some project's `format` and `lint`. A new script is
  checked once its project's glob matches it. Unlike format and lint, the
  discovery runs on every leg (`workspace:test` reads it on macOS and Windows),
  so it must behave identically under BSD tools and bash 3.2: it reads files
  as bytes (`LC_ALL=C` — under a UTF-8 locale macOS's `tr` refuses a GIF's
  bytes, which once ended the scan part-way), uses no `[[ =~ ]]`, and spawns
  no process per file — one batched POSIX `awk` reads every first line, since
  a `head`/`tr` per file took the Windows scan past bun's 5-second test limit.
  Its journeys run on Linux only, so BSD `tr` is stood in, and a journey counts
  the scan's spawns.
- `install-shell-tools.sh` installs the pins from the prebuilt releases, checked
  against `shell-tools.sha256`; refresh that file whenever a pin moves.
  `just bootstrap` and `just shell-tools` run it.
- `LLMLINT_SHELL_TOOLS=off` (`just check-portable`, the macOS/Windows `cross`
  jobs) stands format, lint and the install down with a notice; the Linux gate
  enforces them. `check-portable` also leaves this project's own `test` to
  Linux: its journeys need the real pinned shfmt and shellcheck, which the cross
  jobs do not install, and assert the installer's Linux host behaviour (on macOS
  a missing `TMPDIR` is no failure).
- No project depends on this one by edge, because bench and live may depend on
  no tooling project. Each consumer names `shell.sh` as a cached input instead
  (nx.json's `shellTools`/`shellCoverage`), and Nx counts that as touching the
  consumer: an edit here re-runs every shell project's format, lint and coverage
  (`tools/tests/affected.test.mjs`).
- The journeys (`tests/*.test.mjs`, this project's `test`) run shell.sh with the
  real pinned tools. They run the installer against a stand-in release tree over
  `file://`, with only `uname`, the release servers and the tools' own binaries
  stood in.
