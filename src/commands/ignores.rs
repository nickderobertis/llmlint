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
use crate::io::plugins::PluginResolution;

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

/// Turn the explicit `FILES` llmlint cannot read as target files into one exit-2
/// error listing every bad path, in [`files::read_text`]'s own
/// `reading <path>: <os error>` wording, so every command says the same thing
/// about the same input. An empty list is clean.
///
/// A passed path is a per-invocation assertion that *this file* is there to be
/// judged (or scanned), so a path that names nothing is a usage error rather than
/// a quietly smaller run — the file globs would otherwise take over and the run
/// would report a confident pass over a set the caller never named. This matters
/// most now that CLI files **intersect** the globs (see [`resolve_files`]): a
/// mistyped path simply falls out of every intersection, so nothing downstream
/// would ever try to read it.
pub fn reject_unresolved(unresolved: Vec<files::Unresolved>) -> Result<()> {
    if unresolved.is_empty() {
        return Ok(());
    }
    Err(Error::Io(
        unresolved
            .into_iter()
            .map(|u| u.message)
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// Resolve the union of every evaluated rule's target files (relative to `cwd`),
/// de-duplicated and ordered. This mirrors what `lint` would scan: rules
/// disabled with `relevance: false` never run, so their files are not scanned
/// here either. `cli_files`, when non-empty, intersects each rule's globs exactly
/// as it does for a lint run (see [`resolve_files`]). `scopes`
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

/// The target files for a single rule, applying the same resolution a lint run
/// uses. A rule's **effective filter** is its own `files` when it declares one,
/// else its [`RuleScope`] fallback (the filter of the config that defined it).
/// Glob filters root at the rule's config directory (`scope.dir`) so a nested
/// config's globs mean "relative to me", while resolved paths stay relative to
/// `cwd`.
///
/// Explicit CLI files **intersect** that filter rather than replacing it: a rule's
/// globs say which files the *rule* is about, the passed files say which files
/// *this run* is about, and a file must satisfy both to be judged. Naming a subset
/// therefore narrows every rule — including a glob-scoped one, which used to
/// discard the passed set and pull its whole glob match back in — and a rule whose
/// globs match nothing in the subset resolves to no files, so it is reported
/// skipped for this run rather than judged or errored. An empty `include` set
/// still means "every file", so a config with no `files` block judges exactly what
/// was passed.
///
/// `global_exclude` is the session-level top-level `files.exclude` (cwd-rooted). It
/// is applied as a **hard denylist in every mode**: a per-rule `files.include`
/// narrows *within* the allowed set — it can never resurrect a path the top-level
/// (or the rule's own config-level) `exclude` denied (issue #128) — and neither can
/// naming a path on the command line.
pub fn resolve_files(
    cwd: &Path,
    rule: &Rule,
    cli_files: &[PathBuf],
    scope: &RuleScope,
    global_exclude: &[String],
) -> Result<Vec<PathBuf>> {
    let filter = rule.files.as_ref().unwrap_or(&scope.files);
    // A per-rule filter selects *within* its config's allowed set, so that config's
    // `exclude` (co-rooted at `scope.dir`) is layered on top of it; the fallback
    // filter is that config's own, so its excludes are already inside `filter`.
    let scoped_exclude: &[String] = if rule.files.is_some() {
        &scope.files.exclude
    } else {
        &[]
    };

    if cli_files.is_empty() {
        return files::resolve_scoped_excluding(
            &scope.dir,
            cwd,
            filter,
            scoped_exclude,
            global_exclude,
        );
    }

    // Explicit CLI files are bounded to the rule's directory scope first: a subtree
    // config's rule must not be judged against a passed file outside its directory.
    // Keep only the files under `scope.dir` (reported cwd-relative, as given); a
    // rule with no passed file under its scope resolves to nothing and is skipped —
    // the same "consolidated up from each leaf" trimming a discovery run does.
    let scoped = scope_cli_files(cwd, &scope.dir, cli_files);
    // Then intersect with the rule's own `include` globs, so a passed file is judged
    // only by the rules that are about it.
    let included = files::keep_included(&scope.dir, cwd, &scoped, &filter.include)?;
    // The `exclude` denylist still wins even over an explicitly-named file (config
    // `files.exclude`, an env exclude, or `--exclude`), so a passed path that matches
    // it is dropped — an include never resurrects it.
    let mut scoped_denies = filter.exclude.clone();
    scoped_denies.extend(scoped_exclude.iter().cloned());
    files::drop_excluded(&scope.dir, cwd, &included, &scoped_denies, global_exclude)
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
///
/// `plugins` is what this run's `plugins:` URLs resolved to. When a directive
/// names a rule nothing declares, they are named in the message (see
/// `unknown_rule_trailer`), because a plugin's cached copy being older than its
/// pin promises is what makes a correct suppression read as a typo.
pub fn check(
    cwd: &Path,
    targets: &BTreeSet<PathBuf>,
    known: &BTreeSet<&str>,
    plugins: &[PluginResolution],
) -> Result<()> {
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
        Err(Error::IgnoreDirective {
            plugins: unknown_rule_trailer(&problems, plugins),
            problems: problems.join("\n"),
        })
    }
}

/// The trailer a directive problem set earns when one of its problems is a rule
/// nothing declares: which plugins are loaded and what version each resolved to,
/// since a plugin resolving older than its pin promises is what makes a correct
/// suppression read as a typo. Empty when the run loaded no plugins, or when no
/// problem is an unknown rule.
fn unknown_rule_trailer(problems: &[String], plugins: &[PluginResolution]) -> String {
    if plugins.is_empty() || !problems.iter().any(|p| p.contains("unknown rule")) {
        return String::new();
    }
    let lines: Vec<String> = plugins
        .iter()
        .map(|p| format!("  {}", p.to_human()))
        .collect();
    format!(
        "\nloaded plugins (a rule a plugin declares is missing when its cached copy \
         is older than the pin promises):\n{}\n\
         inspect with `llmlint plugins`, then `llmlint plugins clear` (or \
         LLMLINT_PLUGIN_REFRESH=1) to refetch.",
        lines.join("\n")
    )
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
    fn cli_files_intersect_a_rules_own_globs() {
        // A glob-scoped rule judges only the files in *both* sets: the passed
        // subset narrows it (it no longer pulls `src/b.rs` back in), and a passed
        // file its globs aren't about (`docs/x.md`) is not judged by it.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "src/a.rs");
        touch(cwd, "src/b.rs");
        touch(cwd, "docs/x.md");
        let rule = rule_named(
            "rust_rule",
            Some(FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            }),
        );
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter::default(),
        };
        let cli = vec![PathBuf::from("src/a.rs"), PathBuf::from("docs/x.md")];
        let files = resolve_files(cwd, &rule, &cli, &scope, &[]).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn cli_files_with_no_glob_overlap_resolve_to_nothing() {
        // An empty intersection is an empty file set — the planner reports the rule
        // skipped for this run; it is never an error and never a judged file.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "docs/x.md");
        // A file the rule's globs *do* match exists — it is out of the passed set,
        // so the rule still resolves to nothing rather than judging it.
        touch(cwd, "src/a.rs");
        let rule = rule_named(
            "rust_rule",
            Some(FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            }),
        );
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter::default(),
        };
        let files = resolve_files(cwd, &rule, &[PathBuf::from("docs/x.md")], &scope, &[]).unwrap();
        assert!(files.is_empty(), "expected no files, got {files:?}");
    }

    #[test]
    fn cli_files_intersect_the_configs_globs_for_a_rule_without_its_own() {
        // A rule with no `files` is scoped by its config's globs, so the same
        // intersection applies — a passed file outside them is not judged.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "src/a.rs");
        touch(cwd, "README.md");
        let rule = rule_named("r", None);
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            },
        };
        let cli = vec![PathBuf::from("README.md"), PathBuf::from("src/a.rs")];
        let files = resolve_files(cwd, &rule, &cli, &scope, &[]).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn cli_files_are_kept_whole_when_no_include_globs_are_configured() {
        // An empty `include` means "every file", so a config with no `files` block
        // judges exactly what was passed — today's behavior, unchanged.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "README.md");
        let rule = rule_named("r", None);
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter::default(),
        };
        let cli = vec![PathBuf::from("README.md")];
        let files = resolve_files(cwd, &rule, &cli, &scope, &[]).unwrap();
        assert_eq!(files, cli);
    }

    #[test]
    fn a_rules_own_exclude_drops_a_passed_file_it_denies() {
        // The rule's own `exclude` is a denylist over the intersection too, so
        // naming a file it denies never resurrects it.
        let tmp = tempdir().unwrap();
        let cwd = tmp.path();
        touch(cwd, "src/a.rs");
        touch(cwd, "src/gen.rs");
        let rule = rule_named(
            "rust_rule",
            Some(FileFilter {
                include: vec!["src/**".into()],
                exclude: vec!["**/gen.rs".into()],
            }),
        );
        let scope = RuleScope {
            dir: cwd.to_path_buf(),
            files: FileFilter::default(),
        };
        let cli = vec![PathBuf::from("src/a.rs"), PathBuf::from("src/gen.rs")];
        let files = resolve_files(cwd, &rule, &cli, &scope, &[]).unwrap();
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
