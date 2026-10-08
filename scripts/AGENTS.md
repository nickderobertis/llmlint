# scripts/AGENTS.md — the repo-tooling project

The repository's own tooling: machine setup and the session hook
(`setup*.sh`, `session-setup.sh`), the Nx wrapper and gate plumbing (`nx`,
`nx-base.sh`, `nx-tier.sh`, `bun.sh`), the pinned actionlint installer and the
workflow lint the ci-workflows project runs, the llmlint-tier installer, and two
files whose paths are published contracts and so stay here: `install.sh` (the
README's `curl | sh` URL) and `release-probe.sh` (named by
`release-targets.toml`). The offline journeys over them are `tests/tooling.rs`,
this project's `test` target (run under the shell coverage driver); shfmt and
shellcheck over every script here run in its `format` and `lint`, beside cargo
fmt and clippy. `setup-lib.sh` is sourced only by scripts in this project, so it
needs no project or edge of its own.

- **Gate plumbing.** Each gate recipe runs `nx affected` or `nx run-many --all`
  itself, as `nx-tier.sh` decides: no flag prints the explicit base `nx-base.sh`
  derives (the affected tier), `--all` prints `all` (the full sweep), and any
  other flag is refused before a target runs. `nx-base.sh` takes `NX_BASE` only
  as a plain ref name or a commit SHA that resolves, else the merge base with
  `origin/main` — never Nx's implicit default. `nx` runs Nx on the
  `.tool-versions` bun with no daemon, cloud or TUI. A new gate recipe follows
  the same shape (the `GateRepo` journeys in `tests/tooling.rs` drive the real
  recipes), never a loop of its own.

## Workflow lint (`scripts/lint-workflows.sh`, `scripts/install-actionlint.sh`)

These scripts' journeys run the real scripts with real `curl`/`tar`/`install`;
only what a test cannot own is stood in — the host (`uname`), rhysd's release
server (a local release tree over `file://`), and, where the journey is about
how the lint script treats actionlint's answer, the `actionlint` binary itself.
The installer's supported-platform matrix is read from the script, so the pin
file and the journeys cannot drift from what it can choose.
