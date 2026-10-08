# tools/AGENTS.md — the workspace project

Repo-level checks over the whole project graph. It depends on every other
project, so any change selects it. Its `lint` and `test` are uncached (about a
second each): they judge the graph's edges, and Nx does not count a change to
another project's `implicitDependencies` as a change to a task's inputs, so a
cached pass could replay over a newly forbidden edge.

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
  `lint-workflows`, that `just check` runs every gate target, and that an edit
  selects exactly its owning project and that project's dependents
  (`affected.test.mjs`, asking `nx show projects --affected --files=`).
- **Nested Nx goes through `scripts/nx`.** The checker and these tests compute
  the graph from inside an Nx task; `scripts/nx` is the one place that runs Nx
  with its plugins loaded in-process and no 10-second worker load window (a cold
  Windows runner missed it, and `nx graph` exited with empty stderr). A
  `gate.test.mjs` test fails on any task code or target command that starts Nx
  another way.
- **Supply chain.** `supply-chain` (`cargo deny` + `cargo machete`) is
  workspace-wide and needs a network advisory DB, so it is outside the gate tiers:
  `just deps-check` and CI's `deny` job run it.
