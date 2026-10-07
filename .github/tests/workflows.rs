//! Drift gates over the committed GitHub Actions workflows: the fixed set of
//! status-check contexts every pull request must report, the suppressions
//! comment kept outside it, and the Performance workflow's trigger lists. GitHub
//! is the only thing that runs these files, which a test cannot reach, so the
//! workflow files themselves are the interface under test. This is the
//! `ci-workflows` project's `test` target (actionlint is its `lint-workflows`).

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root: this crate's manifest sits one level below it, and
/// every path these journeys read or drive is relative to the root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the repository root")
        .to_path_buf()
}

// llmlint: ignore-block[tests_mirror_real_usage] a drift gate over a committed workflow: the only command that reads bench.yml's triggers is actionlint, which the test run does not have, so the file itself is the interface under test
/// `bench.yml` must not share its trigger paths through a YAML anchor/alias —
/// actionlint (`just lint-workflows`, inside `check`) has rejected an alias in a
/// `paths` filter — so the two filters are spelled out, and this holds them equal
/// pattern for pattern, in order, so neither can drift from the other.
#[test]
fn bench_workflow_spells_out_equal_push_and_pull_request_paths() {
    let root = repo_root();
    let text = fs::read_to_string(root.join(".github/workflows/bench.yml")).unwrap();
    let on_block: String = text
        .lines()
        .skip_while(|l| *l != "on:")
        .take_while(|l| *l == "on:" || l.is_empty() || l.starts_with(' '))
        .collect::<Vec<_>>()
        .join("\n");
    for line in on_block.lines() {
        // The glob patterns are single-quoted and carry `*`; drop quoted text and
        // comments so only YAML syntax is left to look for `&anchor` / `*alias`.
        let syntax: String = line
            .split('#')
            .next()
            .unwrap()
            .split('\'')
            .step_by(2)
            .collect();
        assert!(
            !syntax.contains('&') && !syntax.contains('*'),
            "bench.yml's triggers use a YAML anchor or alias: {line}"
        );
    }
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).unwrap();
    let paths = |event: &str| -> Vec<String> {
        doc["on"][event]["paths"]
            .as_sequence()
            .unwrap_or_else(|| panic!("bench.yml's {event} trigger has no paths list"))
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let push = paths("push");
    assert!(!push.is_empty(), "bench.yml's push paths are empty");
    assert_eq!(
        paths("pull_request"),
        push,
        "bench.yml's pull_request paths must equal its push paths"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The status-check contexts branch protection is built against: every pull
/// request to `main` must report each of these under exactly this name, or a PR
/// either waits forever on a context that never reports or merges past one that
/// was dropped. `llmlint` is the judged tier's blocking check.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the third copy, the live branch protection, is applied and reconciled off-repo by the governance step (`setup_github_governance.py --verify`); this repo's gate has no settings-API access by design, and the two in-repo copies (the workflows and AGENTS.md) are both held to this list
const PR_CONTEXTS: &[&str] = &[
    "gate",
    "deny",
    "pr-title",
    "cross (macos-latest)",
    "cross (windows-latest)",
    "install (ubuntu-latest)",
    "install (macos-latest)",
    "install (windows-latest)",
    "visual-docs / report (x86_64)",
    "visual-docs / report (arm64)",
    "llmlint",
];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The `if:` conditions a contract job may carry: each is true on every
/// `pull_request` event, so it can never leave the context unreported.
const PR_TRUE_CONDITIONS: &[&str] = &["github.event_name == 'pull_request'"];

fn workflow_docs() -> Vec<(String, serde_yaml_ng::Value)> {
    let dir = repo_root().join(".github/workflows");
    let mut docs: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let doc = serde_yaml_ng::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            (name, doc)
        })
        .collect();
    docs.sort_by(|a, b| a.0.cmp(&b.0));
    docs
}

