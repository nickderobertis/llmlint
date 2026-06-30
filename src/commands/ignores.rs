//! Shared inline-`llmlint: ignore` directive validation: resolve which target
//! files to scan and check the *structure* of any directives they carry.
//!
//! Two commands lean on this: the `lint` pre-flight (so a typo'd ignore fails
//! before any judge call) and the standalone `check-ignores` command (so the
//! same check can run in the fast, deterministic linter loop without touching a
//! model or oneharness). Routing both through one module keeps the two from ever
//! disagreeing about what a well-formed directive is.
//!
//! Honoring a well-formed directive is **not** done here — that is the judge's
//! job, specified in the default prompt template. This module only enforces that
//! each directive names specific, configured rule(s) and a reason; see
//! [`crate::domain::ignore`] for the pure parser.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::domain::config::{Agent, Config, RelevanceMode, Rule};
use crate::domain::ignore;
use crate::errors::{Error, Result};
use crate::io::configfs::{self, RuleScope};
use crate::io::files;

/// The configured rule names — the set a directive may legitimately reference.
/// A directive may name any configured rule, not just the ones a given run
/// selects, so this is always the full config.
pub fn known_rules(config: &Config) -> BTreeSet<&str> {
    config.rules.iter().map(|r| r.name.as_str()).collect()
}

/// Resolve the union of every evaluated rule's target files (relative to `cwd`),
/// de-duplicated and ordered. This mirrors what `lint` would scan: rules
/// disabled with `relevance: false` never run, so their files are not scanned
/// here either. `cli_files`, when non-empty, overrides the config globs exactly
/// as it does for a lint run (per-rule / per-agent `files` still win). `scopes`
/// are the per-rule directory scopes from [`crate::io::configfs::Loaded`], so a
/// nested config's globs root at its own directory exactly as they do for a lint
/// run — the two never disagree about which files carry directives.
pub fn target_files(
    cwd: &Path,
    config: &Config,
    scopes: &BTreeMap<String, RuleScope>,
    cli_files: &[PathBuf],
) -> Result<BTreeSet<PathBuf>> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    for rule in &config.rules {
        if matches!(rule.relevance_mode(), RelevanceMode::Never) {
            continue;
        }
        let agent_name = rule.agent.clone().unwrap_or_else(|| "default".to_string());
        let agent = config.agent_or_default(&agent_name);
        let fallback;
        let scope = match scopes.get(&rule.name) {
            Some(s) => s,
            None => {
                fallback = RuleScope {
                    dir: cwd.to_path_buf(),
                    files: config.files.clone(),
                };
                &fallback
            }
        };
        for f in resolve_files(cwd, rule, &agent, cli_files, scope)? {
            out.insert(f);
        }
    }
    Ok(out)
}

/// The target files for a single rule, applying the same precedence a lint run
/// uses: a per-rule `files` filter wins, then the agent's, then explicit CLI
/// files, then the rule's [`RuleScope`] fallback filter. Glob filters root at the
/// rule's config directory (`scope.dir`) so a nested config's globs mean "relative
/// to me", while resolved paths stay relative to `cwd`.
pub fn resolve_files(
    cwd: &Path,
    rule: &Rule,
    agent: &Agent,
    cli_files: &[PathBuf],
    scope: &RuleScope,
) -> Result<Vec<PathBuf>> {
    if let Some(f) = &rule.files {
        return files::resolve_scoped(&scope.dir, cwd, f);
    }
    if let Some(f) = &agent.files {
        return files::resolve_scoped(&scope.dir, cwd, f);
    }
    if !cli_files.is_empty() {
        // Explicit CLI files override the rule's globs, but they are still bounded
        // to the rule's directory scope: a subtree config's rule must not be judged
        // against a passed file outside its directory. Keep only the files under
        // `scope.dir` (reported cwd-relative, as given); a rule with no passed file
        // under its scope resolves to nothing and is skipped — the same
        // "consolidated up from each leaf" trimming a discovery run does.
        return Ok(scope_cli_files(cwd, &scope.dir, cli_files));
    }
    files::resolve_scoped(&scope.dir, cwd, &scope.files)
}

/// Keep the explicit CLI files that fall under `dir` (a rule's directory scope),
/// preserving their given (cwd-relative) spelling. A file is under `dir` when its
/// absolutized, lexically-normalized path is prefixed by `dir`; an ancestor-scoped
/// rule (e.g. the cwd config, whose `dir` is `cwd` or above) keeps every passed
/// file under `cwd`, so a flat single-config run is unchanged.
fn scope_cli_files(cwd: &Path, dir: &Path, cli_files: &[PathBuf]) -> Vec<PathBuf> {
    cli_files
        .iter()
        .filter(|f| {
            let abs = if f.is_absolute() {
                (*f).clone()
            } else {
                cwd.join(f)
            };
            configfs::normalize(&abs).starts_with(dir)
        })
        .cloned()
        .collect()
}

/// Scan each file (read once, relative to `cwd`) for inline `llmlint: ignore`
/// directives and reject any whose structure is malformed — no rule named, an
/// unknown/invalid rule, a missing reason, or unbalanced block pairing.
/// Non-UTF-8 (binary) files can't carry a text directive and are skipped. Every
/// problem across every file is collected into one [`Error::IgnoreDirective`]
/// (exit 2) so a single run surfaces all the fixes; an empty file set is clean.
pub fn check(cwd: &Path, targets: &BTreeSet<PathBuf>, known: &BTreeSet<&str>) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    for rel in targets {
        let Some(text) = files::read_text(cwd, rel)? else {
            continue;
        };
        for p in ignore::validate(&text, known) {
            problems.push(format!(
                "  {}:{}: {}",
                files::to_slash(rel),
                p.line,
                p.message
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::IgnoreDirective(problems.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scope_cli_files_bounds_to_the_scope_dir_relative_and_absolute() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let dir = cwd.join("backend");
        let abs_in = cwd.join("backend/x.rs"); // absolute, under dir → kept
        let abs_out = cwd.join("other/y.rs"); // absolute, outside → dropped
        let files = vec![
            PathBuf::from("backend/svc.rs"), // relative, under dir → kept
            PathBuf::from("app.rs"),         // relative, outside → dropped
            abs_in.clone(),
            abs_out,
        ];
        let kept = scope_cli_files(cwd, &dir, &files);
        assert_eq!(kept, vec![PathBuf::from("backend/svc.rs"), abs_in]);
    }

    #[test]
    fn scope_cli_files_ancestor_scope_keeps_all_files_under_cwd() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        // A cwd/ancestor scope (the root config) keeps every passed file, so a flat
        // single-config run is unchanged by the per-rule scoping.
        let files = vec![PathBuf::from("a.rs"), PathBuf::from("sub/b.rs")];
        assert_eq!(scope_cli_files(cwd, cwd, &files), files);
    }
}
