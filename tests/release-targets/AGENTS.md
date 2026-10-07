# tests/release-targets/AGENTS.md

## Release declaration + probe (`release_targets.rs`)

`release-targets.toml` is parsed by a restatement of the canonical release-target
schema and held to the real release configuration in both directions: the
published set is *derived* from the workflows' publish steps (`cargo publish`,
`pypa/gh-action-pypi-publish`, a publishing `release-plz release`) and the
manifests' package names, and an unrecognised registry publish (`npm publish`,
`twine upload`, …) fails the gate. The drift check is also driven over the real
`release.yml`/manifests edited to disagree. `scripts/release-probe.sh` runs for
real, with a cleared environment, against a stand-in registry on localhost
(`LLMLINT_RELEASE_PROBE_{CRATES,PYPI}_URL`): a served version for each target,
404 → no release yet, error status / refused connection / a stalled connection
(within the 60s bound) → not answered, planted credentials never sent, and any
identifier but the two declared ids → not answered. Unix-only. Two tests need
the network and are `#[ignore]`-d (`just test-release-targets`): the probe
against the live registries, and the restated schema reconciled against onevcs's
canonical implementation (constants, version-1 key sets, rule expressions, and
that `schema_version = 1` is still in the range it reads).

## Running

The offline tests are this project's `test` target, in the gate tiers. `just
test-release-targets` runs the two network tests (its `network` target); the
`Release targets` workflow (`.github/workflows/release-targets.yml`) runs it on a
change to what they read and weekly. `release-targets.toml` is the canonical
release-target declaration (schema defined in onevcs's `docs/contract.md`) other
repositories wait on; its target ids and short names (`crate:llmlint`/`crate`,
`pypi:llmlint-cli`/`cli`) are named by consumers' plans, so never rename them
unilaterally.
