# .github/AGENTS.md — the ci-workflows project

The GitHub Actions workflows, the CI-only script beside them
(`scripts/ci-gate.sh`), and the tests that hold them (`tests/workflows.rs`, the
drift gates; `tests/ci_gate.rs`, the routing and release-verdict journeys). Its
`lint-workflows` target is actionlint over every workflow, its `lint-sh`
shellchecks `scripts/`.

- **One routing script.** `scripts/ci-gate.sh tier` decides the tier a CI run
  owes and `scripts/ci-gate.sh verdict` decides whether a release may ship; both
  read the same release-PR prefix and sweep-job list, so what counts as a sweep
  is defined once. The workflows call it, never restate its decisions in
  `if:` expressions. Its journeys feed it real event payloads in a scratch repo
  and answer GitHub's API with a stand-in `gh`; a change to either decision gets
  a journey there.
- **Spell repeated lists out.** Workflows spell out repeated lists rather than
  using YAML anchors/aliases, which actionlint has rejected in trigger filters
  (issue #201).

## Required status-check contexts (`workflows/`)

`PR_CONTEXTS` in `tests/workflows.rs` is the fixed context contract, and it moves
only with AGENTS.md's required-checks list (a journey holds that list to it) and
the branch protection that governance applies. A job condition is allowed on a
contract job only when it is true on every PR (`PR_TRUE_CONDITIONS`). screencomp's
inner `report` job name is known only for the pinned `VISUAL_DOCS_REUSABLE`, so a
pin bump fails until that name is re-confirmed.