/// Why a workflow would not run on every pull request to `main`, or `None`
/// when it always does.
fn pr_trigger_gap(doc: &serde_yaml_ng::Value) -> Option<String> {
    let on = &doc["on"];
    let pr = match on.as_mapping() {
        Some(m) => match m.get("pull_request") {
            Some(pr) => pr,
            None => return Some("no pull_request trigger".into()),
        },
        None => return Some(format!("triggers {on:?}, not a pull_request mapping")),
    };
    if pr.is_null() {
        return None;
    }
    if let Some(branches) = pr["branches"].as_sequence() {
        if !branches.iter().any(|b| b.as_str() == Some("main")) {
            return Some(format!("pull_request branches {branches:?} omit main"));
        }
    }
    for filter in ["paths", "paths-ignore", "branches-ignore"] {
        if !pr[filter].is_null() {
            return Some(format!("pull_request carries a `{filter}` filter"));
        }
    }
    if let Some(types) = pr["types"].as_sequence() {
        for needed in ["opened", "synchronize", "reopened"] {
            if !types.iter().any(|t| t.as_str() == Some(needed)) {
                return Some(format!("pull_request types {types:?} omit {needed}"));
            }
        }
    }
    None
}

/// The screencomp reusable workflow `visual-docs.yml` calls, at the pin whose
/// inner job is named `report` (so its contexts are `<caller> / report (<arch>)`).
/// A pin bump fails here until that name is confirmed for the new version.
const VISUAL_DOCS_REUSABLE: &str =
    "nickderobertis/screencomp/.github/workflows/visual-docs-reusable.yml@v0.4.2";

/// The contexts a job reports, as GitHub names them: its `name:` (else its id),
/// suffixed with the matrix value for a one-axis matrix. A reusable-workflow call
/// (`uses:`) to screencomp's visual-docs reports `<caller> / report (<arch>)`
/// per `[capture].arches` lane in screencomp.toml, which the reusable reads.
fn job_contexts(id: &str, job: &serde_yaml_ng::Value) -> Vec<String> {
    let base = job["name"].as_str().unwrap_or(id).to_string();
    if let Some(uses) = job["uses"].as_str() {
        if uses.contains("/visual-docs-reusable.yml@") {
            assert_eq!(
                uses, VISUAL_DOCS_REUSABLE,
                "job {id} moved screencomp's reusable workflow off the pin whose inner \
                 job is `report`: confirm the new version's job name, then update \
                 VISUAL_DOCS_REUSABLE"
            );
            let toml_text = fs::read_to_string(repo_root().join("screencomp.toml")).unwrap();
            let cfg: toml::Value = toml::from_str(&toml_text).unwrap();
            return cfg["capture"]["arches"]
                .as_array()
                .expect("screencomp.toml declares [capture].arches")
                .iter()
                .map(|a| format!("{base} / report ({})", a.as_str().unwrap()))
                .collect();
        }
        return vec![base];
    }
    match job["strategy"]["matrix"].as_mapping() {
        Some(m) if m.len() == 1 => {
            let values = m.values().next().unwrap().as_sequence().unwrap();
            values
                .iter()
                .map(|v| format!("{base} ({})", v.as_str().unwrap()))
                .collect()
        }
        Some(m) => panic!("job {id}: a multi-axis matrix {m:?} is not modelled here"),
        None => vec![base],
    }
}

/// Why a job could be skipped on a pull request (its own `if:` or one along its
/// `needs` chain), or `None` when it always runs once its workflow triggers.
fn pr_job_gap(jobs: &serde_yaml_ng::Mapping, id: &str) -> Option<String> {
    let job = &jobs[id];
    if let Some(cond) = job["if"].as_str() {
        if !PR_TRUE_CONDITIONS.contains(&cond.trim()) {
            return Some(format!("job {id} is conditioned on `{cond}`"));
        }
    }
    let needs: Vec<&str> = match &job["needs"] {
        serde_yaml_ng::Value::String(n) => vec![n.as_str()],
        serde_yaml_ng::Value::Sequence(ns) => ns.iter().map(|n| n.as_str().unwrap()).collect(),
        _ => vec![],
    };
    needs
        .into_iter()
        .find_map(|n| pr_job_gap(jobs, n).map(|gap| format!("job {id} needs {n}, and {gap}")))
}

