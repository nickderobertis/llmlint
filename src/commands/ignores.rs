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

use crate::domain::config::{Config, RelevanceMode, Rule};
use crate::domain::ignore;
use crate::errors::{Error, Result};
use crate::io::configfs::RuleScope;
use crate::io::diff::{self, DiffBackend};
use crate::io::files;

/// The explicit file universe a run is scoped to, or `None` for "walk the tree":
///
///   * the CLI file list, when non-empty;
///   * else, under `--diff`, the changed files the diff backend reports against
///     `diff_base` (so a diff run reviews only what changed);
///   * else `None` — no explicit set, so each rule's globs resolve by walking.
///
/// A rule **intersects** its globs with this set rather than override it (see
/// [`resolve_files`]), so a scoped run never drags in violations from files the
/// change never touched. Shared by `lint`, `check-ignores`, and `lint-config` so
/// they all scope to the same files. `cli_files` should already be normalized via
/// [`files::from_cli`].
pub fn file_universe(
    cwd: &Path,
    cli_files: &[PathBuf],
    diff: Option<DiffBackend>,
    diff_base: Option<String>,
) -> Result<Option<Vec<PathBuf>>> {
    if !cli_files.is_empty() {
        return Ok(Some(cli_files.to_vec()));
    }
    if let Some(backend) = diff {
        return Ok(Some(diff::provider(backend, diff_base).changed_files(cwd)?));
    }
    Ok(None)
}

/// The configured rule names — the set a directive may legitimately reference.
/// A directive may name any configured rule, not just the ones a given run
/// selects, so this is always the full config.
pub fn known_rules(config: &Config) -> BTreeSet<&str> {
    config.rules.iter().map(|r| r.name.as_str()).collect()
}

/// Resolve the union of every evaluated rule's target files (relative to `cwd`),
/// de-duplicated and ordered. This mirrors what `lint` would scan: rules
/// disabled with `relevance: false` never run, so their files are not scanned
/// here either. `universe`, when `Some`, is the explicit file set (CLI files or a
/// `--diff`'s changed files) each rule's globs **intersect** with — exactly as a
/// lint run scopes its files, so the two never disagree about which files carry
/// directives. `scopes` are the per-rule directory scopes from
/// [`crate::io::configfs::Loaded`], so a nested config's globs root at its own
/// directory just as they do for a lint run.
pub fn target_files(
    cwd: &Path,
    config: &Config,
    scopes: &BTreeMap<String, RuleScope>,
    universe: Option<&[PathBuf]>,
) -> Result<BTreeSet<PathBuf>> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    for rule in &config.rules {
        if matches!(rule.relevance_mode(), RelevanceMode::Never) {
            continue;
        }
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
        for f in resolve_files(cwd, rule, universe, scope)? {
            out.insert(f);
        }
    }
    Ok(out)
}

/// The target files for a single rule. The rule's effective globs are its own
/// per-rule `files` filter when set, else its origin config's fallback filter
/// (`scope.files`); glob filters root at the rule's config directory (`scope.dir`)
/// so a nested config's globs mean "relative to me", while resolved paths stay
/// relative to `cwd`.
///
/// `universe` selects between two modes:
/// - `None` — no explicit file set: resolve the globs by **walking** the rule's
///   scope (the whole tree under it).
/// - `Some(files)` — an explicit file set (the CLI file list, or the changed files
///   from a `--diff` run): **intersect** it with the rule's globs instead of
///   letting the globs re-expand across the whole tree. Bounded to the rule's
///   directory scope, so a passed file outside it is never judged by that rule. An
///   empty set means nothing is in scope (a `--diff` with no changes), never a
///   match-all re-expansion — which is what keeps a scoped run from dragging in
///   violations from files the change never touched.
pub fn resolve_files(
    cwd: &Path,
    rule: &Rule,
    universe: Option<&[PathBuf]>,
    scope: &RuleScope,
) -> Result<Vec<PathBuf>> {
    let filter = rule.files.as_ref().unwrap_or(&scope.files);
    match universe {
        Some(files) => files::filter_scoped(&scope.dir, cwd, filter, files),
        None => files::resolve_scoped(&scope.dir, cwd, filter),
    }
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
    use crate::domain::config::FileFilter;
    use std::fs;
    use tempfile::tempdir;

    fn rule_named(name: &str, files: Option<FileFilter>) -> Rule {
        Rule {
            name: name.into(),
            description: "true when ok; false otherwise.".into(),
            r#override: false,
            agent: None,
            judges: None,
            files,
            rationale: None,
            relevance: None,
            require_line_attribution: None,
        }
    }

    fn scope_at(dir: &Path, filter: FileFilter) -> RuleScope {
        RuleScope {
            dir: dir.to_path_buf(),
            files: filter,
        }
    }

    #[test]
    fn resolve_files_intersects_the_universe_with_the_rule_globs() {
        // An explicit universe is narrowed by the rule's own `files` filter: a
        // passed file matching the filter is kept; one that doesn't is dropped, so
        // the glob never re-expands beyond the passed set.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let rule = rule_named(
            "r",
            Some(FileFilter {
                include: vec!["**/*.yml".into()],
                exclude: vec![],
            }),
        );
        let scope = scope_at(cwd, FileFilter::default());
        let universe = vec![PathBuf::from("a.yml"), PathBuf::from("b.rs")];
        let out = resolve_files(cwd, &rule, Some(&universe), &scope).unwrap();
        assert_eq!(out, vec![PathBuf::from("a.yml")]);
    }

    #[test]
    fn resolve_files_falls_back_to_config_filter_when_rule_has_none() {
        // With no per-rule `files`, the scope's (config-level) filter narrows the
        // universe — the config `include` intersects the passed files too.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let rule = rule_named("r", None);
        let scope = scope_at(
            cwd,
            FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            },
        );
        let universe = vec![PathBuf::from("src/a.rs"), PathBuf::from("README.md")];
        let out = resolve_files(cwd, &rule, Some(&universe), &scope).unwrap();
        assert_eq!(out, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn resolve_files_empty_universe_scopes_to_nothing() {
        // A `--diff` with no changes hands an empty universe: the rule resolves to
        // nothing (and is skipped), never a match-all walk.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let rule = rule_named("r", None);
        let scope = scope_at(cwd, FileFilter::default());
        let out = resolve_files(cwd, &rule, Some(&[]), &scope).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn resolve_files_no_universe_walks_the_scope() {
        // Without an explicit universe the globs resolve by walking the tree.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        fs::write(cwd.join("a.rs"), "x").unwrap();
        fs::write(cwd.join("b.rs"), "x").unwrap();
        let rule = rule_named("r", None);
        let scope = scope_at(cwd, FileFilter::default());
        let out = resolve_files(cwd, &rule, None, &scope).unwrap();
        assert_eq!(out, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }

    #[test]
    fn resolve_files_universe_bounds_to_the_scope_dir() {
        // A subtree rule (scope under `backend/`) only sees passed files under its
        // directory — a file outside is never judged by it.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        let rule = rule_named("r", None);
        let scope = scope_at(&cwd.join("backend"), FileFilter::default());
        let universe = vec![PathBuf::from("backend/x.rs"), PathBuf::from("app.rs")];
        let out = resolve_files(cwd, &rule, Some(&universe), &scope).unwrap();
        assert_eq!(out, vec![PathBuf::from("backend/x.rs")]);
    }
}
