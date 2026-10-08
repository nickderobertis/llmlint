# assets/AGENTS.md — the config-lint-plugin project

The published contract directory: `config_lint.yml` (llmlint's own versioned
plugin, which consumers pin `@1` and fetch from `main` by path),
`llmlint.schema.json` (the config schema consumers reference from `main`,
generated from the crate's types and held to them by the
`committed_asset_matches_generated_schema` unit test), and the default and init
templates the binary embeds. Never move or rename a file here: the paths are
published. A contract project: it depends on nothing, and the `llmlint` crate
depends on it.

- `just check-version-bump [base=origin/main]` — dogfood `check-version-bump` on
  llmlint's own versioned plugin (`assets/config_lint.yml`), failing if it changed
  vs the base without a `version:` bump. Out of `check` (it needs a base ref +
  network to resolve it); CI runs it against the PR base. It is this project's
  `check-version-bump` target, which runs the `llmlint` binary as a tool (no
  graph edge).
- Bump the plugin's `version` and the `@1` pins (`init.llmlint.yml`, README,
  `CONFIG_LINT` in the e2e suite) together when its checks change incompatibly.
