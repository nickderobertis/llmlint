# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). This file is
maintained by release-plz; do not hand-edit released sections.

## [Unreleased]

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
