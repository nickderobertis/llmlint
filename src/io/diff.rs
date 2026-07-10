//! Compute per-file diffs to feed the judge, so it knows exactly which lines of
//! each target file changed and can focus its review on them.
//!
//! The capability is backend-agnostic: a [`DiffProvider`] maps a set of target
//! files to their unified diffs, and [`DiffBackend`] selects an implementation.
//! Git is the first (and default) backend; another VCS (or a base-ref/range
//! source) drops in as a new `DiffBackend` variant + `DiffProvider` impl without
//! touching the call sites — `lint` only ever talks to the trait.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;

use crate::errors::{Error, Result};
use crate::io::files;

/// Which diff backend to use. Parsed from `--diff <BACKEND>` (bare `--diff`
/// defaults to [`DiffBackend::Git`]). Lives here, next to the implementations,
/// so adding a backend is one local change; `cli` references it for the flag.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffBackend {
    /// Diff target files against `HEAD` with `git diff` (the default).
    #[default]
    Git,
}

impl DiffBackend {
    /// The lowercase id used in the `--diff` value and error messages.
    pub fn id(self) -> &'static str {
        match self {
            DiffBackend::Git => "git",
        }
    }
}

/// Source of per-file diffs. Implementations live behind this trait so the rest
/// of llmlint (planning, prompt render) is independent of any one VCS.
pub trait DiffProvider {
    /// Compute the unified diff of each file in `files` (paths relative to
    /// `root`). Returns a map from file path to its diff text; a file with no
    /// changes is **absent** from the map (no entry, not an empty string). An
    /// environment fault (backend missing, `root` not under version control) is
    /// an error — the user asked for diffs, so a silent empty result would be a
    /// false "nothing changed".
    fn diffs(&self, root: &Path, files: &[PathBuf]) -> Result<BTreeMap<PathBuf, String>>;
}

/// Build the [`DiffProvider`] for `backend`, comparing against `base` (a git
/// revision or range from `--diff-base`, or `None` for the backend default).
pub fn provider(backend: DiffBackend, base: Option<String>) -> Box<dyn DiffProvider> {
    match backend {
        DiffBackend::Git => Box::new(GitDiff::with_base(base)),
    }
}

/// Diffs a working tree against a base ref with `git diff`.
pub struct GitDiff {
    /// The base each file is compared against. `None` is the default `HEAD`
    /// working-tree diff (with the unborn-HEAD `--cached` fallback). `Some(rev)`
    /// is an explicit base from `--diff-base` — a branch, tag, commit, or an
    /// `A..B`/`A...B` range. Diffing against a branch is exactly the PR-review
    /// case: `--diff-base main` shows what the current branch changed. A **plain
    /// ref** gets **three-dot / merge-base semantics** — the diff is taken from
    /// where the branch diverged (`merge-base(rev, HEAD)`), not the base tip, so
    /// commits that landed on the base branch *after* this branch forked aren't
    /// reported as this branch's deletions (matching GitHub's "Files changed").
    /// An explicit **range** (`A..B`/`A...B`) is the caller's own choice of
    /// semantics and is passed to git untouched.
    base: Option<String>,
    /// The `git` binary (default `git` on PATH); a field so tests can point it
    /// at a missing binary to exercise the not-found path.
    git_bin: String,
}

impl Default for GitDiff {
    fn default() -> Self {
        GitDiff::new()
    }
}

impl GitDiff {
    /// A git differ against the default `HEAD` working-tree base.
    pub fn new() -> Self {
        GitDiff::with_base(None)
    }

    /// A git differ against an explicit `base` ref/range, or the default `HEAD`
    /// working-tree base when `None`.
    pub fn with_base(base: Option<String>) -> Self {
        GitDiff {
            base,
            git_bin: "git".to_string(),
        }
    }

