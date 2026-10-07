//! Journeys over `.github/scripts/ci-gate.sh`, the CI routing and the release's
//! sweep verdict, driven the way the workflows run them: `tier` fed the event
//! payload a GitHub runner hands it, in a scratch git repository, and `verdict`
//! with GitHub's API answered by a stand-in `gh` on PATH (the one external seam;
//! the real script, `jq` and `git` run). Unix-only, like the script.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// The repository root: this crate's manifest sits one level below it.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the repository root")
        .to_path_buf()
}

fn script() -> PathBuf {
    repo_root().join(".github/scripts/ci-gate.sh")
}

/// A command with no inherited GitHub Actions or git repository-selection
/// variables, so a case states its whole environment (and a run inside the
/// repository's own pre-push hook or CI job cannot leak in).
fn clean(mut cmd: Command) -> Command {
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if key.starts_with("GITHUB_")
            || key.starts_with("GIT_")
            || [
                "REPO",
                "SHA",
                "CI_WAIT_ATTEMPTS",
                "CI_WAIT_DELAY",
                "GH_TOKEN",
            ]
            .contains(&&*key)
        {
            cmd.env_remove(&name);
        }
    }
    cmd
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = clean(Command::new("git"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A scratch repository with `main` at one commit, `origin/main` pointing at it,
/// and a feature commit checked out on top — the shape a pull-request checkout
/// has (fetch-depth: 0).
struct Repo {
    dir: TempDir,
    main: String,
    head: String,
}

impl Repo {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t.t"]);
        git(p, &["config", "user.name", "t"]);
        fs::write(p.join("a"), "1").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "base"]);
        let main = git(p, &["rev-parse", "HEAD"]);
        git(p, &["update-ref", "refs/remotes/origin/main", &main]);
        fs::write(p.join("a"), "2").unwrap();
        git(p, &["commit", "-qam", "feature"]);
        let head = git(p, &["rev-parse", "HEAD"]);
        Repo { dir, main, head }
    }

    /// Run `ci-gate.sh tier` for `event` with `payload`, writing GITHUB_OUTPUT
    /// to a file as a runner does; returns the output and what it recorded.
    fn tier(&self, event: &str, payload: &str) -> (Output, String) {
        let event_path = self.dir.path().join("event.json");
        fs::write(&event_path, payload).unwrap();
        let github_output = self.dir.path().join("github-output");
        let _ = fs::remove_file(&github_output);
        let out = clean(Command::new("bash"))
            .arg(script())
            .arg("tier")
            .current_dir(self.dir.path())
            .env("GITHUB_EVENT_NAME", event)
            .env("GITHUB_EVENT_PATH", &event_path)
            .env("GITHUB_OUTPUT", &github_output)
            .output()
            .unwrap();
        (out, fs::read_to_string(&github_output).unwrap_or_default())
    }
}

fn pull_request(head_ref: &str, head_repo: &str, head_sha: &str) -> String {
    format!(
        r#"{{"pull_request":{{"head":{{"ref":"{head_ref}","sha":"{head_sha}","repo":{{"full_name":"{head_repo}"}}}},
            "base":{{"ref":"main","repo":{{"full_name":"owner/llmlint"}}}}}}}}"#
    )
}

