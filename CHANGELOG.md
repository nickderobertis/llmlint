# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). This file is
maintained by release-plz; do not hand-edit released sections.

## [Unreleased]

## [0.3.12](https://github.com/nickderobertis/llmlint/compare/v0.3.11...v0.3.12) - 2026-07-10

### Fixed

- *(files)* top-level exclude is a hard denylist a rule include can't override ([#129](https://github.com/nickderobertis/llmlint/pull/129))

## [0.3.11](https://github.com/nickderobertis/llmlint/compare/v0.3.10...v0.3.11) - 2026-07-10

### Added

- *(diff)* restrict --diff to the changed files, skipping empty diffs ([#126](https://github.com/nickderobertis/llmlint/pull/126))

## [0.3.10](https://github.com/nickderobertis/llmlint/compare/v0.3.9...v0.3.10) - 2026-07-10

### Added

- *(plan)* token-weighted batching, ignore-aware scope/diff trimming, and plan explanation ([#122](https://github.com/nickderobertis/llmlint/pull/122))

## [0.3.9](https://github.com/nickderobertis/llmlint/compare/v0.3.8...v0.3.9) - 2026-07-07

### Fixed

- raise default per-judge timeout to 600s (10 minutes) ([#120](https://github.com/nickderobertis/llmlint/pull/120))

## [0.3.8](https://github.com/nickderobertis/llmlint/compare/v0.3.7...v0.3.8) - 2026-07-05

### Added

- *(config-lint)* prefer files globs over relevance for path-scoped rules ([#118](https://github.com/nickderobertis/llmlint/pull/118))

## [0.3.7](https://github.com/nickderobertis/llmlint/compare/v0.3.6...v0.3.7) - 2026-07-04

### Added

- log full run results and add the `history` command ([#116](https://github.com/nickderobertis/llmlint/pull/116))

## [0.3.6](https://github.com/nickderobertis/llmlint/compare/v0.3.5...v0.3.6) - 2026-07-03

### Fixed

- fall back to a oneharness beside the llmlint executable ([#114](https://github.com/nickderobertis/llmlint/pull/114))

## [0.3.5](https://github.com/nickderobertis/llmlint/compare/v0.3.4...v0.3.5) - 2026-07-03

### Fixed

- depend on oneharness-cli so pip install is a complete setup ([#112](https://github.com/nickderobertis/llmlint/pull/112))

## [0.3.4](https://github.com/nickderobertis/llmlint/compare/v0.3.3...v0.3.4) - 2026-07-03

### Fixed

- publish PyPI wheels as llmlint-cli ([#109](https://github.com/nickderobertis/llmlint/pull/109))

## [0.3.3](https://github.com/nickderobertis/llmlint/compare/v0.3.2...v0.3.3) - 2026-07-03

### Added

- distribute prebuilt binaries as PyPI wheels ([#107](https://github.com/nickderobertis/llmlint/pull/107))

## [0.3.2](https://github.com/nickderobertis/llmlint/compare/v0.3.1...v0.3.2) - 2026-07-03

### Added

- sign releases with Sigstore and make install.sh mirror-configurable ([#104](https://github.com/nickderobertis/llmlint/pull/104))

## [0.3.1](https://github.com/nickderobertis/llmlint/compare/v0.3.0...v0.3.1) - 2026-07-02

### Documentation

- document batching and cost vs performance in README ([#102](https://github.com/nickderobertis/llmlint/pull/102))

## [0.3.0](https://github.com/nickderobertis/llmlint/compare/v0.2.32...v0.3.0) - 2026-07-02

### Fixed

- [**breaking**] scope agents to their subtree and remove agent.files ([#99](https://github.com/nickderobertis/llmlint/pull/99))

## [0.2.32](https://github.com/nickderobertis/llmlint/compare/v0.2.31...v0.2.32) - 2026-07-02

### Added

- *(config-lint)* flag verbose rule descriptions and relevance conditions ([#97](https://github.com/nickderobertis/llmlint/pull/97))

## [0.2.31](https://github.com/nickderobertis/llmlint/compare/v0.2.30...v0.2.31) - 2026-07-01

### Added

- *(files)* default to linting the whole tree when no files block is set ([#95](https://github.com/nickderobertis/llmlint/pull/95))

## [0.2.30](https://github.com/nickderobertis/llmlint/compare/v0.2.29...v0.2.30) - 2026-07-01

### Documentation

- complete the prompt-template variables table in the README ([#93](https://github.com/nickderobertis/llmlint/pull/93))

## [0.2.29](https://github.com/nickderobertis/llmlint/compare/v0.2.28...v0.2.29) - 2026-07-01

### Added

- *(lint)* interactive live-progress view with agent-safe suppression ([#90](https://github.com/nickderobertis/llmlint/pull/90))

## [0.2.28](https://github.com/nickderobertis/llmlint/compare/v0.2.27...v0.2.28) - 2026-07-01

### Added

- add lint-config subcommand for linting llmlint configs ([#89](https://github.com/nickderobertis/llmlint/pull/89))

## [0.2.27](https://github.com/nickderobertis/llmlint/compare/v0.2.26...v0.2.27) - 2026-07-01

### Documentation

- surface not-relevant in headline report, split multi-judge screenshot ([#87](https://github.com/nickderobertis/llmlint/pull/87))

## [0.2.26](https://github.com/nickderobertis/llmlint/compare/v0.2.25...v0.2.26) - 2026-06-30

### Documentation

- don't require every rule to state both true and false ([#84](https://github.com/nickderobertis/llmlint/pull/84))

## [0.2.25](https://github.com/nickderobertis/llmlint/compare/v0.2.24...v0.2.25) - 2026-06-30

### Added

- scope rules per file with validation, rework, and deterministic ignores ([#80](https://github.com/nickderobertis/llmlint/pull/80))

## [0.2.24](https://github.com/nickderobertis/llmlint/compare/v0.2.23...v0.2.24) - 2026-06-30

### Added

- *(diff)* diff against a branch/ref/range via --diff-base and a config diff_base default ([#79](https://github.com/nickderobertis/llmlint/pull/79))

## [0.2.23](https://github.com/nickderobertis/llmlint/compare/v0.2.22...v0.2.23) - 2026-06-30

### Added

- add require_line_attribution rule option ([#76](https://github.com/nickderobertis/llmlint/pull/76))

### Fixed

- keep the --diff template block purely additive (no-diff prompt unchanged) ([#77](https://github.com/nickderobertis/llmlint/pull/77))

## [0.2.22](https://github.com/nickderobertis/llmlint/compare/v0.2.21...v0.2.22) - 2026-06-30

### Added

- add --diff to surface changed-line diffs in the judge prompt ([#74](https://github.com/nickderobertis/llmlint/pull/74))

## [0.2.21](https://github.com/nickderobertis/llmlint/compare/v0.2.20...v0.2.21) - 2026-06-30

### Fixed

- *(config)* relevance-gate the subtree cascade by the linted files ([#72](https://github.com/nickderobertis/llmlint/pull/72))

## [0.2.20](https://github.com/nickderobertis/llmlint/compare/v0.2.19...v0.2.20) - 2026-06-30

### Added

- *(config)* nest config discovery up the tree and cascade into subtrees ([#70](https://github.com/nickderobertis/llmlint/pull/70))

## [0.2.19](https://github.com/nickderobertis/llmlint/compare/v0.2.18...v0.2.19) - 2026-06-30

### Added

- *(config)* trace where each config item is defined ([#68](https://github.com/nickderobertis/llmlint/pull/68))

## [0.2.18](https://github.com/nickderobertis/llmlint/compare/v0.2.17...v0.2.18) - 2026-06-30

### Added

- add standalone check-ignores command for the fast linter loop ([#67](https://github.com/nickderobertis/llmlint/pull/67))
- balance rules evenly across judge batches ([#65](https://github.com/nickderobertis/llmlint/pull/65))

## [0.2.17](https://github.com/nickderobertis/llmlint/compare/v0.2.16...v0.2.17) - 2026-06-27

### Added

- require oneharness >= 0.3.0 and run it in read-only mode ([#63](https://github.com/nickderobertis/llmlint/pull/63))

## [0.2.16](https://github.com/nickderobertis/llmlint/compare/v0.2.15...v0.2.16) - 2026-06-26

### Added

- support block-scoped ignore directives ([#60](https://github.com/nickderobertis/llmlint/pull/60))

## [0.2.15](https://github.com/nickderobertis/llmlint/compare/v0.2.14...v0.2.15) - 2026-06-26

### Documentation

- add terminal screenshots for every command + lint verbosities ([#58](https://github.com/nickderobertis/llmlint/pull/58))

## [0.2.14](https://github.com/nickderobertis/llmlint/compare/v0.2.13...v0.2.14) - 2026-06-26

### Added

- render report color correctly on Windows + verify it on a real console ([#55](https://github.com/nickderobertis/llmlint/pull/55))

## [0.2.13](https://github.com/nickderobertis/llmlint/compare/v0.2.12...v0.2.13) - 2026-06-26

### Added

- add strict inline `llmlint: ignore` directives ([#51](https://github.com/nickderobertis/llmlint/pull/51))

## [0.2.12](https://github.com/nickderobertis/llmlint/compare/v0.2.11...v0.2.12) - 2026-06-26

### Added

- override plugin rules by name with `override: true` ([#52](https://github.com/nickderobertis/llmlint/pull/52))

## [0.2.11](https://github.com/nickderobertis/llmlint/compare/v0.2.10...v0.2.11) - 2026-06-26

### Added

- add relevance gating to the rule framework ([#49](https://github.com/nickderobertis/llmlint/pull/49))

## [0.2.10](https://github.com/nickderobertis/llmlint/compare/v0.2.9...v0.2.10) - 2026-06-25

### Documentation

- use lowercase json true/false in rule examples ([#47](https://github.com/nickderobertis/llmlint/pull/47))

## [0.2.9](https://github.com/nickderobertis/llmlint/compare/v0.2.8...v0.2.9) - 2026-06-25

### Documentation

- let screenshots carry the report output in the README ([#45](https://github.com/nickderobertis/llmlint/pull/45))

## [0.2.8](https://github.com/nickderobertis/llmlint/compare/v0.2.7...v0.2.8) - 2026-06-25

### Added

- colorized report and terminal screenshots via screencomp ([#42](https://github.com/nickderobertis/llmlint/pull/42))

## [0.2.7](https://github.com/nickderobertis/llmlint/compare/v0.2.6...v0.2.7) - 2026-06-25

### Added

- rationales option with strict verdict ordering and per-judge breakdown ([#40](https://github.com/nickderobertis/llmlint/pull/40))

## [0.2.6](https://github.com/nickderobertis/llmlint/compare/v0.2.5...v0.2.6) - 2026-06-25

### Fixed

- *(lint)* error on unknown --rule/--agent targets instead of a false green ([#36](https://github.com/nickderobertis/llmlint/pull/36))

## [0.2.5](https://github.com/nickderobertis/llmlint/compare/v0.2.4...v0.2.5) - 2026-06-25

### Added

- *(init)* publish a config JSON Schema, derived from the model, and reference it from init configs ([#37](https://github.com/nickderobertis/llmlint/pull/37))

## [0.2.4](https://github.com/nickderobertis/llmlint/compare/v0.2.3...v0.2.4) - 2026-06-25

### Added

- concise lint output by default, with -v for full detail and oneharness debug ([#34](https://github.com/nickderobertis/llmlint/pull/34))

## [0.2.3](https://github.com/nickderobertis/llmlint/compare/v0.2.2...v0.2.3) - 2026-06-25

### Fixed

- *(plugins)* bound transitive plugin resolution at depth 100 ([#31](https://github.com/nickderobertis/llmlint/pull/31))

## [0.2.2](https://github.com/nickderobertis/llmlint/compare/v0.2.1...v0.2.2) - 2026-06-25

### Documentation

- document prompt template variables in README ([#27](https://github.com/nickderobertis/llmlint/pull/27))

## [0.2.1](https://github.com/nickderobertis/llmlint/compare/v0.2.0...v0.2.1) - 2026-06-25

### Added

- reject an even number of judges in config validation ([#26](https://github.com/nickderobertis/llmlint/pull/26))

## [0.2.0](https://github.com/nickderobertis/llmlint/compare/v0.1.8...v0.2.0) - 2026-06-24

### Added

- [**breaking**] plugins config key with versioned, cached URL fetching ([#24](https://github.com/nickderobertis/llmlint/pull/24))

## [0.1.5](https://github.com/nickderobertis/llmlint/compare/v0.1.4...v0.1.5) - 2026-06-23

### Added

- publish to crates.io on release with post-publish verification ([#14](https://github.com/nickderobertis/llmlint/pull/14))

## [0.1.3](https://github.com/nickderobertis/llmlint/compare/v0.1.2...v0.1.3) - 2026-06-23

### Added

- fall back to oneharness default harness when agent leaves it unset ([#9](https://github.com/nickderobertis/llmlint/pull/9))

## [0.1.2](https://github.com/nickderobertis/llmlint/compare/v0.1.1...v0.1.2) - 2026-06-23

### Added

- initial llmlint — LLM-as-judge linter (Rust CLI)

### Fixed

- render target file paths with forward slashes on all platforms ([#1](https://github.com/nickderobertis/llmlint/pull/1))

## [0.1.1](https://github.com/nickderobertis/llmlint/compare/v0.1.0...v0.1.1) - 2026-06-23

### Added

- initial llmlint — LLM-as-judge linter (Rust CLI)

### Fixed

- render target file paths with forward slashes on all platforms ([#1](https://github.com/nickderobertis/llmlint/pull/1))

## [0.1.0](https://github.com/nickderobertis/llmlint/releases/tag/v0.1.0) - 2026-06-23

### Added

- initial llmlint — LLM-as-judge linter (Rust CLI)

### Fixed

- render target file paths with forward slashes on all platforms ([#1](https://github.com/nickderobertis/llmlint/pull/1))
