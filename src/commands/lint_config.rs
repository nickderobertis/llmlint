//! `llmlint lint-config`: lint llmlint config files with the bundled config-lint
//! rules, without requiring the user to add the plugin to their own config.
//!
//! This is the `lint` command with the bundled config-lint plugin included by
//! default (see [`crate::io::assets`] and [`crate::io::configfs::load_config_lint`]).
//! It runs in two phases, cheapest first:
//!
//! 1. **Comment check** — the deterministic, model-free validation of inline
//!    `llmlint: ignore` directive *structure* in the target config files (the same
//!    check `check-ignores` and the `lint` pre-flight run). A malformed directive
//!    is a hard exit-2 error here, so a typo fails fast before any (paid) judge run.
//! 2. **Config lint** — the LLM-as-judge pass over each config's rules, reusing the
//!    full lint engine (`lint::run_loaded`).
//!
//! The config-lint agent scopes its rules to config-file globs, so the run always
//! targets configuration rather than source.

use crate::cli::LintConfigArgs;
use crate::commands::{ignores, lint};
use crate::domain::config::validate;
use crate::errors::Result;
use crate::io::{configfs, files};

pub fn run(args: LintConfigArgs) -> Result<i32> {
    let cwd = lint::resolve_cwd(&args.cwd)?;

    // Load the bundled config-lint plugin as this run's config — no discovery, no
    // network. It resolves offline from the embedded copy.
    let loaded = configfs::load_config_lint(&cwd)?;
    validate(&loaded.config)?;

    // Phase 1 — the comment check. Resolve the config-lint target files exactly as
    // the lint run will (its agent globs pick the llmlint config files, unless the
    // CLI passed explicit ones) and reject any malformed ignore directive before
    // spending a judge call. `lint::run_loaded` re-runs this same check as its
    // pre-flight, so the fast static phase and the full run can never disagree.
    let cli_files = files::from_cli(&cwd, &args.files);
    let targets = ignores::target_files(&cwd, &loaded.config, &loaded.scopes, &cli_files)?;
    let known = ignores::known_rules(&loaded.config);
    ignores::check(&cwd, &targets, &known)?;

    // Phase 2 — the LLM-as-judge config lint, through the shared engine.
    lint::run_loaded(loaded, cwd, args.into_lint_args(), "lint-config")
}
