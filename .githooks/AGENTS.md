# .githooks/AGENTS.md — the git-hooks project

`pre-push` is the local enforcement point (enable once per clone:
`git config core.hooksPath .githooks`). `.githooks/pre-push` runs `just
lint-llm-validate` on every push before the visual guard: a failure blocks, a
missing llmlint warns and skips. The visual guard then re-captures and classifies
the screenshots when a guarded path changed; its journeys, and the rules for
changing the hook, are in `screenshots/AGENTS.md` ("The pre-push visual guard").
This project's `lint-sh` shellchecks the hook.
