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
    /// working-tree diff (with the unborn-HEAD `--cached` fallback); `Some(rev)`
    /// is an explicit base from `--diff-base` — a branch, tag, commit, or an
    /// `A..B`/`A...B` range — trusted as-is so git surfaces a bad ref directly
    /// instead of silently falling back. Diffing against a branch is exactly the
    /// PR-review case: `--diff-base main` shows what the current branch changed.
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
        // Pick the base. An explicit `--diff-base` (a ref or range) is trusted
        // as-is: a bad ref is git's own clear error, not a silent fallback, and
        // a range never has an unborn-HEAD problem. The implicit default diffs
        // the worktree against `HEAD`, but an unborn HEAD has no commit, so it
        // diffs the staged index against the empty tree (`git diff --cached`)
        // instead of fataling.
        let base_arg = match &self.base {
            Some(rev) => rev.clone(),
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
        // An explicit base is trusted, not probed: a ref that doesn't resolve is
        // git's own error surfaced as a diff error, never a silent `--cached`.
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
}
