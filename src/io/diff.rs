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

/// Build the [`DiffProvider`] for `backend`.
pub fn provider(backend: DiffBackend) -> Box<dyn DiffProvider> {
    match backend {
        DiffBackend::Git => Box::new(GitDiff::new()),
    }
}

/// Diffs a working tree against a base ref with `git diff`.
pub struct GitDiff {
    /// The base revision each file is compared against (default `HEAD`). Held as
    /// a field — not hardcoded at the call — so exposing a `--diff-base`/range
    /// later is a constructor change, not a rewrite.
    base: String,
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
    pub fn new() -> Self {
        GitDiff {
            base: "HEAD".to_string(),
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

    /// Whether `base` resolves to a commit. A freshly-`git init`'d repo has an
    /// unborn `HEAD`, so `git diff HEAD` would fatal; we fall back to a
    /// `--cached` diff (the index against the empty tree) there instead of
    /// erroring, so staged new files still show as additions.
    fn base_exists(&self, root: &Path) -> bool {
        Command::new(&self.git_bin)
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--verify", "--quiet", &self.base])
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
        // A committed base diffs the worktree against it (`git diff HEAD`); an
        // unborn HEAD has no base, so diff the staged index against the empty
        // tree (`git diff --cached`) instead of fataling.
        let base_arg = if self.base_exists(root) {
            self.base.as_str()
        } else {
            "--cached"
        };

        let mut out = BTreeMap::new();
        for file in files {
            // `to_slash` so the pathspec is the forward-slash form git speaks on
            // every platform; `--no-color` keeps the diff plain for the prompt.
            let rel = files::to_slash(file);
            let args = ["diff", "--no-color", base_arg, "--", &rel];
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
            base: "HEAD".into(),
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

        let diffs = provider(DiffBackend::Git)
            .diffs(root, &[PathBuf::from("a.rs")])
            .unwrap();
        assert!(diffs.contains_key(Path::new("a.rs")));
    }

    #[test]
    fn default_matches_new() {
        let d = GitDiff::default();
        assert_eq!(d.base, "HEAD");
        assert_eq!(d.git_bin, "git");
    }
}
