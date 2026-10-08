# .githooks/AGENTS.md — the git-hooks project

`pre-push` is the local enforcement point (enable once per clone:
`git config core.hooksPath .githooks`). `.githooks/pre-push` runs `just
lint-llm-validate` on every push before the visual guard: a failure blocks, a
missing llmlint warns and skips. The visual guard then re-captures and classifies
the screenshots when a guarded path changed. The hook's journeys are the
screenshots project's (`screenshots/tests/visual_guard.rs`), since the guard
drives that project's capture scripts; a change to the hook's logic gets one
there. This project's `format` (shfmt) and `lint` (shellcheck) check the hook,
through `tools/shell/shell.sh`; it has no `test` of its own, and the hook's
shell coverage comes from the screenshots project's journeys.