// llmlint: ignore-block[tests_mirror_real_usage] a drift gate over the committed workflows: GitHub is the only thing that turns them into status checks, which a test run cannot reach, so the workflow files are the interface under test
/// Every context in the fixed contract is reported, under exactly its name, by a
/// job that runs on every pull request to `main`: no trigger filter, `if:`, or
/// `needs` edge can leave one unreported. Read from the committed workflows, so
/// adding a workflow or a condition is checked against the contract.
#[test]
fn every_required_context_is_reported_on_every_pull_request() {
    let mut reported = std::collections::BTreeMap::new();
    for (file, doc) in workflow_docs() {
        if doc["on"].get("pull_request").is_none() {
            continue;
        }
        let jobs = doc["jobs"].as_mapping().unwrap();
        for (id, job) in jobs {
            let id = id.as_str().unwrap();
            for ctx in job_contexts(id, job) {
                let gap = pr_trigger_gap(&doc).or_else(|| pr_job_gap(jobs, id));
                let prev = reported.insert(ctx.clone(), (file.clone(), gap));
                assert!(prev.is_none(), "context {ctx} is reported by two jobs");
            }
        }
    }
    for ctx in PR_CONTEXTS {
        match reported.get(*ctx) {
            None => panic!("no workflow job reports the required context {ctx}"),
            Some((file, Some(gap))) => {
                panic!("{file} may leave required context {ctx} unreported: {gap}")
            }
            Some((_, None)) => {}
        }
    }
}

/// AGENTS.md's required-checks bullet is the human-facing copy of the contract:
/// it names every `PR_CONTEXTS` entry (and no stale `check` context), so the list a
/// maintainer reads cannot drift from the one the workflows are held to. The
/// live branch protection is reconciled by governance's `--verify`, off-repo.
#[test]
fn agents_md_lists_the_required_context_contract() {
    let text = fs::read_to_string(repo_root().join("AGENTS.md")).unwrap();
    let start = text
        .find("- **Required status checks**")
        .expect("AGENTS.md has a Required status checks bullet");
    let bullet = &text[start..];
    let bullet = &bullet[..bullet[2..].find("\n- ").map_or(bullet.len(), |i| i + 2)];
    // Backticked names, with line-wrapping inside a name collapsed.
    let named: std::collections::BTreeSet<String> = bullet
        .split('`')
        .skip(1)
        .step_by(2)
        .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    for ctx in PR_CONTEXTS {
        assert!(
            named.contains(*ctx),
            "AGENTS.md's list omits {ctx}: {named:?}"
        );
    }
    assert!(
        !named.contains("check"),
        "AGENTS.md names a `check` context; the gate's context is `gate`"
    );
}

/// notignored is a review artifact, not a gate: its own workflow (a `needs` edge
/// cannot cross workflows, so no contract job can wait on it), skipping fork PRs
/// (whose read-only token cannot comment), and so never a required context.
#[test]
fn notignored_runs_on_pull_requests_outside_the_required_contexts() {
    let docs = workflow_docs();
    let (_, doc) = docs
        .iter()
        .find(|(f, _)| f == "notignored.yml")
        .expect(".github/workflows/notignored.yml exists");
    assert_eq!(pr_trigger_gap(doc), None);
    assert_eq!(doc["permissions"]["contents"].as_str(), Some("read"));
    assert_eq!(doc["permissions"]["pull-requests"].as_str(), Some("write"));
    let jobs = doc["jobs"].as_mapping().unwrap();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let (id, job) = jobs.iter().next().unwrap();
    assert_eq!(
        job["if"].as_str(),
        Some("github.event.pull_request.head.repo.full_name == github.repository")
    );
    let steps = job["steps"].as_sequence().unwrap();
    assert!(steps.iter().any(|s| s["uses"]
        .as_str()
        .is_some_and(|u| u.starts_with("actions/checkout@"))
        && s["with"]["fetch-depth"].as_u64() == Some(0)));
    assert!(steps
        .iter()
        .any(|s| s["uses"].as_str() == Some("nickderobertis/notignored@v0")));
    for ctx in job_contexts(id.as_str().unwrap(), job) {
        assert!(!PR_CONTEXTS.contains(&ctx.as_str()), "{ctx} is required");
    }
}
// llmlint: ignore-end[tests_mirror_real_usage]