#[test]
fn a_release_plz_pull_request_routes_to_the_full_sweep_on_its_head() {
    let repo = Repo::new();
    let (out, recorded) = repo.tier(
        "pull_request",
        &pull_request(
            "release-plz-2026-10-07T12-00-00Z",
            "owner/llmlint",
            &repo.head,
        ),
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(recorded, format!("tier=all\nbase=\nhead={}\n", repo.head));
    assert!(String::from_utf8_lossy(&out.stderr).contains("release PR"));
}

#[test]
fn an_ordinary_pull_request_routes_to_the_affected_tier_from_the_merge_base() {
    let repo = Repo::new();
    let (out, recorded) = repo.tier(
        "pull_request",
        &pull_request("feature/graph", "owner/llmlint", &repo.head),
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        recorded,
        format!("tier=affected\nbase={}\nhead=\n", repo.main)
    );
}

#[test]
fn a_fork_branch_named_like_a_release_pr_is_not_swept_as_one() {
    // Only release-plz, pushing to this repository, opens release PRs; a fork
    // choosing the same branch name gets the ordinary affected tier.
    let repo = Repo::new();
    let (out, recorded) = repo.tier(
        "pull_request",
        &pull_request("release-plz-fake", "someone/llmlint", &repo.head),
    );
    assert!(out.status.success(), "{out:?}");
    assert!(recorded.starts_with("tier=affected\n"), "{recorded}");
}

#[test]
fn a_push_to_main_routes_to_the_affected_tier_from_the_previous_tip() {
    let repo = Repo::new();
    let (out, recorded) = repo.tier("push", &format!(r#"{{"before":"{}"}}"#, repo.main));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        recorded,
        format!("tier=affected\nbase={}\nhead=\n", repo.main)
    );
    // A first push to a branch has an all-zero `before`: fall back to HEAD~1.
    let zero = "0".repeat(40);
    let (out, recorded) = repo.tier("push", &format!(r#"{{"before":"{zero}"}}"#));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        recorded,
        format!("tier=affected\nbase={}\nhead=\n", repo.main)
    );
}

#[test]
fn a_manual_dispatch_routes_to_the_full_sweep() {
    let repo = Repo::new();
    let (out, recorded) = repo.tier("workflow_dispatch", "{}");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(recorded, "tier=all\nbase=\nhead=\n");
}

#[test]
fn an_unroutable_event_or_payload_is_refused_without_a_tier() {
    let repo = Repo::new();
    let (out, recorded) = repo.tier("schedule", "{}");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("no tier is defined for the 'schedule' event"));
    assert!(recorded.is_empty(), "{recorded}");

    let (out, recorded) = repo.tier(
        "pull_request",
        &pull_request("feature", "owner/llmlint", &repo.head).replace("\"main\"", "\"a..b\""),
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no usable base ref ('a..b')"));
    assert!(recorded.is_empty(), "{recorded}");
}

#[test]
fn the_release_pr_prefix_is_the_one_release_plz_auto_merges() {
    // release-plz.yml's auto-merge step selects the release PR by branch prefix;
    // the router must sweep exactly those PRs, so the two prefixes are one fact.
    let gate = fs::read_to_string(script()).unwrap();
    let prefix = gate
        .lines()
        .find_map(|l| l.strip_prefix("readonly RELEASE_PR_PREFIX=\""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("ci-gate.sh declares RELEASE_PR_PREFIX");
    let plz = fs::read_to_string(repo_root().join(".github/workflows/release-plz.yml")).unwrap();
    assert!(
        plz.contains(&format!("startswith(\"{prefix}\")")),
        "release-plz.yml must select release PRs by the prefix ci-gate.sh sweeps: {prefix}"
    );
}

// ---- verdict ---------------------------------------------------------------

const SHA: &str = "1111111111111111111111111111111111111111";
const TREE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_TREE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REPO: &str = "owner/llmlint";

/// A stand-in GitHub API: a `gh` on PATH answering `gh api <endpoint>` from
/// fixture files — the released commit, the ci.yml runs per event, and each
/// run's jobs (`jobs-<id>.json`, or `jobs-<id>.<n>.json` for the n-th poll).
/// It records every endpoint it was asked for.
struct Github {
    dir: TempDir,
}

/// A ci.yml run as the runs endpoint lists it.
fn run(id: u64, event: &str, branch: &str, head_repo: &str, tree: &str, status: &str) -> String {
    format!(
        r#"{{"id":{id},"event":"{event}","head_branch":"{branch}","head_sha":"{SHA}","status":"{status}",
            "created_at":"2026-10-07T12:00:{id:02}Z","html_url":"https://github.com/{REPO}/actions/runs/{id}",
            "head_repository":{{"full_name":"{head_repo}"}},"head_commit":{{"id":"{SHA}","tree_id":"{tree}"}}}}"#
    )
}

/// The sweep jobs of one run, each `(name, status, conclusion)`.
fn jobs(list: &[(&str, &str, &str)]) -> String {
    let jobs: Vec<String> = list
        .iter()
        .map(|(name, status, conclusion)| {
            let conclusion = if conclusion.is_empty() {
                "null".to_string()
            } else {
                format!("\"{conclusion}\"")
            };
            format!(r#"{{"name":"{name}","status":"{status}","conclusion":{conclusion}}}"#)
        })
        .collect();
    format!(r#"{{"jobs":[{}]}}"#, jobs.join(","))
}

const GREEN: &[(&str, &str, &str)] = &[
    ("gate", "completed", "success"),
    ("cross (macos-latest)", "completed", "success"),
    ("cross (windows-latest)", "completed", "success"),
    ("deny", "completed", "success"),
];

impl Github {
    fn new(pr_runs: &[String], dispatch_runs: &[String]) -> Self {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(
            p.join("commit.json"),
            format!(r#"{{"sha":"{SHA}","commit":{{"tree":{{"sha":"{TREE}"}}}}}}"#),
        )
        .unwrap();
        for (file, runs) in [
            ("runs-pr.json", pr_runs),
            ("runs-dispatch.json", dispatch_runs),
        ] {
            fs::write(
                p.join(file),
                format!(
                    r#"{{"total_count":{},"workflow_runs":[{}]}}"#,
                    runs.len(),
                    runs.join(",")
                ),
            )
            .unwrap();
        }
        fs::create_dir_all(p.join("bin")).unwrap();
        let gh = p.join("bin/gh");
        fs::write(
            &gh,
            r#"#!/usr/bin/env bash
set -euo pipefail
d="$GH_FIXTURES"
printf '%s\n' "$*" >>"$d/calls"
[ "$1" = api ] || { echo "stub gh: only 'api' is answered" >&2; exit 2; }
[ -z "${GH_FAIL:-}" ] || { echo 'HTTP 403: Resource not accessible by integration' >&2; exit 1; }
case "$2" in
  */commits/*) cat "$d/commit.json" ;;
  *runs\?event=pull_request*) cat "$d/runs-pr.json" ;;
  *runs\?event=workflow_dispatch*) cat "$d/runs-dispatch.json" ;;
  */actions/runs/*/jobs*)
    id="$(sed 's@.*/actions/runs/\([0-9]*\)/jobs.*@\1@' <<<"$2")"
    n=$(( $(cat "$d/polls-$id" 2>/dev/null || echo 0) + 1 ))
    echo "$n" >"$d/polls-$id"
    if [ -f "$d/jobs-$id.$n.json" ]; then cat "$d/jobs-$id.$n.json"; else cat "$d/jobs-$id.json"; fi ;;
  *) echo "stub gh: no answer for $2" >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        Github { dir }
    }

    fn jobs(&self, file: &str, body: &str) -> &Self {
        fs::write(self.dir.path().join(file), body).unwrap();
        self
    }

    fn verdict(&self, env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        clean(Command::new("bash"))
            .arg(script())
            .arg("verdict")
            .env("PATH", path)
            .env("GH_FIXTURES", self.dir.path())
            .env("REPO", REPO)
            .env("SHA", SHA)
            .env("CI_WAIT_ATTEMPTS", "3")
            .env("CI_WAIT_DELAY", "0")
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_green_release_pr_sweep_of_the_released_tree_lets_the_release_ship() {
    let gh = Github::new(
        &[run(
            7,
            "pull_request",
            "release-plz-2026",
            REPO,
            TREE,
            "completed",
        )],
        &[],
    );
    gh.jobs("jobs-7.json", &jobs(GREEN));
    let out = gh.verdict(&[]);
    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("CI run 7") && said.contains(TREE), "{said}");
    // It asked about the released commit and that run's jobs, nothing else.
    let calls = gh.calls();
    assert!(
        calls.contains(&format!("repos/{REPO}/commits/{SHA}")),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("repos/{REPO}/actions/runs/7/jobs")),
        "{calls}"
    );
}

#[test]
fn a_red_sweep_stops_the_release_naming_the_failed_job() {
    let gh = Github::new(
        &[run(
            8,
            "pull_request",
            "release-plz-2026",
            REPO,
            TREE,
            "completed",
        )],
        &[],
    );
    gh.jobs(
        "jobs-8.json",
        &jobs(&[
            ("gate", "completed", "failure"),
            ("cross (macos-latest)", "completed", "success"),
            ("cross (windows-latest)", "completed", "success"),
        ]),
    );
    let out = gh.verdict(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("CI run 8 job gate (failure)"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn no_sweep_at_all_stops_the_release() {
    // An ordinary pull request's (affected-tier) run of the same tree is not a
    // sweep verdict, and neither is a fork's release-plz-named branch.
    let gh = Github::new(
        &[
            run(3, "pull_request", "feature/x", REPO, TREE, "completed"),
            run(
                4,
                "pull_request",
                "release-plz-fake",
                "someone/llmlint",
                TREE,
                "completed",
            ),
        ],
        &[],
    );
    gh.jobs("jobs-3.json", &jobs(GREEN))
        .jobs("jobs-4.json", &jobs(GREEN));
    let out = gh.verdict(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = stderr(&out);
    assert!(
        err.contains(&format!(
            "no full-sweep CI run tested the released tree {TREE}"
        )),
        "{err}"
    );
    assert!(err.contains("workflow_dispatch"), "{err}");
    assert!(!gh.calls().contains("/jobs"), "{}", gh.calls());
}

#[test]
fn a_green_sweep_of_a_different_tree_stops_the_release() {
    let gh = Github::new(
        &[run(
            9,
            "pull_request",
            "release-plz-2026",
            REPO,
            OTHER_TREE,
            "completed",
        )],
        &[],
    );
    gh.jobs("jobs-9.json", &jobs(GREEN));
    let out = gh.verdict(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = stderr(&out);
    assert!(err.contains(&format!("released tree {TREE}")), "{err}");
    assert!(
        err.contains(&format!("run 9 swept tree {OTHER_TREE}")),
        "{err}"
    );
}

#[test]
fn a_manual_sweep_of_the_released_tree_is_a_verdict() {
    // The recovery path the refusals name: dispatch CI on the released commit.
    let gh = Github::new(
        &[run(
            9,
            "pull_request",
            "release-plz-2026",
            REPO,
            OTHER_TREE,
            "completed",
        )],
        &[run(
            12,
            "workflow_dispatch",
            "main",
            REPO,
            TREE,
            "completed",
        )],
    );
    gh.jobs("jobs-12.json", &jobs(GREEN));
    let out = gh.verdict(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("CI run 12"));
}

#[test]
fn the_newest_sweep_of_the_tree_decides_and_a_running_one_is_waited_for() {
    // An older green sweep does not outvote a newer one; the newer one is still
    // running on the first poll and green on the second.
    let gh = Github::new(
        &[
            run(
                5,
                "pull_request",
                "release-plz-old",
                REPO,
                TREE,
                "completed",
            ),
            run(
                6,
                "pull_request",
                "release-plz-new",
                REPO,
                TREE,
                "in_progress",
            ),
        ],
        &[],
    );
    gh.jobs("jobs-5.json", &jobs(GREEN))
        .jobs(
            "jobs-6.1.json",
            &jobs(&[
                ("gate", "in_progress", ""),
                ("cross (macos-latest)", "completed", "success"),
                ("cross (windows-latest)", "completed", "success"),
            ]),
        )
        .jobs("jobs-6.json", &jobs(GREEN));
    let out = gh.verdict(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("CI run 6"));
    assert!(
        stderr(&out).contains("gate (in_progress); waiting (1/3)"),
        "{}",
        stderr(&out)
    );
    assert!(!gh.calls().contains("runs/5/jobs"), "{}", gh.calls());
}

#[test]
fn a_finished_sweep_missing_a_job_or_one_that_never_finishes_stops_the_release() {
    let gh = Github::new(
        &[run(
            10,
            "pull_request",
            "release-plz-2026",
            REPO,
            TREE,
            "completed",
        )],
        &[],
    );
    gh.jobs(
        "jobs-10.json",
        &jobs(&[
            ("gate", "completed", "success"),
            ("cross (macos-latest)", "completed", "success"),
        ]),
    );
    let out = gh.verdict(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("finished without a verdict from job cross (windows-latest)"),
        "{}",
        stderr(&out)
    );

    let gh = Github::new(
        &[run(
            11,
            "pull_request",
            "release-plz-2026",
            REPO,
            TREE,
            "in_progress",
        )],
        &[],
    );
    gh.jobs("jobs-11.json", &jobs(&[("gate", "queued", "")]));
    let out = gh.verdict(&[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("did not finish within 3 polls"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn an_unreadable_api_or_bad_inputs_stop_the_release() {
    let gh = Github::new(&[], &[]);
    let out = gh.verdict(&[("GH_FAIL", "1")]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr(&out).contains("HTTP 403"), "{}", stderr(&out));

    let out = gh.verdict(&[("SHA", "abc123")]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(stderr(&out).contains("not a full 40-character commit sha"));
    assert!(gh.calls().is_empty() || !gh.calls().contains("abc123"));
}
