# tools/AGENTS.md — the workspace project

Repo-level checks over the whole project graph. It depends on every other
project, so any change selects it; its targets read only `project.json` and
`Cargo.toml` files (and the scripts and justfile its tests check), so they replay
from the Nx cache when none of those changed.

- **Boundaries.** `check-project-boundaries.mjs` (this project's `lint`) reads
  every project's `type:*` tag and every edge — each Cargo path dependency between
  members and each Nx `implicitDependencies` entry, held equal to the graph `nx
  graph` resolves — and fails on an edge `project-boundaries.json` disallows, and
  on a Cargo edge Nx does not also know (an affected-detection hole). Change the
  policy file deliberately: its point is that no project the gate always reaches
  (the crate, the plugin contract, the fixture, the tooling) can depend on an
  expensive one, and no expensive project on another.
- **Tests.** `tests/*.test.mjs` (`bun test`, this project's `test`) drive the
  checker against real scratch Cargo workspaces, and check against the real graph
  that every shell script is under some project's `lint-sh`, the workflows under
  `lint-workflows`, and that `just check` runs every gate target.
- **Supply chain.** `supply-chain` (`cargo deny` + `cargo machete`) is
  workspace-wide and needs a network advisory DB, so it is outside the gate tiers:
  `just deps-check` and CI's `deny` job run it.
