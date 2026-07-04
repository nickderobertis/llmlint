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
    // An empty `include` set means "every file under the walk root": `build_set`
    // returns `None` for it, which the loop below reads as match-all (see the
    // `is_none_or`). This is the repo-wide default — a config with no `files`
    // block lints the whole tree from `cwd` instead of nothing. A present set
    // filters as usual; `exclude` (and the gitignore-aware walk) still subtract
    // from whatever `include` picks.
    let include = build_set(&filter.include)?;
    let exclude = build_set(&filter.exclude)?;

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
        let excluded = exclude.as_ref().is_some_and(|e| e.is_match(rel_glob));
        let included = include.as_ref().is_none_or(|set| set.is_match(rel_glob));
        if included && !excluded {
            out.push(rel_out.to_path_buf());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Intersect an **explicit candidate file list** with a [`FileFilter`] — the
/// "explicit files ∩ config globs" path. Each candidate (given relative to
/// `out_root`, or absolute) is matched by its `glob_root`-relative path against
/// the filter's `include`/`exclude` globs, exactly as [`resolve_scoped`] matches
/// a walked file, so a rule's globs *narrow* an explicit set instead of
/// re-expanding across the whole tree. Only candidates that fall under `glob_root`
/// are considered (a rule never judges a file outside its directory scope);
/// returned paths keep their `out_root`-relative spelling, sorted and de-duped.
///
/// This is what makes an explicit file universe — the CLI file list, or the
/// changed files from a `--diff` run — a filter that config globs intersect with
/// rather than one the globs ignore. An empty `include` still means match-all (the
/// repo-wide default), so a config with no `files` block keeps every candidate.
pub fn filter_scoped(
    glob_root: &Path,
    out_root: &Path,
    filter: &FileFilter,
    candidates: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let include = build_set(&filter.include)?;
    let exclude = build_set(&filter.exclude)?;

    let mut out = Vec::new();
    for cand in candidates {
        // Absolutize (CLI paths are usually cwd-relative) and lexically normalize
        // so the prefix test and glob match see a clean path, matching how
        // `configfs` keys files elsewhere.
        let abs = if cand.is_absolute() {
            cand.clone()
        } else {
            out_root.join(cand)
        };
        let abs = crate::io::configfs::normalize(&abs);
        // Match globs against the config-dir-relative path; a candidate outside the
        // glob root is out of this rule's scope and dropped.
        let Ok(rel_glob) = abs.strip_prefix(glob_root) else {
            continue;
        };
        let excluded = exclude.as_ref().is_some_and(|e| e.is_match(rel_glob));
        let included = include.as_ref().is_none_or(|set| set.is_match(rel_glob));
        if included && !excluded {
            out.push(cand.clone());
        }
    }
    out.sort();
    out.dedup();
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
    fn filter_scoped_intersects_candidates_with_globs() {
        // The candidate list is the universe; the filter's globs narrow it. A
        // candidate matching the include is kept; one that doesn't is dropped —
        // globs never re-expand beyond the passed files.
        let dir = tempdir().unwrap();
        let filter = FileFilter {
            include: vec!["src/**/*.rs".into()],
            exclude: vec!["**/gen.rs".into()],
        };
        let candidates = vec![
            PathBuf::from("src/a.rs"),   // matches include -> kept
            PathBuf::from("src/gen.rs"), // excluded -> dropped
            PathBuf::from("README.md"),  // outside include -> dropped
        ];
        let out = filter_scoped(dir.path(), dir.path(), &filter, &candidates).unwrap();
        assert_eq!(out, vec![PathBuf::from("src/a.rs")]);
    }

    #[test]
    fn filter_scoped_empty_include_keeps_every_candidate_under_root() {
        // No `files` block (default filter) is match-all: every candidate under the
        // glob root survives, so a flat config with an explicit file list is
        // unchanged by the intersection.
        let dir = tempdir().unwrap();
        let candidates = vec![PathBuf::from("a.rs"), PathBuf::from("sub/b.rs")];
        let out =
            filter_scoped(dir.path(), dir.path(), &FileFilter::default(), &candidates).unwrap();
        assert_eq!(out, candidates);
    }

    #[test]
    fn filter_scoped_bounds_candidates_to_the_glob_root() {
        // With a nested glob root, only candidates under it are considered; the
        // returned paths keep their out_root-relative spelling.
        let dir = tempdir().unwrap();
        let glob_root = dir.path().join("frontend");
        let candidates = vec![
            PathBuf::from("frontend/app.ts"), // under glob_root -> kept
            PathBuf::from("backend/svc.rs"),  // outside -> dropped
        ];
        let out =
            filter_scoped(&glob_root, dir.path(), &FileFilter::default(), &candidates).unwrap();
        assert_eq!(out, vec![PathBuf::from("frontend/app.ts")]);
    }

    #[test]
    fn filter_scoped_empty_candidates_is_empty() {
        // An explicit-but-empty universe (a `--diff` with no changes) yields
        // nothing, never a match-all re-expansion.
        let dir = tempdir().unwrap();
        let out = filter_scoped(dir.path(), dir.path(), &FileFilter::default(), &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn from_cli_relativizes_and_sorts() {
        let root = Path::new("/repo");
        let files = vec![PathBuf::from("/repo/b.rs"), PathBuf::from("/repo/a.rs")];
        let out = from_cli(root, &files);
        assert_eq!(out, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }
}
