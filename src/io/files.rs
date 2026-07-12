//! Resolve a [`FileFilter`] to a concrete list of target files, and normalize
//! the explicit CLI file list. Uses `ignore` (gitignore-aware walk) + `globset`.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::domain::config::FileFilter;
use crate::errors::{Error, Result};

fn build_set(globs: &[String]) -> Result<Option<GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in globs {
        let glob =
            Glob::new(g).map_err(|e| Error::InvalidConfig(format!("invalid glob {g:?}: {e}")))?;
        b.add(glob);
    }
    let set = b
        .build()
        .map_err(|e| Error::InvalidConfig(format!("invalid glob set: {e}")))?;
    Ok(Some(set))
}

/// Resolve `filter` to a sorted, de-duplicated list of files **relative to
/// `root`**. An empty `include` set means **every file under `root`** (the
/// repo-wide default, so a config with no `files` block lints the whole tree
/// from `cwd` rather than nothing); `exclude` and `.gitignore` still apply.
pub fn resolve(root: &Path, filter: &FileFilter) -> Result<Vec<PathBuf>> {
    resolve_scoped(root, root, filter)
}

/// Resolve `filter` with two roots: globs are matched **relative to `glob_root`**
/// (a config's own directory, so a nested config's `*.txt` means
/// `<that dir>/*.txt`), while the returned paths are **relative to `out_root`**
/// (the run's cwd — what oneharness and the report speak). Only files under
/// *both* roots are considered, so the deeper of the two bounds the walk; if the
/// roots don't nest (neither contains the other) there is nothing in scope.
///
/// When `glob_root == out_root` this is the plain single-root case ([`resolve`]).
pub fn resolve_scoped(
    glob_root: &Path,
    out_root: &Path,
    filter: &FileFilter,
) -> Result<Vec<PathBuf>> {
    resolve_scoped_excluding(glob_root, out_root, filter, &[], &[])
}

