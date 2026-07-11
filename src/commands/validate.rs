//! `llmlint validate`: run every deterministic, model-free check in one pass —
//! the fast static gate for an llmlint project. It chains, cheapest first:
//!
//! 1. **Config structure** — load the discovered (or explicit `--config`) config
//!    and run the same structural validation a lint does (unique rule names, valid
//!    identifiers, resolvable agents). A bad config is a hard exit-2 error.
//! 2. **Ignore directives** — the deterministic `check-ignores` structure check
//!    over the target files.
//! 3. **Version bumps** — the deterministic `check-version-bump` check: a versioned
//!    config that changed vs the base must bump its `version`.
//!
//! Its file-selection surface mirrors the `lint` command so the static gate can be
//! scoped the same way a review is: explicit **`FILES`** narrow every check to those
//! files (relevance-gating the subtree cascade exactly as a lint run does), and
//! **`--diff`** restricts the ignore-directive scan to the changed files (their
//! intersection with the globs), just like `lint --diff`. `--diff-base` (folded with
//! a config `diff_base`) sets the base for both the ignore-scan restriction and the
//! always-on version-bump diff.
//!
//! Every step is free of any model/oneharness call, so `validate` sits in the tight
//! static loop next to fmt/clippy — the one command that runs all of llmlint's own
//! deterministic checks together. The LLM-as-judge passes stay in `lint` /
//! `lint-config`. Each check also has a standalone command (`check-ignores`,
//! `check-version-bump`); `validate` routes through the *same* shared functions, so
//! it can never disagree with running them one by one.

use std::path::PathBuf;

use crate::cli::ValidateArgs;
use crate::commands::{ignores, lint, version_bump};
use crate::domain::config::validate;
use crate::errors::Result;
use crate::io::{configfs, diff, files};

pub fn run(args: ValidateArgs) -> Result<i32> {
    let cwd = lint::resolve_cwd(&args.cwd)?;

    // 1. Config structure. Explicit `FILES` relevance-gate the subtree cascade the
    // same way a lint run does — validating one area never loads an unrelated
    // subtree's config. This load also yields the target files (and their subtree
    // scopes) for step 2, so the ignore scan sees exactly what a lint run would.
    let loaded = configfs::load_with_targets(&args.config, &cwd, &args.files)?;
    validate(&loaded.config)?;

    let cli_files = files::from_cli(&cwd, &args.files);
    // The effective diff base: `--diff-base` wins over a config `diff_base`, exactly
    // as `lint`'s `apply_cli_overrides` resolves it. `None` leaves each backend's
    // built-in default (`HEAD` for git).
    let diff_base = args
        .diff_base
        .clone()
        .or_else(|| loaded.config.diff_base.clone());

    // 2. Ignore-directive structure over the target files. `FILES` narrow the set
    // (per-rule/per-agent `files` still win); under `--diff` it is further
    // restricted to the changed files — the intersection with the globs — mirroring
    // `lint --diff`, so an empty intersection is a clean pass.
    let mut targets = ignores::target_files(&cwd, &loaded.config, &loaded.scopes, &cli_files)?;
    if let Some(backend) = args.diff {
        let files_vec: Vec<PathBuf> = targets.iter().cloned().collect();
        let diffs = diff::provider(backend, diff_base.clone()).diffs(&cwd, &files_vec)?;
        // Keep a file only when it changed (has a diff) *and* still exists on disk:
        // an unchanged file has no diff and a deleted path has no file to scan.
        targets.retain(|f| diffs.contains_key(f) && cwd.join(f).exists());
    }
    let known = ignores::known_rules(&loaded.config);
    ignores::check(&cwd, &targets, &known)?;

    // 3. Version bumps. `FILES`, when given, are the candidate set (the same
    // narrowing the other two steps get); otherwise `check` defaults to the
    // discovered llmlint configs. Either way it touches git only if any candidate
    // declares a `version`. (For an oddly-named plugin config in an otherwise
    // unconfigured project, use the standalone `check-version-bump`, which needs no
    // discoverable project config.)
    let backend = args.diff.unwrap_or_default();
    let versioned = version_bump::check(&cwd, &cli_files, backend, diff_base)?;

    let scanned = if args.diff.is_some() {
        "changed file(s) scanned for ignores"
    } else {
        "file(s) scanned for ignores"
    };
    println!(
        "llmlint: static checks passed ({} {scanned}, \
         {versioned} versioned config(s) checked)",
        targets.len()
    );
    Ok(0)
}
