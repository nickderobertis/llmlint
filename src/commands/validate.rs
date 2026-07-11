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
//! Every step is free of any model/oneharness call, so `validate` sits in the tight
//! static loop next to fmt/clippy — the one command that runs all of llmlint's own
//! deterministic checks together. The LLM-as-judge passes stay in `lint` /
//! `lint-config`. Each check also has a standalone command (`check-ignores`,
//! `check-version-bump`); `validate` routes through the *same* shared functions, so
//! it can never disagree with running them one by one.

use crate::cli::ValidateArgs;
use crate::commands::{ignores, lint, version_bump};
use crate::domain::config::validate;
use crate::errors::Result;
use crate::io::configfs;

pub fn run(args: ValidateArgs) -> Result<i32> {
    let cwd = lint::resolve_cwd(&args.cwd)?;

    // 1. Config structure. This also yields the target files (and their subtree
    // scopes) for step 2, so the ignore scan sees exactly what a lint run would.
    let loaded = configfs::load(&args.config, &cwd)?;
    validate(&loaded.config)?;

    // 2. Ignore-directive structure over every target file (no CLI narrowing:
    // `validate` is a whole-project gate).
    let targets = ignores::target_files(&cwd, &loaded.config, &loaded.scopes, &[])?;
    let known = ignores::known_rules(&loaded.config);
    ignores::check(&cwd, &targets, &known)?;

    // 3. Version bumps for the discovered versioned config files (an empty CLI file
    // list makes `check` default to the discovered llmlint configs; it touches git
    // only if any of them declares a `version`).
    let backend = args.diff.unwrap_or_default();
    let versioned = version_bump::check(&cwd, &[], backend, args.diff_base.clone())?;

    println!(
        "llmlint: static checks passed ({} file(s) scanned for ignores, \
         {versioned} versioned config(s) checked)",
        targets.len()
    );
    Ok(0)
}
