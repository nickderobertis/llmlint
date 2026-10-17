# tests/fingerprint-version/AGENTS.md

The proof that llmlint's own version is part of `llmlint fingerprint`: the
workspace's built `llmlint` and a second build from a scratch copy with only
`Cargo.toml`'s version changed must print different digests.

- **Never a gate-tier target.** The second build costs minutes, so the target is
  `version-proof`, not `test`: no `nx affected -t test` / `run-many -t test` sweep
  selects it. Run it with `just test-fingerprint-version`.
- **Its workflow is its only scheduler.** `.github/workflows/fingerprint-version.yml`
  runs it on pull requests and pushes to `main` whose changes touch its inputs.
  Those `paths` must equal the target's Nx `inputs` in `project.json`, which the
  ci-workflows journey `fingerprint_version_workflow_paths_are_the_version_proof_inputs`
  enforces — change both together. The inputs are only what can change the
  result: crate sources and assets, manifest, lockfile, build config, the
  stand-in harness, and this test.
