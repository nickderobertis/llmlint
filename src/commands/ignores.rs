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

use crate::domain::config::{Config, FileFilter, RelevanceMode, Rule};
use crate::domain::ignore;
use crate::errors::{Error, Result};
use crate::io::configfs::{self, RuleScope};
use crate::io::files;

/// After the session `files` filter is overridden post-load (by the env layer or
/// a CLI flag), re-point the **cwd-rooted** rule scopes at the new filter. Rule
/// scopes capture their config's `files` at load time and are the fallback filter
/// for a rule with no per-rule `files`, so without this a session-level
/// `files.include` (or `--exclude`) override would change the reported config but
/// not what those rules actually target. Only scopes rooted at `cwd` (the session
/// config's own rules) are re-pointed — a subtree or ancestor rule keeps its own
/// directory-scoped filter, which the session-level override does not govern.
pub fn retarget_session_scopes(
    scopes: &mut BTreeMap<String, RuleScope>,
    cwd: &Path,
    after: &FileFilter,
) {
    for scope in scopes.values_mut() {
        if scope.dir == cwd {
            scope.files = after.clone();
        }
    }
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
        for f in resolve_files(cwd, rule, cli_files, scope, &config.files.exclude)? {
            out.insert(f);
        }
    }
    Ok(out)
}

/// The target files for a single rule, applying the same precedence a lint run
/// uses: a per-rule `files` filter wins, then explicit CLI files, then the rule's
/// [`RuleScope`] fallback filter. Glob filters root at the rule's config directory
/// (`scope.dir`) so a nested config's globs mean "relative to me", while resolved
/// paths stay relative to `cwd`.
///
/// `global_exclude` is the session-level top-level `files.exclude` (cwd-rooted). It
/// is applied as a **hard denylist in every glob mode**: a per-rule `files.include`
/// narrows *within* the allowed set — it can never resurrect a path the top-level
/// (or the rule's own config-level) `exclude` denied (issue #128). Explicit CLI
/// files stay a direct request and are not filtered by it.
pub fn resolve_files(
    cwd: &Path,
    rule: &Rule,
    cli_files: &[PathBuf],
    scope: &RuleScope,
    global_exclude: &[String],
) -> Result<Vec<PathBuf>> {
    if let Some(f) = &rule.files {
        // A per-rule `files.include` selects *within* the allowed set. Layer both
        // the rule's own config-level `exclude` (`scope.files.exclude`, co-rooted
        // at `scope.dir`) and the session-level global `exclude` on top, so neither
        // can be overridden by the rule's include.
        return files::resolve_scoped_excluding(
            &scope.dir,
            cwd,
            f,
            &scope.files.exclude,
            global_exclude,
        );
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
    // The rule falls back to its config's `files` (whose own `exclude` is already
    // in the filter); still layer the session-level global `exclude` on top so a
    // subtree rule honors an ancestor's top-level exclude too.
    files::resolve_scoped_excluding(&scope.dir, cwd, &scope.files, &[], global_exclude)
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

    use crate::domain::config::FileFilter;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

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

    #[test]
    fn resolve_files_applies_global_exclude_over_a_rule_include() {
        // Issue #128: a rule's `files.include` must not resurrect a path the
        // top-level `files.exclude` denied.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "tests/unit.rs");
        touch(cwd, "tests/fixtures/big.json");
        let rule = rule_named(
            "judge_tests",
            Some(FileFilter {
                include: vec!["**/tests/**".into()],
                exclude: vec![],
            }),
        );
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter {
                include: vec![],
                exclude: vec!["tests/fixtures/**".into()],
            },
        };
        let files = resolve_files(cwd, &rule, &[], &scope, &["tests/fixtures/**".into()]).unwrap();
        assert_eq!(files, vec![PathBuf::from("tests/unit.rs")]);
    }

    #[test]
    fn resolve_files_fallback_still_honors_global_exclude() {
        // A rule with no own `files` falls back to its config's filter; the
        // session-level global exclude still drops the excluded path.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "src/a.rs");
        touch(cwd, "vendored/gen.rs");
        let rule = rule_named("r", None);
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter {
                include: vec!["**/*.rs".into()],
                exclude: vec![],
            },
        };
        let files = resolve_files(cwd, &rule, &[], &scope, &["vendored/**".into()]).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/a.rs")]);
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