    /// Run `git` in `root` with `args`, mapping a missing binary or a non-zero
    /// exit to a clear [`Error::Diff`]. Returns captured stdout on success.
    fn git(&self, root: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.git_bin)
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| {
                let message = if e.kind() == std::io::ErrorKind::NotFound {
                    "git not found on PATH; install git or choose another --diff backend"
                        .to_string()
                } else {
                    format!("running git: {e}")
                };
                Error::Diff {
                    backend: DiffBackend::Git.id().to_string(),
                    message,
                }
            })?;
        if !output.status.success() {
            return Err(Error::Diff {
                backend: DiffBackend::Git.id().to_string(),
                message: format!(
                    "`git {}` failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether `rev` resolves to a commit. A freshly-`git init`'d repo has an
    /// unborn `HEAD`, so `git diff HEAD` would fatal; we fall back to a
    /// `--cached` diff (the index against the empty tree) there instead of
    /// erroring, so staged new files still show as additions.
    fn rev_exists(&self, root: &Path, rev: &str) -> bool {
        Command::new(&self.git_bin)
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--verify", "--quiet", rev])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// The merge base (divergence point) of `rev` and `HEAD`, giving an explicit
    /// `--diff-base <ref>` three-dot / PR-review semantics: the diff is taken
    /// from where the branch forked, not the base tip, so base-branch commits
    /// that landed after the fork aren't rendered as this branch's changes.
    ///
    /// `None` when git can't compute one — disjoint histories, an unborn `HEAD`,
    /// or a bad ref. The caller then falls back to `rev` itself (a two-dot diff),
    /// so an unrelated base stays diffable and a bad ref still surfaces its error
    /// at the `git diff` step rather than being swallowed here.
    fn merge_base(&self, root: &Path, rev: &str) -> Option<String> {
        let output = Command::new(&self.git_bin)
            .arg("-C")
            .arg(root)
            .args(["merge-base", rev, "HEAD"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let mb = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!mb.is_empty()).then_some(mb)
    }
}

impl DiffProvider for GitDiff {
    fn diffs(&self, root: &Path, files: &[PathBuf]) -> Result<BTreeMap<PathBuf, String>> {
        // Validate the boundary once: a non-repo (or missing git) is a clear
        // error rather than a per-file failure or a silent empty result.
        let inside = self.git(root, &["rev-parse", "--is-inside-work-tree"])?;
        if inside.trim() != "true" {
            return Err(Error::Diff {
                backend: DiffBackend::Git.id().to_string(),
                message: format!("{} is not inside a git work tree", root.display()),
            });
        }
        // Pick the base.
        //
        // An explicit **range** (`A..B`/`A...B`) is the caller's own choice of
        // semantics — passed to git untouched. A **plain ref** gets three-dot /
        // merge-base semantics (see `merge_base`): the diff is taken from where
        // the branch diverged, not the base tip, so base-branch drift after the
        // fork isn't reported as this branch's changes. If no merge base exists
        // (disjoint histories) or one can't be computed, it falls back to a
        // two-dot diff against the ref itself, so an unrelated base stays
        // diffable and a bad ref still surfaces its error at the `git diff` step.
        //
        // The implicit default diffs the worktree against `HEAD`, but an unborn
        // HEAD has no commit, so it diffs the staged index against the empty tree
        // (`git diff --cached`) instead of fataling.
        let base_arg = match &self.base {
            Some(rev) if rev.contains("..") => rev.clone(),
            Some(rev) => self.merge_base(root, rev).unwrap_or_else(|| rev.clone()),
            None if self.rev_exists(root, "HEAD") => "HEAD".to_string(),
            None => "--cached".to_string(),
        };

        let mut out = BTreeMap::new();
        for file in files {
            // `to_slash` so the pathspec is the forward-slash form git speaks on
            // every platform; `--no-color` keeps the diff plain for the prompt.
            let rel = files::to_slash(file);
            let args = ["diff", "--no-color", base_arg.as_str(), "--", &rel];
            let diff = self.git(root, &args)?;
            if !diff.trim().is_empty() {
                out.insert(file.clone(), diff);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "t@t.t"]);
        git(dir, &["config", "user.name", "t"]);
        // Keep commits from depending on the host's default branch name.
        git(dir, &["checkout", "-q", "-b", "main"]);
    }

    #[test]
    fn backend_default_is_git() {
        assert_eq!(DiffBackend::default(), DiffBackend::Git);
        assert_eq!(DiffBackend::Git.id(), "git");
    }

    #[test]
    fn git_diff_reports_only_changed_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(root.join("b.rs"), "fn b() {}\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "init"]);

        // Change only a.rs after the commit.
        fs::write(root.join("a.rs"), "fn a() { todo!() }\n").unwrap();

        let provider = GitDiff::new();
        let diffs = provider
            .diffs(root, &[PathBuf::from("a.rs"), PathBuf::from("b.rs")])
            .unwrap();

        // a.rs changed -> present, carrying the added line; b.rs unchanged -> absent.
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
        assert!(diffs[Path::new("a.rs")].contains("todo!()"));
        assert!(!diffs.contains_key(Path::new("b.rs")), "got {diffs:?}");
    }

    #[test]
    fn unborn_head_falls_back_to_worktree_diff() {
        // A fresh repo with no commit: `git diff HEAD` would fatal, so the
        // provider must diff the index/worktree instead of erroring.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]); // staged, so it shows vs the empty index base

        let diffs = GitDiff::new()
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
    }

    #[test]
    fn non_repo_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let err = GitDiff::new()
            .diffs(dir.path(), &[PathBuf::from("a.rs")])
            .unwrap_err();
        // Not a work tree -> the git invocation fails; surfaced as a diff error.
        assert!(matches!(err, Error::Diff { .. }), "got {err:?}");
        assert!(err.to_string().contains("diff (git)"));
    }

    #[test]
    fn bare_repo_is_not_a_work_tree() {
        // A bare repo answers `is-inside-work-tree` with "false" (exit 0), so the
        // explicit work-tree check — not just git's exit code — must reject it.
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q", "--bare"]);
        let err = GitDiff::new()
            .diffs(dir.path(), &[PathBuf::from("a.rs")])
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not inside a git work tree"), "got {msg}");
    }

    #[test]
    fn missing_git_binary_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let provider = GitDiff {
            base: None,
            git_bin: "definitely-not-a-real-git-xyz".into(),
        };
        let err = provider
            .diffs(dir.path(), &[PathBuf::from("a.rs")])
            .unwrap_err();
        assert!(err.to_string().contains("git not found"), "got {err}");
    }

    #[test]
    fn provider_dispatches_git_backend() {
        // The factory returns a working git provider for the git backend.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "init"]);
        fs::write(root.join("a.rs"), "fn a() { 1; }\n").unwrap();

        let diffs = provider(DiffBackend::Git, None)
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")));
    }

    #[test]
    fn default_matches_new() {
        let d = GitDiff::default();
        assert_eq!(d.base, None);
        assert_eq!(d.git_bin, "git");
    }

    #[test]
    fn explicit_base_diffs_against_a_named_ref() {
        // Baseline on `main`, then a committed change on a feature branch: the
        // worktree is clean vs HEAD, but differs from `main`. The default (HEAD)
        // base sees nothing; `--diff-base main` surfaces what the branch changed.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("a.rs"), "fn a() { feature(); }\n").unwrap();
        git(root, &["commit", "-q", "-am", "feature change"]);

        // Clean worktree vs HEAD -> the default base reports no change.
        let none = GitDiff::new()
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(none.is_empty(), "got {none:?}");

        // vs `main` -> the committed feature change shows up.
        let vs_main = GitDiff::with_base(Some("main".into()))
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(vs_main.contains_key(Path::new("a.rs")), "got {vs_main:?}");
        assert!(vs_main[Path::new("a.rs")].contains("feature()"));
    }

    #[test]
    fn explicit_base_accepts_a_range() {
        // A `main..HEAD` range expression is passed through to `git diff` as-is,
        // so commit-to-commit review works without involving the worktree.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("a.rs"), "fn a() { ranged(); }\n").unwrap();
        git(root, &["commit", "-q", "-am", "feature change"]);

        let diffs = GitDiff::with_base(Some("main..HEAD".into()))
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
        assert!(diffs[Path::new("a.rs")].contains("ranged()"));
    }

    #[test]
    fn explicit_base_with_unknown_ref_is_a_clear_error() {
        // A ref that doesn't resolve has no merge base, so the diff falls back to
        // a two-dot diff against the ref — where `git diff` surfaces the bad ref
        // as its own error, never a silent `--cached`.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "baseline"]);

        let err = GitDiff::with_base(Some("no-such-ref".into()))
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap_err();
        assert!(matches!(err, Error::Diff { .. }), "got {err:?}");
        assert!(err.to_string().contains("diff (git)"), "got {err}");
    }

    #[test]
    fn explicit_base_ignores_base_branch_drift_after_the_fork() {
        // The issue-137 scenario: a feature branch forks from `main`, then `main`
        // advances with an unrelated commit the branch never merged. A two-dot
        // diff against the base *tip* would render `main`'s later change as this
        // branch's deletion; three-dot / merge-base semantics must not.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        // Shared history: both files exist at the fork point.
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(root.join("base_only.rs"), "fn base() {}\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "fork point"]);

        // Feature branch: change only a.rs.
        git(root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("a.rs"), "fn a() { feature(); }\n").unwrap();
        git(root, &["commit", "-q", "-am", "feature change"]);

        // main advances after the fork with a change to base_only.rs the branch
        // never saw.
        git(root, &["checkout", "-q", "main"]);
        fs::write(root.join("base_only.rs"), "fn base() { drifted(); }\n").unwrap();
        git(root, &["commit", "-q", "-am", "base drift"]);
        git(root, &["checkout", "-q", "feature"]);

        let diffs = GitDiff::with_base(Some("main".into()))
            .diffs(
                root,
                &[PathBuf::from("a.rs"), PathBuf::from("base_only.rs")],
            )
            .unwrap();

        // The branch's own change is present...
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
        assert!(diffs[Path::new("a.rs")].contains("feature()"));
        // ...but the base-branch drift the branch never touched is not reported
        // as this branch's change (the false-positive the issue describes).
        assert!(
            !diffs.contains_key(Path::new("base_only.rs")),
            "base-branch drift leaked into the diff: {diffs:?}"
        );
    }

    #[test]
    fn explicit_two_dot_range_still_includes_base_drift() {
        // A caller who *wants* two-dot semantics keeps them: an explicit `A..B`
        // range is passed through untouched, so a `main..HEAD`-style range is
        // not rewritten to merge-base. Here we assert the range spelling is
        // honored end to end by diffing the branch's own change.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("a.rs"), "fn a() { ranged(); }\n").unwrap();
        git(root, &["commit", "-q", "-am", "feature change"]);

        let diffs = GitDiff::with_base(Some("main..HEAD".into()))
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
        assert!(diffs[Path::new("a.rs")].contains("ranged()"));
    }

    #[test]
    fn unrelated_base_falls_back_to_two_dot_diff() {
        // Disjoint histories have no merge base, so the diff falls back to a
        // two-dot diff against the ref rather than erroring — an unrelated base
        // is still diffable.
        let dir = tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "main baseline"]);

        // An orphan branch with no shared history, carrying its own a.rs.
        git(root, &["checkout", "-q", "--orphan", "orphan"]);
        fs::write(root.join("a.rs"), "fn a() { orphan(); }\n").unwrap();
        git(root, &["add", "a.rs"]);
        git(root, &["commit", "-q", "-m", "orphan baseline"]);

        // vs `main` (no common ancestor) -> two-dot fallback surfaces the diff.
        let diffs = GitDiff::with_base(Some("main".into()))
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")), "got {diffs:?}");
    }
}
