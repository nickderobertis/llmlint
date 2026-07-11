//! `llmlint check-version-bump`: verify that every versioned config file that
//! changed vs a base also bumped its top-level `version:`. Fast, deterministic,
//! and free of any model or oneharness call — it belongs in the static-check loop
//! next to fmt/clippy and `check-ignores`.
//!
//! The pure decision (does a file declare a version? did its diff bump it?) lives
//! in [`crate::domain::versionbump`]; this module owns the I/O — resolving which
//! config files are versioned and computing their diffs through the same
//! backend-agnostic [`crate::io::diff`] provider `lint --diff` uses.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cli::CheckVersionBumpArgs;
use crate::commands::{ignores, lint};
use crate::domain::versionbump;
use crate::errors::{Error, Result};
use crate::io::diff::DiffBackend;
use crate::io::{configfs, diff, files};

pub fn run(args: CheckVersionBumpArgs) -> Result<i32> {
    let cwd = lint::resolve_cwd(&args.cwd)?;
    let cli_files = files::from_cli(&cwd, &args.files);
    let backend = args.diff.unwrap_or_default();
    let checked = check(&cwd, &cli_files, backend, args.diff_base.clone())?;
    println!("llmlint: versioned configs OK ({checked} file(s) checked)");
    Ok(0)
}

/// Resolve the versioned config files, diff them against the base, and reject any
/// that changed without bumping their `version`. Returns the number of versioned
/// files checked. Shared with `llmlint validate` so the standalone command and
/// the chained one can never disagree.
///
/// The candidate set is the explicit `cli_files` when given (so an oddly-named
/// plugin config — e.g. this repo's own `assets/config_lint.yml`, which no
/// standard glob matches — can be guarded by path), else every discovered llmlint
/// config file (the same globs `lint-config` uses). Either way it is filtered to
/// the files that actually declare a top-level `version:`: a file with no version
/// has nothing to bump and is not checked. When no versioned file is in scope the
/// check is a clean no-op that never touches git.
pub(crate) fn check(
    cwd: &Path,
    cli_files: &[PathBuf],
    backend: DiffBackend,
    base: Option<String>,
) -> Result<usize> {
    let versioned = versioned_targets(cwd, cli_files)?;
    if versioned.is_empty() {
        return Ok(0);
    }

    // Only now (there is something to check) do we touch the VCS, so a project
    // with no versioned config never requires a git work tree.
    let files_vec: Vec<PathBuf> = versioned.iter().cloned().collect();
    let diffs = diff::provider(backend, base).diffs(cwd, &files_vec)?;

    let mut offenders: Vec<String> = Vec::new();
    for rel in &versioned {
        // An unchanged file is absent from the diff map (`None`); a changed one
        // must have bumped its version.
        if versionbump::changed_without_bump(diffs.get(rel).map(String::as_str)) {
            offenders.push(format!("  {}", files::to_slash(rel)));
        }
    }

    if offenders.is_empty() {
        Ok(versioned.len())
    } else {
        Err(Error::VersionBump(offenders.join("\n")))
    }
}

/// The set of versioned config files (relative to `cwd`) in scope. See [`check`].
fn versioned_targets(cwd: &Path, cli_files: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let candidates: BTreeSet<PathBuf> = if cli_files.is_empty() {
        // Default: the llmlint config files in the tree. Reuse the bundled
        // config-lint plugin's globs (`**/llmlint.yml`, `**/*.llmlint.yml`, …) —
        // the same "what is an llmlint config file" definition `lint-config` uses,
        // so the two never disagree. Resolves offline from the embedded copy, so
        // it needs no project config and no network.
        let loaded = configfs::load_config_lint(cwd)?;
        ignores::target_files(cwd, &loaded.config, &loaded.scopes, &[])?
    } else {
        cli_files.iter().cloned().collect()
    };

    // Keep only the files that actually declare a top-level `version:` — those are
    // the published plugins a change could silently alter under a fixed pin.
    let mut out = BTreeSet::new();
    for rel in candidates {
        if let Some(text) = files::read_text(cwd, &rel)? {
            if versionbump::declares_version(&text) {
                out.insert(rel);
            }
        }
    }
    Ok(out)
}