/// Like [`resolve_scoped`], but with two extra exclude glob lists applied as a
/// **hard denylist** on top of `filter`: a path either list denies is dropped
/// even when `filter.include` matches it — an `include` can never resurrect an
/// excluded path (see issue #128). Both *add to* whatever `filter.exclude`
/// already subtracts.
///
/// - `scoped_exclude` is matched against the **`glob_root`-relative** path, so it
///   is a config-level `files.exclude` co-rooted with the rule's own globs (the
///   top-level `exclude` of the config that declared the rule).
/// - `global_exclude` is matched against the **`out_root`-relative** (cwd-relative)
///   path — the session-level `files.exclude`, which is cwd-rooted like every
///   session setting, so it applies uniformly across nested configs.
pub fn resolve_scoped_excluding(
    glob_root: &Path,
    out_root: &Path,
    filter: &FileFilter,
    scoped_exclude: &[String],
    global_exclude: &[String],
) -> Result<Vec<PathBuf>> {
    // An empty `include` set means "every file under the walk root": `build_set`
    // returns `None` for it, which the loop below reads as match-all (see the
    // `is_none_or`). This is the repo-wide default — a config with no `files`
    // block lints the whole tree from `cwd` instead of nothing. A present set
    // filters as usual; `exclude` (and the gitignore-aware walk) still subtract
    // from whatever `include` picks.
    let include = build_set(&filter.include)?;
    let exclude = build_set(&filter.exclude)?;
    // The two hard-denylist sets, applied on top of `filter` (see the doc above):
    // `scoped_exclude` rooted like the rule's globs, `global_exclude` cwd-rooted.
    let scoped_ex = build_set(scoped_exclude)?;
    let global_ex = build_set(global_exclude)?;

    // Walk the more specific (deeper) root so every visited file is under both;
    // if neither root contains the other the scopes don't overlap — no files.
    let walk_root = if out_root.starts_with(glob_root) {
        out_root
    } else if glob_root.starts_with(out_root) {
        glob_root
    } else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    // `hidden(false)` so dotfiles like `.llmlint.yml` are visited; `.gitignore`
    // is still respected so we never lint ignored/build files.
    for entry in WalkBuilder::new(walk_root).hidden(false).build() {
        let entry =
            entry.map_err(|e| Error::Io(format!("walking {}: {e}", walk_root.display())))?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        // Match globs against the config-dir-relative path; report cwd-relative.
        let (Ok(rel_glob), Ok(rel_out)) =
            (path.strip_prefix(glob_root), path.strip_prefix(out_root))
        else {
            continue;
        };
        // `exclude` wins over `include`, and the two hard denylists win over both:
        // include minus exclude, and an include never brings back an excluded path.
        let excluded = exclude.as_ref().is_some_and(|e| e.is_match(rel_glob))
            || scoped_ex.as_ref().is_some_and(|e| e.is_match(rel_glob))
            || global_ex.as_ref().is_some_and(|e| e.is_match(rel_out));
        let included = include.as_ref().is_none_or(|set| set.is_match(rel_glob));
        if included && !excluded {
            out.push(rel_out.to_path_buf());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Drop explicitly-passed files that either exclude denylist matches. Explicit
/// CLI files bypass glob *inclusion* (the user named them), but `exclude` is a
/// hard "never lint this" denylist that still wins — the same rule the
/// glob-selected set follows (issue #128) — so a config / env / `--exclude` glob
/// drops a passed path too. `scoped_exclude` is matched against the
/// `glob_root`-relative path, `global_exclude` against the `out_root`
/// (cwd)-relative path, exactly as in [`resolve_scoped_excluding`]. With no
/// exclude globs this is the identity.
pub fn drop_excluded(
    glob_root: &Path,
    out_root: &Path,
    files: &[PathBuf],
    scoped_exclude: &[String],
    global_exclude: &[String],
) -> Result<Vec<PathBuf>> {
    let scoped_ex = build_set(scoped_exclude)?;
    let global_ex = build_set(global_exclude)?;
    if scoped_ex.is_none() && global_ex.is_none() {
        return Ok(files.to_vec());
    }
    let mut out = Vec::new();
    for f in files {
        // The passed path is `out_root`-relative (or absolute). Normalize to an
        // absolute path, then match each denylist against its own rooting; a path
        // outside a root simply can't match that set.
        let abs = if f.is_absolute() {
            f.clone()
        } else {
            out_root.join(f)
        };
        let abs = crate::io::configfs::normalize(&abs);
        let scoped_hit = abs
            .strip_prefix(glob_root)
            .ok()
            .is_some_and(|r| scoped_ex.as_ref().is_some_and(|e| e.is_match(r)));
        let global_hit = abs
            .strip_prefix(out_root)
            .ok()
            .is_some_and(|r| global_ex.as_ref().is_some_and(|e| e.is_match(r)));
        if !scoped_hit && !global_hit {
            out.push(f.clone());
        }
    }
    Ok(out)
}

/// Read a target file (given relative to `root`) as UTF-8 text, for scanning
/// inline `llmlint: ignore` directives. Returns `Ok(None)` for a non-UTF-8
/// (binary) file — it can't carry a text directive, so it is skipped rather than
/// failing the run. A genuine read error (e.g. permissions) is propagated.
pub fn read_text(root: &Path, rel: &Path) -> Result<Option<String>> {
    let path = root.join(rel);
    let bytes =
        std::fs::read(&path).map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
    Ok(String::from_utf8(bytes).ok())
}

/// Render a (relative) path with forward slashes; see [`crate::domain::to_slash`].
/// Re-exported here next to the other path helpers so `commands`/`io` share one
/// spelling with the planner's per-rule file lists.
pub use crate::domain::to_slash;

/// Normalize explicit CLI file paths to be relative to `root` where possible
/// (tidier prompts and output); paths outside `root` are kept as given.
pub fn from_cli(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = files
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| f.clone())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    #[test]
    fn drop_excluded_filters_explicit_paths() {
        let cwd = Path::new("/proj");
        let files = vec![PathBuf::from("src/gen.rs"), PathBuf::from("src/keep.rs")];
        // A cwd-rooted (global) exclude drops the matching passed path...
        let kept = drop_excluded(cwd, cwd, &files, &[], &["**/gen.rs".into()]).unwrap();
        assert_eq!(kept, vec![PathBuf::from("src/keep.rs")]);
        // ...a scoped (glob-root-relative) exclude does too...
        let kept = drop_excluded(cwd, cwd, &files, &["**/gen.rs".into()], &[]).unwrap();
        assert_eq!(kept, vec![PathBuf::from("src/keep.rs")]);
        // ...and with no exclude globs it is the identity.
        assert_eq!(drop_excluded(cwd, cwd, &files, &[], &[]).unwrap(), files);
    }

    #[test]
    fn include_exclude_globs() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        touch(dir.path(), "src/b.rs");
        touch(dir.path(), "src/gen.rs");
        touch(dir.path(), "README.md");
        let filter = FileFilter {
            include: vec!["src/**/*.rs".into()],
            exclude: vec!["**/gen.rs".into()],
        };
        let files = resolve(dir.path(), &filter).unwrap();
        let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        assert!(names.iter().any(|n| n.ends_with("a.rs")));
        assert!(names.iter().any(|n| n.ends_with("b.rs")));
        assert!(!names.iter().any(|n| n.ends_with("gen.rs")));
        assert!(!names.iter().any(|n| n.ends_with("README.md")));
    }

    #[test]
    fn double_star_matches_root_level_file() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "llmlint.yml");
        touch(dir.path(), "nested/llmlint.yml");
        let filter = FileFilter {
            include: vec!["**/llmlint.yml".into()],
            exclude: vec![],
        };
        let files = resolve(dir.path(), &filter).unwrap();
        // `**/llmlint.yml` must match both the root and nested config file.
        assert_eq!(files.len(), 2, "got {files:?}");
    }

    #[test]
    fn scoped_roots_globs_at_the_config_dir_but_reports_cwd_relative() {
        // A nested config in `a/b` whose `*.txt` should mean "txt under a/b", run
        // from cwd `a`. The glob is rooted at `a/b` (so it bounds to that subtree),
        // and the returned paths are relative to the cwd `a` (what oneharness and
        // the report use).
        let dir = tempdir().unwrap();
        touch(dir.path(), "a/b/c.txt");
        touch(dir.path(), "a/b/deep/d.txt"); // under a/b — included (`*` spans `/`)
        touch(dir.path(), "a/top.txt"); // outside the `a/b` glob root — excluded
        let filter = FileFilter {
            include: vec!["*.txt".into()],
            exclude: vec![],
        };
        let glob_root = dir.path().join("a/b");
        let out_root = dir.path().join("a");
        let files = resolve_scoped(&glob_root, &out_root, &filter).unwrap();
        // Both files under a/b, reported relative to cwd `a`; `a/top.txt` is out.
        assert_eq!(
            files,
            vec![PathBuf::from("b/c.txt"), PathBuf::from("b/deep/d.txt")]
        );
    }

    #[test]
    fn scoped_ancestor_glob_root_only_sees_files_under_cwd() {
        // glob_root is an ancestor of cwd: the walk is still bounded to cwd, and a
        // recursive `**/*.rs` rooted at the ancestor matches cwd files (reported
        // relative to cwd).
        let dir = tempdir().unwrap();
        touch(dir.path(), "proj/src/a.rs");
        let filter = FileFilter {
            include: vec!["**/*.rs".into()],
            exclude: vec![],
        };
        let glob_root = dir.path().to_path_buf(); // ancestor
        let out_root = dir.path().join("proj"); // cwd
        let files = resolve_scoped(&glob_root, &out_root, &filter).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn scoped_non_overlapping_roots_yield_nothing() {
        let dir = tempdir().unwrap();
        touch(dir.path(), "a/x.txt");
        touch(dir.path(), "b/y.txt");
        let filter = FileFilter {
            include: vec!["**/*.txt".into()],
            exclude: vec![],
        };
        // `a` and `b` are siblings — neither contains the other.
        let files = resolve_scoped(&dir.path().join("a"), &dir.path().join("b"), &filter).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn empty_include_matches_every_file_under_root() {
        // No `files` block (the default filter) is the repo-wide "lint everything
        // under cwd" default — every file in the tree, not nothing.
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        touch(dir.path(), "README.md");
        let files = resolve(dir.path(), &FileFilter::default()).unwrap();
        assert_eq!(
            files,
            vec![PathBuf::from("README.md"), PathBuf::from("src/a.rs")]
        );
    }

    #[test]
    fn empty_include_still_honors_exclude() {
        // The match-all default is still narrowed by `exclude`, so it never
        // reintroduces files the config deliberately subtracts. (The
        // gitignore-aware walk narrows it too — exercised in the git-backed e2e.)
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        touch(dir.path(), "src/gen.rs");
        let filter = FileFilter {
            include: vec![],
            exclude: vec!["**/gen.rs".into()],
        };
        let files = resolve(dir.path(), &filter).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn global_exclude_wins_over_a_rule_include() {
        // Issue #128: a rule-level `include` must not resurrect a path the
        // top-level `exclude` denied. `**/tests/**` includes `tests/fixtures/*`,
        // but the cwd-rooted global exclude `tests/fixtures/**` still drops it.
        let dir = tempdir().unwrap();
        touch(dir.path(), "src/a.rs");
        touch(dir.path(), "tests/unit.rs");
        touch(dir.path(), "tests/fixtures/big.json");
        let rule_filter = FileFilter {
            include: vec!["**/tests/**".into()],
            exclude: vec![],
        };
        let files = resolve_scoped_excluding(
            dir.path(),
            dir.path(),
            &rule_filter,
            &[],
            &["tests/fixtures/**".into()],
        )
        .unwrap();
        // The rule's own test file is judged; the globally-excluded fixture is not.
        assert_eq!(files, vec![PathBuf::from("tests/unit.rs")]);
    }

    #[test]
    fn scoped_exclude_wins_over_a_rule_include() {
        // The rule's own config-level `exclude` (co-rooted at glob_root) also wins
        // over the rule's `include`, independent of any global exclude.
        let dir = tempdir().unwrap();
        touch(dir.path(), "tests/unit.rs");
        touch(dir.path(), "tests/fixtures/big.json");
        let rule_filter = FileFilter {
            include: vec!["**/tests/**".into()],
            exclude: vec![],
        };
        let files = resolve_scoped_excluding(
            dir.path(),
            dir.path(),
            &rule_filter,
            &["tests/fixtures/**".into()],
            &[],
        )
        .unwrap();
        assert_eq!(files, vec![PathBuf::from("tests/unit.rs")]);
    }

    #[test]
    fn global_exclude_matches_cwd_relative_under_a_nested_glob_root() {
        // The global exclude is matched cwd-relative (out_root), while the rule
        // filter roots at the nested glob_root. A `sub/vendored/**` global exclude
        // drops the vendored file even though the rule include (`*.rs`, rooted at
        // `sub`) matches it.
        let dir = tempdir().unwrap();
        touch(dir.path(), "sub/keep.rs");
        touch(dir.path(), "sub/vendored/gen.rs");
        let rule_filter = FileFilter {
            include: vec!["**/*.rs".into()],
            exclude: vec![],
        };
        let glob_root = dir.path().join("sub");
        let files = resolve_scoped_excluding(
            &glob_root,
            dir.path(),
            &rule_filter,
            &[],
            &["sub/vendored/**".into()],
        )
        .unwrap();
        assert_eq!(files, vec![PathBuf::from("sub/keep.rs")]);
    }

    #[test]
    fn invalid_glob_is_reported() {
        let filter = FileFilter {
            include: vec!["[".into()],
            exclude: vec![],
        };
        assert!(matches!(
            resolve(Path::new("."), &filter),
            Err(Error::InvalidConfig(_))
        ));
    }

    #[test]
    fn read_text_reads_utf8_and_skips_binary() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "// llmlint: ignore[r] x\n").unwrap();
        // An invalid UTF-8 byte sequence: not text, so it carries no directive.
        fs::write(dir.path().join("blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            read_text(dir.path(), Path::new("a.rs")).unwrap().as_deref(),
            Some("// llmlint: ignore[r] x\n")
        );
        assert_eq!(read_text(dir.path(), Path::new("blob.bin")).unwrap(), None);
    }

    #[test]
    fn read_text_missing_file_is_an_error() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            read_text(dir.path(), Path::new("nope.rs")),
            Err(Error::Io(_))
        ));
    }

    #[test]
    fn from_cli_relativizes_and_sorts() {
        let root = Path::new("/repo");
        let files = vec![PathBuf::from("/repo/b.rs"), PathBuf::from("/repo/a.rs")];
        let out = from_cli(root, &files);
        assert_eq!(out, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }
}
