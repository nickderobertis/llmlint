//! `llmlint check-ignores`: validate the *structure* of inline `llmlint: ignore`
//! directives in the target files — fast, deterministic, and free of any model
//! or oneharness call. It is the same pre-flight `lint` runs, lifted into its own
//! command so it can sit in the tight static-check loop next to fmt/clippy and
//! catch a typo'd or reason-less ignore long before a (paid) judge run.

use crate::cli::CheckIgnoresArgs;
use crate::commands::ignores;
use crate::domain::config::validate;
use crate::errors::{Error, Result};
use crate::io::{configfs, files};

pub fn run(args: CheckIgnoresArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };

    // Explicit CLI files relevance-gate the subtree cascade exactly as `lint`
    // does, so the two never disagree about which files are in scope.
    let loaded = configfs::load_with_targets(&args.config, &cwd, &args.files)?;
    let scopes = loaded.scopes;
    let config = loaded.config;
    validate(&config)?;

    // Explicit CLI files, when given, are the file universe each rule's globs
    // intersect with (else the globs walk the tree) — the same scoping a lint run
    // uses, so the fast static check and the full run never disagree. (No `--diff`
    // on this command, so the diff-narrowed universe never applies.)
    let cli_files = files::from_cli(&cwd, &args.files);
    let universe = ignores::file_universe(&cwd, &cli_files, None, None)?;
    let targets = ignores::target_files(&cwd, &config, &scopes, universe.as_deref())?;
    let known = ignores::known_rules(&config);

    // A malformed directive is a hard exit-2 error (`Error::IgnoreDirective`),
    // exactly as it is for a lint run; a clean scan is quiet but for one line so
    // the command is usable in a noisy pre-commit loop.
    ignores::check(&cwd, &targets, &known)?;
    println!(
        "llmlint: ignore directives OK ({} file(s) scanned)",
        targets.len()
    );
    Ok(0)
}
