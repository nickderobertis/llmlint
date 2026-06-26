//! End-to-end tests: drive the **real `llmlint` binary** the way a user does,
//! against the deterministic `llmlint-mock-oneharness` fixture (the genuinely
//! external boundary) via `--oneharness-bin`. No network, no real LLM. Every
//! user-facing journey — happy path and failure/recovery — lands here as the
//! source of truth for what's covered (see `tests/AGENTS.md`).

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn mock_path() -> PathBuf {
    cargo_bin("llmlint-mock-oneharness")
}

/// A throwaway project directory with helpers to write configs/fixtures and
/// build llmlint invocations pointed at the mock.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        Project {
            dir: TempDir::new().unwrap(),
        }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, rel: &str, contents: &str) -> &Self {
        let p = self.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
        self
    }
    fn write_verdicts(&self, json: &str) -> PathBuf {
        let p = self.path().join("verdicts.json");
        fs::write(&p, json).unwrap();
        p
    }
    /// A bare llmlint command (cwd = project), no oneharness wiring — for
    /// subcommands like `init`/`config` that take no `--oneharness-bin`.
    fn bare(&self) -> Command {
        let mut c = Command::cargo_bin("llmlint").unwrap();
        c.current_dir(self.path());
        c
    }
    /// A default-`lint` command wired to the mock harness. Output lists failing
    /// rules + the summary; passed/skipped rules are only counted.
    fn lint(&self) -> Command {
        let mut c = self.bare();
        c.arg("--oneharness-bin").arg(mock_path());
        c
    }
    /// `lint` at `-v`: itemizes every rule (passed/skipped too) and prints the
    /// oneharness debug view to stderr.
    fn lint_v(&self) -> Command {
        let mut c = self.lint();
        c.arg("-v");
        c
    }
}

const RULE: &str = "true when ok; false otherwise.";

/// The bundled config-lint plugin, referenced by URL + version pin (resolved
/// offline from the binary's embedded copy).
const CONFIG_LINT: &str =
    "https://raw.githubusercontent.com/nickderobertis/llmlint/main/assets/config_lint.yml@1";

/// A throwaway localhost HTTP server for the plugin-fetch journey: serves one
/// fixed body to every GET and counts requests, so a test can assert that a
/// cached pin is not refetched. This exercises the real HTTPS-client fetch path
/// (localhost only — no external network).
struct HttpServer {
    base_url: String,
    hits: Arc<AtomicUsize>,
}

impl HttpServer {
    fn serve(body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = Arc::clone(&hits);
        let body = body.to_string();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 1024]; // read + discard the request head
                let _ = stream.read(&mut buf);
                hits_thread.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        HttpServer {
            base_url: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// Build a valid `file://` URL from a path on any platform: forward slashes,
/// with a leading slash before a Windows drive letter. Embedding the raw
/// `Path::display()` (with `\` on Windows) in a double-quoted YAML scalar would
/// be misread as an escape, so always normalize here.
fn file_url(path: &Path) -> String {
    let s = path.display().to_string().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

// ---- happy path ----------------------------------------------------------

#[test]
fn all_rules_pass_exits_zero() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n  \
             - {{ name: b_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true, "b_rule": true}"#);

    // Default verbosity: passing rules are only counted, so the output is just
    // the summary line — no `PASS`/`FAIL` lines at all.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 rules: 2 passed, 0 failed, 0 skipped",
        ))
        .stdout(predicate::str::contains("PASS").not())
        .stdout(predicate::str::contains("FAIL").not());
}

#[test]
fn violation_fails_with_file_and_line() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_inline_sql, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/db.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"no_inline_sql": {"holds": false, "violations": [
            {"file": "src/db.rs", "line": 12, "message": "inline SQL"}]}}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL no_inline_sql"))
        .stdout(predicate::str::contains("src/db.rs:12: inline SQL"));
}

#[test]
fn default_shows_failures_and_verbose_adds_rules_and_debug() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: passing_rule, description: \"{RULE}\" }}\n  \
             - {{ name: failing_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"passing_rule": true,
            "failing_rule": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 1, "message": "nope"}]}}"#,
    );

    // Default: failing rules + locations + summary, but passing rules are only
    // counted (not itemized) and there is no oneharness debug on stderr.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL failing_rule"))
        .stdout(predicate::str::contains("src/lib.rs:1: nope"))
        .stdout(predicate::str::contains(
            "2 rules: 1 passed, 1 failed, 0 skipped",
        ))
        .stdout(predicate::str::contains("PASS passing_rule").not())
        .stderr(predicate::str::contains("oneharness:").not());

    // `-v`: every rule is itemized on stdout, and the oneharness debug view
    // (exact command + raw result) is printed to stderr.
    p.lint_v()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("PASS passing_rule"))
        .stdout(predicate::str::contains("FAIL failing_rule"))
        .stdout(predicate::str::contains("src/lib.rs:1: nope"))
        // The debug view goes to stderr: the exact `oneharness run …` command
        // and the raw JSON result it returned.
        .stderr(predicate::str::contains("# oneharness: agent default"))
        .stderr(predicate::str::contains("run --system"))
        .stderr(predicate::str::contains("result:"))
        .stderr(predicate::str::contains("\"oneharness_version\":\"mock\""));
}

#[test]
fn color_is_off_when_piped_but_forced_by_color_always() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: passing_rule, description: \"{RULE}\" }}\n  \
             - {{ name: failing_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"passing_rule": true,
            "failing_rule": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 1, "message": "nope"}]}}"#,
    );

    // `assert_cmd` captures stdout through a pipe (not a terminal), so the
    // default `auto` policy resolves to no color: the report is plain text.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL failing_rule"))
        .stdout(predicate::str::contains('\u{1b}').not());

    // `--color always` forces ANSI even through a pipe: the FAIL word is wrapped
    // in red (bold `1m` + red `31m`) and the summary's failed count is red too,
    // while the passing count is green (`32m`).
    p.lint()
        .arg("--color")
        .arg("always")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "\u{1b}[1m\u{1b}[31mFAIL\u{1b}[0m failing_rule",
        ))
        .stdout(predicate::str::contains(
            "\u{1b}[1m\u{1b}[31m1 failed\u{1b}[0m",
        ))
        .stdout(predicate::str::contains(
            "\u{1b}[1m\u{1b}[32m1 passed\u{1b}[0m",
        ));

    // `--color never` keeps it plain even if a terminal would otherwise color,
    // and `NO_COLOR` disables `auto` regardless of the stream.
    p.lint()
        .arg("--color")
        .arg("never")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains('\u{1b}').not());
    p.lint()
        .arg("--color")
        .arg("always")
        .env("NO_COLOR", "1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        // `always` still wins over NO_COLOR (NO_COLOR only governs `auto`).
        .stdout(predicate::str::contains('\u{1b}'));
}

// ---- multi-judge majority vote -------------------------------------------

#[test]
fn majority_vote_flips_a_single_dissent_to_pass() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: voted_rule, description: \"{RULE}\", judges: 3 }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Three sequential judges: fail, pass, pass -> majority pass.
    let verdicts = p.write_verdicts(r#"{"voted_rule": [false, true, true]}"#);
    let state = p.path().join("state");

    p.lint_v()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS voted_rule"));
}

#[test]
fn majority_vote_fails_when_most_judges_dissent() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: voted_rule, description: \"{RULE}\", judges: 3 }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"voted_rule": [{"holds": false, "violations": [{"message": "bad"}]}, true, false]}"#,
    );
    let state = p.path().join("state");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL voted_rule (1/3 judges held)",
        ));
}

#[test]
fn multi_judge_failure_shows_each_judges_result_and_rationale() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: voted_rule, description: \"{RULE}\", judges: 3 }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Three sequential judges with distinct rationales: violate, hold, violate
    // -> majority fail. Each judge's result + rationale must be itemized.
    let verdicts = p.write_verdicts(
        r#"{"voted_rule": [
            {"holds": false, "rationale": "raw SQL at lib.rs:1",
                "violations": [{"file": "src/lib.rs", "line": 1, "message": "inline SQL"}]},
            {"holds": true, "rationale": "uses the query layer"},
            {"holds": false, "rationale": "string-built query"}
        ]}"#,
    );
    let state = p.path().join("state");

    let out = p
        .lint()
        .arg("--max-parallel")
        .arg("1")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // The machine contract carries every judge's holds + rationale, in order.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let judges = v["rules"][0]["judges"].as_array().unwrap();
    assert_eq!(judges.len(), 3);
    assert_eq!(judges[0]["holds"], false);
    assert_eq!(judges[0]["rationale"], "raw SQL at lib.rs:1");
    assert_eq!(judges[1]["holds"], true);
    assert_eq!(judges[1]["rationale"], "uses the query layer");

    // The default human report itemizes each judge at the failure (no `-v`).
    let state2 = p.path().join("state2");
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state2)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL voted_rule (1/3 judges held)",
        ))
        .stdout(predicate::str::contains(
            "judge 1 violated: raw SQL at lib.rs:1",
        ))
        .stdout(predicate::str::contains(
            "judge 2 held: uses the query layer",
        ))
        .stdout(predicate::str::contains(
            "judge 3 violated: string-built query",
        ))
        .stdout(predicate::str::contains("src/lib.rs:1: inline SQL"));
}

// ---- includes / plugin system --------------------------------------------

#[test]
fn includes_merge_rules_from_another_file() {
    let p = Project::new();
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: team_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - ./team.yml\nrules:\n  \
             - {{ name: root_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"root_rule": true, "team_rule": true}"#);

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"root_rule"));
    assert!(names.contains(&"team_rule"));
}

#[test]
fn config_lint_plugin_catches_a_bad_rule() {
    let p = Project::new();
    // The bundled plugin lints config files; it always runs against llmlint.yml.
    p.write(
        "llmlint.yml",
        &format!("version: 1\nplugins:\n  - {CONFIG_LINT}\n"),
    );
    let verdicts = p.write_verdicts(
        r#"{"name_is_descriptive_not_placeholder":
              {"holds": false, "violations": [{"file": "llmlint.yml", "message": "rule named 'foo'"}]},
            "description_states_clear_true_and_false": true,
            "name_matches_description": true}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL name_is_descriptive_not_placeholder",
        ))
        .stdout(predicate::str::contains("rule named 'foo'"));
}

#[test]
fn plugin_from_a_file_url_merges_its_rules() {
    let p = Project::new();
    let plugin = p.path().join("shared.yml");
    fs::write(
        &plugin,
        format!("version: 1\nrules:\n  - {{ name: shared_rule, description: \"{RULE}\" }}\n"),
    )
    .unwrap();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - \"{}@1\"\nrules:\n  \
             - {{ name: local_rule, description: \"{RULE}\" }}\n",
            file_url(&plugin)
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"local_rule": true, "shared_rule": true}"#);

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_CACHE_DIR", p.path().join("cache"))
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"local_rule"));
    assert!(names.contains(&"shared_rule"));
}

#[test]
fn plugin_version_mismatch_is_an_error() {
    let p = Project::new();
    let plugin = p.path().join("shared.yml");
    // Declares version 2, but the config pins @1 -> hard error.
    fs::write(
        &plugin,
        format!("version: 2\nrules:\n  - {{ name: shared_rule, description: \"{RULE}\" }}\n"),
    )
    .unwrap();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nplugins:\n  - \"{}@1\"\n", file_url(&plugin)),
    );
    p.lint()
        .env("LLMLINT_CACHE_DIR", p.path().join("cache"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "requested version 1 but the config declares version 2",
        ));
}

#[test]
fn removed_llmlint_scheme_is_a_clear_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nplugins:\n  - llmlint:config-lint\n",
    );
    p.lint().assert().code(2).stderr(predicate::str::contains(
        "the `llmlint:` plugin scheme was removed",
    ));
}

#[test]
fn renamed_top_level_include_key_is_rejected() {
    let p = Project::new();
    p.write("llmlint.yml", "version: 1\ninclude:\n  - ./team.yml\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("renamed to `plugins`"));
}

#[test]
fn override_extends_a_plugin_rule_by_name() {
    let p = Project::new();
    // The plugin contributes `shared_rule` with full text and 1 judge.
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: shared_rule, description: \"{RULE}\", judges: 1 }}\n"),
    );
    // The root overrides it: bump judges, omit the description to inherit it.
    p.write(
        "llmlint.yml",
        "version: 1\nplugins:\n  - ./team.yml\nrules:\n  \
         - { name: shared_rule, override: true, judges: 3 }\n",
    );
    let out = p.bare().arg("config").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules = v["config"]["rules"].as_array().unwrap();
    // Resolved to a single rule with the override's judges and the plugin's text.
    let matches: Vec<&Value> = rules
        .iter()
        .filter(|r| r["name"] == "shared_rule")
        .collect();
    assert_eq!(matches.len(), 1, "override should collapse to one rule");
    assert_eq!(matches[0]["judges"], 3);
    assert_eq!(matches[0]["description"], RULE);
    // The `override` flag is resolved away, not echoed back.
    assert!(matches[0].get("override").is_none());
}

#[test]
fn override_changes_the_actual_lint_run() {
    // Beyond the config dump: prove a resolved override reaches the planner and
    // changes execution. The plugin ships a 1-judge rule; the root overrides it
    // to 3 judges (inheriting the description), and the run executes all three.
    let p = Project::new();
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: voted_rule, description: \"{RULE}\", judges: 1 }}\n"),
    );
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - ./team.yml\nrules:\n  \
         - { name: voted_rule, override: true, judges: 3 }\n",
    );
    p.write("src/lib.rs", "// code\n");
    // Three verdicts -> the override took effect (a 1-judge run takes one).
    let verdicts = p.write_verdicts(r#"{"voted_rule": [true, true, true]}"#);
    let state = p.path().join("state");

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules = v["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1, "override should collapse to one rule");
    assert_eq!(rules[0]["name"], "voted_rule");
    let judges = rules[0]["judges"].as_array().unwrap();
    assert_eq!(judges.len(), 3, "override should have bumped judges 1 -> 3");
}

#[test]
fn duplicate_rule_name_without_override_is_an_error() {
    let p = Project::new();
    p.write(
        "team.yml",
        &format!("rules:\n  - {{ name: shared_rule, description: \"{RULE}\" }}\n"),
    );
    // Re-declares `shared_rule` without `override` -> a clear exit-2 error.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nplugins:\n  - ./team.yml\nrules:\n  \
             - {{ name: shared_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.bare()
        .arg("config")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"))
        .stderr(predicate::str::contains("override: true"));
}

#[test]
fn override_without_a_base_rule_is_an_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  - { name: orphan, override: true }\n",
    );
    p.bare()
        .arg("config")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no base rule"));
}

#[test]
fn pinned_url_plugin_is_fetched_over_http_and_cached() {
    let p = Project::new();
    let server = HttpServer::serve(&format!(
        "version: 1\nrules:\n  - {{ name: remote_rule, description: \"{RULE}\" }}\n"
    ));
    let url = server.url("/rules.yml");
    let cache = p.path().join("cache");
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - \"{url}@1\"\nrules:\n  \
             - {{ name: local_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"local_rule": true, "remote_rule": true}"#);

    let run = || {
        p.lint()
            .arg("--format")
            .arg("json")
            .env("LLMLINT_CACHE_DIR", &cache)
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .env("HTTP_PROXY", "")
            .env("http_proxy", "")
            .env("LLMLINT_MOCK_VERDICTS", &verdicts)
            .output()
            .unwrap()
    };

    let out = run();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"remote_rule"));
    assert_eq!(server.hits(), 1, "first run should fetch exactly once");

    // Second run reuses the cached pin: the server sees no further requests.
    let out = run();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(server.hits(), 1, "an unchanged pin must not refetch");
}

// ---- file selection -------------------------------------------------------

#[test]
fn include_exclude_globs_select_the_right_files() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**/*.rs\"]\n  exclude: [\"**/gen.rs\"]\nrules:\n  \
             - {{ name: scoped_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "// a\n");
    p.write("src/gen.rs", "// generated\n");
    let verdicts = p.write_verdicts(r#"{"scoped_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("src/a.rs"), "system:\n{system}");
    assert!(
        !system.contains("gen.rs"),
        "excluded file leaked:\n{system}"
    );
}

#[test]
fn explicit_cli_files_override_config_globs() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: cli_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "// a\n");
    p.write("README.md", "# readme\n");
    let verdicts = p.write_verdicts(r#"{"cli_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("README.md")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("README.md"), "system:\n{system}");
    assert!(
        !system.contains("src/a.rs"),
        "config glob should be overridden"
    );
}

// ---- filters --------------------------------------------------------------

#[test]
fn rule_filter_limits_which_rules_run() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: keep_rule, description: \"{RULE}\" }}\n  \
             - {{ name: drop_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"keep_rule": true, "drop_rule": true}"#);

    let out = p
        .lint()
        .arg("--rule")
        .arg("keep_rule")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["keep_rule"]);
}

#[test]
fn rules_with_no_matching_files_are_skipped() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"does-not-exist/**\"]\nrules:\n  \
             - {{ name: lonely_rule, description: \"{RULE}\" }}\n"
        ),
    );
    let verdicts = p.write_verdicts(r#"{"lonely_rule": true}"#);

    p.lint_v()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("SKIP lonely_rule"));
}

// ---- init -----------------------------------------------------------------

#[test]
fn init_scaffolds_a_config_then_refuses_to_clobber() {
    let p = Project::new();
    p.bare().arg("init").assert().success();
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    assert!(cfg.contains("plugins:"));
    assert!(cfg.contains("config_lint.yml@1"));
    // The `$schema` modeline leads the file so editors validate against the
    // published JSON Schema.
    assert!(cfg.starts_with(
        "# yaml-language-server: $schema=https://raw.githubusercontent.com/\
         nickderobertis/llmlint/main/assets/llmlint.schema.json"
    ));

    // Second init without --force fails (exit 2).
    p.bare().arg("init").assert().code(2);
    // --force overwrites.
    p.bare().arg("init").arg("--force").assert().success();
}

#[test]
fn init_with_template_embeds_the_prompt_template() {
    let p = Project::new();
    p.bare()
        .arg("init")
        .arg("--with-template")
        .assert()
        .success();
    let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
    assert!(cfg.contains("prompt_template: |"));
    assert!(cfg.contains("{% for r in rules %}"));
}

#[test]
fn init_then_self_lint_is_clean() {
    let p = Project::new();
    p.bare().arg("init").assert().success();
    // The example rule targets src/** (empty here -> skipped); the config-lint
    // plugin targets the config file. Mock holds everything -> exit 0.
    let verdicts = p.write_verdicts(
        r#"{"description_states_clear_true_and_false": true,
            "name_is_descriptive_not_placeholder": true,
            "name_matches_description": true,
            "public_items_are_documented": true}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

// ---- config / doctor ------------------------------------------------------

#[test]
fn config_command_prints_merged_config_and_sources() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nplugins:\n  - {CONFIG_LINT}\nrules:\n  \
             - {{ name: my_rule, description: \"{RULE}\" }}\n"
        ),
    );
    let out = p.bare().arg("config").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["config_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s == CONFIG_LINT));
    let names: Vec<&str> = v["config"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"my_rule"));
    assert!(names.contains(&"name_matches_description"));
}

#[test]
fn doctor_reports_the_harness_version() {
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .assert()
        .success()
        .stdout(predicate::str::contains("oneharness"));
}

#[test]
fn doctor_fails_clearly_when_oneharness_is_missing() {
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", "/nonexistent/oneharness-xyz")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("oneharness not found"));
}

// ---- failure / recovery ---------------------------------------------------

#[test]
fn missing_config_is_a_usage_error() {
    let p = Project::new();
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no llmlint config found"));
}

#[test]
fn malformed_config_is_a_usage_error() {
    let p = Project::new();
    p.write("llmlint.yml", "rules:\n  - name: r\n    bogus_field: 1\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("config"));
}

#[test]
fn duplicate_rule_names_are_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n"
        ),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"));
}

#[test]
fn schema_validation_failure_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_FAIL_SCHEMA", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("failed schema validation"));
}

#[test]
fn missing_structured_output_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_NO_STRUCTURED", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no structured output"));
}

#[test]
fn unparseable_oneharness_output_is_surfaced() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_GARBAGE", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "could not parse oneharness output",
        ));
}

#[test]
fn verbose_debug_view_is_shown_even_when_a_judge_errors() {
    // At `-v`, a judge that produces unusable output still errors the run
    // (exit 2, error on stdout) AND its exact command + raw result are traced
    // to stderr, so the failure can be debugged.
    let p = lint_project();
    p.lint_v()
        .env("LLMLINT_MOCK_GARBAGE", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "could not parse oneharness output",
        ))
        .stderr(predicate::str::contains("# oneharness: agent default"))
        .stderr(predicate::str::contains("run --system"))
        // The raw (unparseable) result the mock printed is captured verbatim.
        .stderr(predicate::str::contains("result:"))
        .stderr(predicate::str::contains("this is not json"));
}

#[test]
fn json_format_is_unaffected_by_verbosity() {
    // `-v` must not leak the debug view into the JSON report: stdout stays pure
    // JSON, and the oneharness trace still goes to stderr.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: j_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts =
        p.write_verdicts(r#"{"j_rule": {"holds": false, "violations": [{"message": "nope"}]}}"#);

    let out = p
        .lint_v()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // stdout parses cleanly as the JSON report (no debug text mixed in).
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["failed"], 1);
    assert_eq!(v["rules"][0]["name"], "j_rule");
    // The debug view is still emitted, but on stderr.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("# oneharness:"));
    assert!(err.contains("run --system"));
}

/// A minimal one-rule project used by the failure-path tests.
fn lint_project() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: some_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p
}

// ---- additional coverage: filters, passthrough, more failure shapes -------

#[test]
fn empty_rule_selection_exits_zero() {
    // A *valid* but empty selection: `default_rule` exists and agent `special`
    // exists, but the rule isn't assigned to that agent, so they don't
    // intersect. That is a legitimate "nothing to run" -> exit 0, distinct from
    // a typo'd name (which is an error; see below).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  special:\n    \
             harness: claude-code\nrules:\n  \
             - {{ name: default_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .arg("--rule")
        .arg("default_rule")
        .arg("--agent")
        .arg("special")
        .assert()
        .success()
        .stdout(predicate::str::contains("0 rules"));
}

#[test]
fn unknown_rule_name_is_an_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: only_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .arg("--rule")
        .arg("nonexistent")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no rule named nonexistent"))
        .stderr(predicate::str::contains("available rules: only_rule"));
}

#[test]
fn unknown_agent_name_is_an_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  special:\n    \
             harness: claude-code\nrules:\n  \
             - {{ name: only_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .arg("--agent")
        .arg("typo")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no agent named typo"))
        .stderr(predicate::str::contains(
            "available agents: default, special",
        ));
}

#[test]
fn rule_filter_is_repeatable() {
    // `--rule` is documented as repeatable: two flags select exactly those two
    // of three rules (the headline "target individual rules" capability).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: keep_a, description: \"{RULE}\" }}\n  \
             - {{ name: keep_b, description: \"{RULE}\" }}\n  \
             - {{ name: drop_c, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"keep_a": true, "keep_b": true, "drop_c": true}"#);

    let out = p
        .lint()
        .arg("--rule")
        .arg("keep_a")
        .arg("--rule")
        .arg("keep_b")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["keep_a", "keep_b"]);
}

#[test]
fn partial_unknown_rule_is_an_error() {
    // One valid name does not excuse a typo in another `--rule`: the typo is
    // still an exit-2 error (it would otherwise be a silent false green).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: only_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .arg("--rule")
        .arg("only_rule")
        .arg("--rule")
        .arg("typo")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no rule named typo"));
}

#[test]
fn agent_default_selects_only_unassigned_rules() {
    // `--agent default` is the documented way to target rules with no explicit
    // agent, even when other agents are declared.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  special:\n    \
             harness: claude-code\nrules:\n  \
             - {{ name: free_rule, description: \"{RULE}\" }}\n  \
             - {{ name: special_rule, description: \"{RULE}\", agent: special }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"free_rule": true, "special_rule": true}"#);
    let out = p
        .lint()
        .arg("--agent")
        .arg("default")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["free_rule"]);
}

#[test]
fn agent_filter_selects_only_that_agent() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  special:\n    \
             harness: claude-code\nrules:\n  \
             - {{ name: default_rule, description: \"{RULE}\" }}\n  \
             - {{ name: special_rule, description: \"{RULE}\", agent: special }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"default_rule": true, "special_rule": true}"#);
    let out = p
        .lint()
        .arg("--agent")
        .arg("special")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["special_rule"]);
}

#[test]
fn oneharness_passthrough_args_are_accepted() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  model: gpt-5\n  \
             schema_max_retries: 1\n  config: [\"./oh.toml\"]\nagents:\n  a:\n    model: opus\n\
             rules:\n  - {{ name: passthrough_rule, description: \"{RULE}\", agent: a }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write("oh.toml", "model = \"gpt-5\"\n");
    let verdicts = p.write_verdicts(r#"{"passthrough_rule": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn harness_is_omitted_when_unset_and_forwarded_when_set() {
    let p = Project::new();
    // Two agents: one leaves harness unset, one pins it.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  pinned:\n    \
             harness: codex\nrules:\n  \
             - {{ name: default_rule, description: \"{RULE}\" }}\n  \
             - {{ name: pinned_rule, description: \"{RULE}\", agent: pinned }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"default_rule": true, "pinned_rule": true}"#);

    // Unset agent: --rule limits the run to one judge so the dump is unambiguous.
    let unset_args = p.path().join("unset-args.txt");
    p.lint()
        .arg("--rule")
        .arg("default_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &unset_args)
        .assert()
        .success();
    let unset = fs::read_to_string(&unset_args).unwrap();
    assert!(
        !unset.lines().any(|l| l == "--harness"),
        "expected no --harness flag when the agent leaves it unset, got:\n{unset}"
    );

    // Pinned agent: the configured harness id is forwarded verbatim.
    let pinned_args = p.path().join("pinned-args.txt");
    p.lint()
        .arg("--rule")
        .arg("pinned_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &pinned_args)
        .assert()
        .success();
    let pinned = fs::read_to_string(&pinned_args).unwrap();
    let harness_val = pinned
        .lines()
        .skip_while(|l| *l != "--harness")
        .nth(1)
        .expect("--harness flag should be present for a pinned agent");
    assert_eq!(harness_val, "codex");
}

#[test]
fn multiple_oneharness_configs_warns() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  config: [\"./a.toml\"]\n\
             rules:\n  - {{ name: warn_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"warn_rule": true}"#);
    p.lint()
        .arg("--oneharness-config")
        .arg("./b.toml")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stderr(predicate::str::contains("single file"));
}

#[test]
fn oneharness_bin_from_env_is_used() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: env_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"env_rule": true}"#);
    // No --oneharness-bin; resolve it from the environment instead.
    p.bare()
        .arg("-v")
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS env_rule"));
}

#[test]
fn empty_results_from_oneharness_is_an_error() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_EMPTY_RESULTS", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no results"));
}

#[test]
fn bad_verdict_shape_is_an_error() {
    let p = lint_project();
    p.lint()
        .env("LLMLINT_MOCK_BAD_SHAPE", "1")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("invalid verdict shape"));
}

#[test]
fn config_command_rejects_invalid_config() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: dup, description: \"{RULE}\" }}\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n"
        ),
    );
    p.bare()
        .arg("config")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"));
}

#[test]
fn config_command_rejects_even_judges() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: tied, description: \"{RULE}\", judges: 2 }}\n"),
    );
    p.bare()
        .arg("config")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must be odd"));
}

#[test]
fn init_writes_to_a_custom_output_path() {
    let p = Project::new();
    p.bare()
        .arg("init")
        .arg("--output")
        .arg("config/nested/custom.yml")
        .assert()
        .success();
    assert!(p.path().join("config/nested/custom.yml").is_file());
}

#[test]
fn init_global_uses_xdg_config_home() {
    let p = Project::new();
    let xdg = p.path().join("xdg");
    p.bare()
        .arg("init")
        .arg("--global")
        .env("XDG_CONFIG_HOME", &xdg)
        .assert()
        .success();
    assert!(xdg.join("llmlint/llmlint.yml").is_file());
}

#[cfg(unix)]
#[test]
fn init_global_falls_back_to_home_config() {
    let p = Project::new();
    let home = p.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    p.bare()
        .arg("init")
        .arg("--global")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", &home)
        .assert()
        .success();
    assert!(home.join(".config/llmlint/llmlint.yml").is_file());
}

// ---- explicit --config (replaces discovery; repeatable merge) -------------

#[test]
fn explicit_config_flag_replaces_discovery_and_merges_multiple() {
    let p = Project::new();
    // No config at a default name/location; the configs live elsewhere and are
    // named so upward discovery would never find them.
    p.write(
        "configs/base.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: base_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "configs/extra.yml",
        &format!("rules:\n  - {{ name: extra_rule, description: \"{RULE}\" }}\n"),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"base_rule": true, "extra_rule": true}"#);

    // Two `--config` entries: the first supplies top-level scalars, both
    // contribute rules. Discovery is replaced entirely.
    let out = p
        .lint()
        .arg("--config")
        .arg("configs/base.yml")
        .arg("--config")
        .arg("configs/extra.yml")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"base_rule"), "got: {names:?}");
    assert!(names.contains(&"extra_rule"), "got: {names:?}");
}

#[test]
fn config_command_honors_explicit_config_and_cwd() {
    let p = Project::new();
    // Config lives under a non-default name in a subdir; nothing at the root.
    p.write(
        "proj/custom.yml",
        &format!("version: 1\nrules:\n  - {{ name: explicit_rule, description: \"{RULE}\" }}\n"),
    );
    let proj = p.path().join("proj");
    // Process cwd is the project root; `--cwd` is the base both for discovery
    // *and* for resolving the relative `--config` path. If `--cwd` were ignored,
    // `custom.yml` would resolve against the root and fail to load.
    let out = p
        .bare()
        .arg("config")
        .arg("--cwd")
        .arg(&proj)
        .arg("--config")
        .arg("custom.yml")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["config"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"explicit_rule"), "got: {names:?}");
}

// ---- --cwd (config discovery + the harness working directory) -------------

#[test]
fn cwd_flag_drives_discovery_and_the_harness_directory() {
    let p = Project::new();
    p.write(
        "proj/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: cwd_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("proj/src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"cwd_rule": true}"#);
    let args_dump = p.path().join("args.txt");
    let proj = p.path().join("proj");

    // The process runs from the project root (no config there); `--cwd ./proj`
    // is where discovery happens. Success proves discovery used `--cwd`.
    p.lint_v()
        .arg("--cwd")
        .arg(&proj)
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS cwd_rule"));

    // And the same directory is forwarded to oneharness as its `--cwd`.
    let dumped = fs::read_to_string(&args_dump).unwrap();
    let cwd_val = dumped
        .lines()
        .skip_while(|l| *l != "--cwd")
        .nth(1)
        .expect("--cwd flag should be forwarded to oneharness");
    assert_eq!(Path::new(cwd_val), proj);
}

// ---- --timeout (forwarded to oneharness) ----------------------------------

#[test]
fn timeout_flag_is_forwarded_to_oneharness() {
    let p = lint_project();
    let args_dump = p.path().join("args.txt");
    p.lint()
        .arg("--timeout")
        .arg("7")
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();
    let dumped = fs::read_to_string(&args_dump).unwrap();
    let timeout_val = dumped
        .lines()
        .skip_while(|l| *l != "--timeout")
        .nth(1)
        .expect("--timeout flag should be forwarded to oneharness");
    assert_eq!(timeout_val, "7");
}

// ---- per-rule / per-agent files precedence over global globs --------------

#[test]
fn per_rule_files_override_global_globs() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: scoped, description: \"{RULE}\", files: {{ include: [\"only/**\"] }} }}\n"
        ),
    );
    p.write("src/app.rs", "// app\n");
    p.write("only/special.rs", "// special\n");
    let verdicts = p.write_verdicts(r#"{"scoped": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("only/special.rs"), "system:\n{system}");
    assert!(
        !system.contains("src/app.rs"),
        "per-rule files should override the global glob:\n{system}"
    );
}

#[test]
fn per_agent_files_override_global_globs() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  docs:\n    \
             files:\n      include: [\"docs/**\"]\nrules:\n  \
             - {{ name: doc_rule, description: \"{RULE}\", agent: docs }}\n"
        ),
    );
    p.write("src/app.rs", "// app\n");
    p.write("docs/guide.md", "# guide\n");
    let verdicts = p.write_verdicts(r#"{"doc_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("docs/guide.md"), "system:\n{system}");
    assert!(
        !system.contains("src/app.rs"),
        "per-agent files should override the global glob:\n{system}"
    );
}

// ---- model passthrough (global default + per-agent override) --------------

#[test]
fn model_is_forwarded_with_agent_override_taking_precedence() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  model: global-model\n\
             agents:\n  pinned:\n    model: agent-model\nrules:\n  \
             - {{ name: global_model_rule, description: \"{RULE}\" }}\n  \
             - {{ name: pinned_model_rule, description: \"{RULE}\", agent: pinned }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"global_model_rule": true, "pinned_model_rule": true}"#);

    // Default agent inherits the global oneharness model.
    let global_args = p.path().join("global-args.txt");
    p.lint()
        .arg("--rule")
        .arg("global_model_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &global_args)
        .assert()
        .success();
    let g = fs::read_to_string(&global_args).unwrap();
    let global_model = g
        .lines()
        .skip_while(|l| *l != "--model")
        .nth(1)
        .expect("--model should be forwarded for the global default");
    assert_eq!(global_model, "global-model");

    // A per-agent model overrides the global one.
    let pinned_args = p.path().join("pinned-args.txt");
    p.lint()
        .arg("--rule")
        .arg("pinned_model_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &pinned_args)
        .assert()
        .success();
    let pinned = fs::read_to_string(&pinned_args).unwrap();
    let pinned_model = pinned
        .lines()
        .skip_while(|l| *l != "--model")
        .nth(1)
        .expect("--model should be forwarded for a pinned agent");
    assert_eq!(pinned_model, "agent-model");
}

// ---- JSON output contract (failure + run-error shapes) --------------------

#[test]
fn json_output_reports_violations_and_summary_on_failure() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n  \
             - {{ name: documented, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"no_todo": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 3, "message": "stray TODO"}]},
            "documented": true}"#,
    );

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    // Summary counts are part of the machine-readable contract.
    assert_eq!(v["summary"]["total"], 2);
    assert_eq!(v["summary"]["passed"], 1);
    assert_eq!(v["summary"]["failed"], 1);
    assert_eq!(v["summary"]["errored"], 0);
    assert!(v["errors"].as_array().unwrap().is_empty());
    // The failing rule carries its outcome and located violation.
    let failing = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "no_todo")
        .expect("failing rule present");
    assert_eq!(failing["outcome"], "fail");
    let viol = &failing["violations"][0];
    assert_eq!(viol["file"], "src/lib.rs");
    assert_eq!(viol["line"], 3);
    assert_eq!(viol["message"], "stray TODO");
}

#[test]
fn json_output_reports_run_errors_and_exits_two() {
    let p = lint_project();
    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_NO_STRUCTURED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["errored"], 1);
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "one judge run errored: {errors:?}");
    assert!(
        errors[0].as_str().unwrap().contains("no structured output"),
        "errors: {errors:?}"
    );
}

// ---- custom prompt templates reach the judge ------------------------------

#[test]
fn global_prompt_template_override_reaches_the_judge() {
    let p = Project::new();
    // A custom top-level template entirely replaces the bundled one.
    p.write(
        "llmlint.yml",
        "version: 1\n\
         prompt_template: |\n  \
           GLOBAL_TEMPLATE_MARKER\n  \
           {% for r in rules %}rule={{ r.name }}\n  \
           {% endfor %}\n\
         files:\n  include: [\"src/**\"]\n\
         rules:\n  \
           - name: templated_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"templated_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("GLOBAL_TEMPLATE_MARKER"),
        "custom template should drive the prompt:\n{system}"
    );
    assert!(system.contains("rule=templated_rule"), "system:\n{system}");
}

#[test]
fn agent_prompt_template_is_appended_for_its_rules() {
    let p = Project::new();
    // The agent's `prompt_template` is extra text appended to the master template.
    p.write(
        "llmlint.yml",
        "version: 1\n\
         files:\n  include: [\"src/**\"]\n\
         agents:\n  reviewer:\n    prompt_template: \"AGENT_APPENDED_MARKER\"\n\
         rules:\n  \
           - name: reviewed_rule\n    \
             description: \"true when ok; false otherwise.\"\n    \
             agent: reviewer\n",
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"reviewed_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("AGENT_APPENDED_MARKER"),
        "agent prompt_template should be appended to the prompt:\n{system}"
    );
}

#[test]
fn yaml_anchors_merge_keys_and_stash_keys_resolve() {
    let p = Project::new();
    // An `x-` stash key holds anchors; one is aliased into an agent's appended
    // template, and a `<<` merge key folds shared agent fields in. All three are
    // resolved by the YAML layer before the config is parsed.
    p.write(
        "llmlint.yml",
        "version: 1\n\
         x-snippets:\n  guidance: &guidance \"ANCHORED_GUIDANCE_MARKER\"\n\
         x-defaults: &agent_defaults\n  harness: codex\n\
         files:\n  include: [\"src/**\"]\n\
         agents:\n  reviewer:\n    <<: *agent_defaults\n    prompt_template: *guidance\n\
         rules:\n  \
           - name: anchored_rule\n    \
             description: \"true when ok; false otherwise.\"\n    \
             agent: reviewer\n",
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"anchored_rule": true}"#);
    let dump = p.path().join("system.txt");
    let args_dump = p.path().join("args.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();

    // The aliased anchor reached the rendered prompt...
    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("ANCHORED_GUIDANCE_MARKER"),
        "aliased anchor should reach the prompt:\n{system}"
    );
    // ...and the `<<`-merged harness field took effect.
    let args = fs::read_to_string(&args_dump).unwrap();
    let harness = args
        .lines()
        .skip_while(|l| *l != "--harness")
        .nth(1)
        .expect("the merged harness should be forwarded");
    assert_eq!(harness, "codex");
}

// ---- batching (one oneharness call per batch) -----------------------------

#[test]
fn rules_for_one_agent_share_a_single_oneharness_call_by_default() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: rule_a, description: \"{RULE}\" }}\n  \
             - {{ name: rule_b, description: \"{RULE}\" }}\n  \
             - {{ name: rule_c, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"rule_a": true, "rule_b": true, "rule_c": true}"#);
    let runlog = p.path().join("runlog");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success();

    let calls = runlog_calls(&runlog);
    assert_eq!(
        calls.len(),
        1,
        "the default batch groups all three rules into one call: {calls:?}"
    );
    for name in ["rule_a", "rule_b", "rule_c"] {
        assert!(calls[0].contains(name), "call: {:?}", calls[0]);
    }
}

#[test]
fn batch_size_splits_rules_into_separate_oneharness_calls() {
    let p = Project::new();
    // batch_size 1 on the default agent forces one oneharness call per rule.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  default:\n    batch_size: 1\n\
             rules:\n  \
             - {{ name: rule_a, description: \"{RULE}\" }}\n  \
             - {{ name: rule_b, description: \"{RULE}\" }}\n  \
             - {{ name: rule_c, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"rule_a": true, "rule_b": true, "rule_c": true}"#);
    let runlog = p.path().join("runlog");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success();

    let calls = runlog_calls(&runlog);
    assert_eq!(calls.len(), 3, "one call per rule: {calls:?}");
    assert!(
        calls.iter().all(|c| !c.contains(',')),
        "each call should carry exactly one rule: {calls:?}"
    );
}

/// Read the per-invocation run-log files the mock wrote, one entry per
/// oneharness call (its comma-joined rule names).
fn runlog_calls(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect()
}

// ---- validation exit-2 journeys -------------------------------------------

#[test]
fn invalid_rule_name_is_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: \"bad-name\", description: \"{RULE}\" }}\n"),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a valid identifier"));
}

#[test]
fn empty_rule_description_is_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  - { name: blank_rule, description: \"   \" }\n",
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("empty description"));
}

#[test]
fn zero_judges_is_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: no_judge, description: \"{RULE}\", judges: 0 }}\n"
        ),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must be >= 1"));
}

#[test]
fn zero_batch_size_is_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nagents:\n  a:\n    batch_size: 0\nrules:\n  \
             - {{ name: r, description: \"{RULE}\", agent: a }}\n"
        ),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("batch_size: 0"));
}

#[test]
fn rule_referencing_unknown_agent_is_rejected() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: orphan, description: \"{RULE}\", agent: ghost }}\n"
        ),
    );
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown agent"));
}

// ---- inline `llmlint: ignore` directive journeys --------------------------

/// A project with one rule (`no_todo`) over `src/**`, plus `src/lib.rs` carrying
/// `body`. The directive *structure* is validated by llmlint; honoring it is the
/// judge's job, so the mock just returns the given verdict.
fn ignore_project(body: &str) -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", body);
    p
}

#[test]
fn well_formed_ignore_directives_pass_validation() {
    // Line- and file-scoped directives in the supported comment styles, plus a
    // prose mention of the marker that must NOT be treated as a directive.
    let p = ignore_project(
        "// llmlint: ignore[no_todo] tracked in JIRA-1\n\
         /* llmlint: ignore-file[no_todo] vendored, reviewed upstream */\n\
         // see llmlint: docs for the ignore feature\n",
    );
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn ignore_directive_without_brackets_is_rejected() {
    let p = ignore_project("// llmlint: ignore please skip this\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("llmlint: ignore"))
        .stderr(predicate::str::contains("brackets"))
        .stderr(predicate::str::contains("src/lib.rs:1:"));
}

#[test]
fn ignore_directive_with_empty_rule_list_is_rejected() {
    let p = ignore_project("// llmlint: ignore[] no rules named\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("at least one rule"));
}

#[test]
fn ignore_directive_naming_unknown_rule_is_rejected() {
    let p = ignore_project("// llmlint: ignore[no_todoo] typo'd rule name\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown rule"))
        .stderr(predicate::str::contains("configured rules: no_todo"));
}

#[test]
fn ignore_directive_without_reason_is_rejected() {
    let p = ignore_project("// llmlint: ignore[no_todo]\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("give a reason"));
}

#[test]
fn default_prompt_documents_ignore_directives() {
    let p = ignore_project("// code\n");
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    let dump = p.path().join("system.txt");
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let prompt = fs::read_to_string(&dump).unwrap();
    assert!(
        prompt.contains("llmlint: ignore["),
        "prompt should document the ignore directive: {prompt}"
    );
    assert!(
        prompt.contains("ignore-file"),
        "prompt should document the file-scoped form: {prompt}"
    );
}

#[test]
fn malformed_directive_in_an_excluded_file_does_not_fail() {
    // Only resolved *target* files are scanned. A malformed directive in an
    // excluded file must not fail the run.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\n  exclude: [\"src/generated/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write("src/generated/api.rs", "// llmlint: ignore[bogus]\n");
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn directive_naming_a_configured_but_unselected_rule_is_accepted() {
    // The known set is the full config, not the `--rule` selection, so a
    // directive may reference any configured rule even when this run skips it.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n  \
             - {{ name: no_sql, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "src/lib.rs",
        "// llmlint: ignore[no_sql] handled by the query layer\n",
    );
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    p.lint()
        .arg("--rule")
        .arg("no_todo")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn malformed_directives_across_files_all_report_before_any_judge_runs() {
    // Every malformed directive surfaces in one error (located per file:line),
    // and validation precedes oneharness — so no judge is invoked at all.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "// llmlint: ignore[no_todo]\n");
    p.write("src/b.rs", "// llmlint: ignore[ghost] a reason\n");
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    let runlog = p.path().join("runlog");
    fs::create_dir(&runlog).unwrap();

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("src/a.rs:1:"))
        .stderr(predicate::str::contains("src/b.rs:1:"))
        .stderr(predicate::str::contains("give a reason"))
        .stderr(predicate::str::contains("unknown rule"));

    // Fail-fast: validation ran before planning, so the mock was never invoked.
    assert_eq!(
        fs::read_dir(&runlog).unwrap().count(),
        0,
        "no oneharness call should happen when a directive is malformed"
    );
}

// ---- oneharness passthrough actually forwarded ----------------------------

#[test]
fn schema_max_retries_is_forwarded_to_oneharness() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  schema_max_retries: 4\n\
             rules:\n  - {{ name: retry_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"retry_rule": true}"#);
    let args_dump = p.path().join("args.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();

    let args = fs::read_to_string(&args_dump).unwrap();
    let retries = args
        .lines()
        .skip_while(|l| *l != "--schema-max-retries")
        .nth(1)
        .expect("--schema-max-retries should be forwarded");
    assert_eq!(retries, "4");
}

#[test]
fn config_timeout_is_forwarded_when_no_cli_flag() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  timeout: 33\n\
             rules:\n  - {{ name: timed_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"timed_rule": true}"#);
    let args_dump = p.path().join("args.txt");

    // No `--timeout` on the CLI: the config value must be used.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();

    let args = fs::read_to_string(&args_dump).unwrap();
    let timeout = args
        .lines()
        .skip_while(|l| *l != "--timeout")
        .nth(1)
        .expect("--timeout should be forwarded from config");
    assert_eq!(timeout, "33");
}

#[test]
fn config_level_oneharness_bin_is_used() {
    let p = Project::new();
    // The binary is resolved from `oneharness.bin` in the config alone — no
    // `--oneharness-bin` flag and no `LLMLINT_ONEHARNESS_BIN` env.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  bin: '{}'\n\
             rules:\n  - {{ name: bin_rule, description: \"{RULE}\" }}\n",
            mock_path().display()
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"bin_rule": true}"#);

    p.bare()
        .arg("-v") // `-v` itemizes the passing rule so we can assert on it
        .env_remove("LLMLINT_ONEHARNESS_BIN")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS bin_rule"));
}

// ---- plugin cache refresh -------------------------------------------------

#[test]
fn plugin_refresh_forces_a_refetch() {
    let p = Project::new();
    let server = HttpServer::serve(&format!(
        "version: 1\nrules:\n  - {{ name: remote_rule, description: \"{RULE}\" }}\n"
    ));
    let url = server.url("/rules.yml");
    let cache = p.path().join("cache");
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - \"{url}@1\"\nrules:\n  \
             - {{ name: local_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"local_rule": true, "remote_rule": true}"#);

    let run = |refresh: bool| {
        let mut c = p.lint();
        c.env("LLMLINT_CACHE_DIR", &cache)
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .env("HTTP_PROXY", "")
            .env("http_proxy", "")
            .env("LLMLINT_MOCK_VERDICTS", &verdicts);
        if refresh {
            c.env("LLMLINT_PLUGIN_REFRESH", "1");
        }
        c.assert().success();
    };

    run(false);
    assert_eq!(server.hits(), 1, "first run fetches once");
    // Refresh overrides the cache and refetches even though the pin is unchanged.
    run(true);
    assert_eq!(server.hits(), 2, "refresh must refetch the cached pin");
}

// ---- --max-parallel concurrency (rendezvous barrier) ----------------------

/// A two-agent project: each rule is its own oneharness run, so `--max-parallel`
/// controls whether the two runs overlap.
fn two_run_project() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\n\
         files:\n  include: [\"src/**\"]\n\
         agents:\n  a: {}\n  b: {}\n\
         rules:\n  \
           - name: rule_a\n    description: \"true when ok; false otherwise.\"\n    agent: a\n  \
           - name: rule_b\n    description: \"true when ok; false otherwise.\"\n    agent: b\n",
    );
    p.write("src/lib.rs", "// code\n");
    p
}

#[test]
fn max_parallel_runs_judges_concurrently() {
    let p = two_run_project();
    let verdicts = p.write_verdicts(r#"{"rule_a": true, "rule_b": true}"#);
    let barrier = p.path().join("barrier");

    // The barrier releases only when both judges are present at once. With
    // `--max-parallel 2` both runs share a wave, so they rendezvous and pass.
    // (A generous timeout tolerates slow process spawns; the success path
    // returns the instant both arrive, so the test stays fast.)
    p.lint_v() // `-v` itemizes the passing rules so we can assert on them
        .arg("--max-parallel")
        .arg("2")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_BARRIER", &barrier)
        .env("LLMLINT_MOCK_BARRIER_N", "2")
        .env("LLMLINT_MOCK_BARRIER_MS", "30000")
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS rule_a"))
        .stdout(predicate::str::contains("PASS rule_b"));
}

#[test]
fn serial_wave_does_not_satisfy_the_concurrency_barrier() {
    let p = two_run_project();
    let verdicts = p.write_verdicts(r#"{"rule_a": true, "rule_b": true}"#);
    let barrier = p.path().join("barrier");

    // With `--max-parallel 1` the first run is alone in its wave; it never sees a
    // second peer, times out at the barrier, and is reported as a run error
    // (exit 2). This is the negative control proving the barrier really gates on
    // concurrency rather than passing trivially.
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_BARRIER", &barrier)
        .env("LLMLINT_MOCK_BARRIER_N", "2")
        .env("LLMLINT_MOCK_BARRIER_MS", "1500")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no structured output"));
}

// ---- rationales -----------------------------------------------------------

/// The ordered key list of a JSON object (relies on `serde_json`'s
/// `preserve_order`, which is unified on for this whole package).
fn key_order(obj: &Value) -> Vec<String> {
    obj.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn rationale_is_required_and_ordered_in_the_schema_by_default() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r_one, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"r_one": true}"#);
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .success();

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    let rule = &schema["properties"]["r_one"];
    // Strict field order: name -> rationale -> result, in both the property map
    // and the `required` list, so next-token prediction anchors each verdict.
    assert_eq!(
        key_order(&rule["properties"]),
        ["name", "rationale", "holds", "violations"]
    );
    assert_eq!(
        rule["required"],
        serde_json::json!(["name", "rationale", "holds"])
    );
    // The name is pinned to the exact rule so the judge can't mislabel it.
    assert_eq!(rule["properties"]["name"]["const"], "r_one");
}

#[test]
fn rationale_shows_for_failure_by_default_and_for_passes_at_verbose() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: passing_rule, description: \"{RULE}\" }}\n  \
             - {{ name: failing_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"passing_rule": {"holds": true, "rationale": "every import flows downward"},
            "failing_rule": {"holds": false, "rationale": "raw SQL built in lib.rs:1",
                "violations": [{"file": "src/lib.rs", "line": 1, "message": "inline SQL"}]}}"#,
    );

    // Default: the failure's rationale is shown; the passing rule (and its
    // rationale) is not itemized.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL failing_rule"))
        .stdout(predicate::str::contains(
            "rationale: raw SQL built in lib.rs:1",
        ))
        .stdout(predicate::str::contains("every import flows downward").not());

    // `-v`: every evaluated rule shows its rationale, passing ones included.
    p.lint_v()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("PASS passing_rule"))
        .stdout(predicate::str::contains(
            "rationale: every import flows downward",
        ))
        .stdout(predicate::str::contains(
            "rationale: raw SQL built in lib.rs:1",
        ));
}

#[test]
fn no_rationales_flag_drops_rationale_from_schema_and_report() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: bare_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Even if the harness returns a rationale anyway, `--no-rationales` must
    // suppress it: llmlint is authoritative about whether one is shown.
    let verdicts = p.write_verdicts(
        r#"{"bare_rule": {"holds": false, "rationale": "leaked rationale",
            "violations": [{"file": "src/lib.rs", "line": 1, "message": "nope"}]}}"#,
    );
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .arg("--no-rationales")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL bare_rule"))
        .stdout(predicate::str::contains("rationale:").not())
        .stdout(predicate::str::contains("leaked rationale").not());

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    let rule = &schema["properties"]["bare_rule"];
    assert_eq!(
        key_order(&rule["properties"]),
        ["name", "holds", "violations"]
    );
    assert_eq!(rule["required"], serde_json::json!(["name", "holds"]));
}

#[test]
fn cli_rationales_flag_overrides_config_false() {
    let p = Project::new();
    // Config disables rationales; the CLI flag turns them back on (CLI wins).
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrationales: false\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r_one, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"r_one": true}"#);
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .arg("--rationales")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .success();

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    assert_eq!(
        schema["properties"]["r_one"]["required"],
        serde_json::json!(["name", "rationale", "holds"])
    );
}

#[test]
fn per_rule_rationale_overrides_the_session_default() {
    let p = Project::new();
    // Session default off, but one rule opts back in. Both share a file set, so
    // they land in one judge call (one schema) where they must differ per rule.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrationales: false\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: wants_one, description: \"{RULE}\", rationale: true }}\n  \
             - {{ name: wants_none, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"wants_one": true, "wants_none": true}"#);
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .success();

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    assert_eq!(
        schema["properties"]["wants_one"]["required"],
        serde_json::json!(["name", "rationale", "holds"])
    );
    assert_eq!(
        schema["properties"]["wants_none"]["required"],
        serde_json::json!(["name", "holds"])
    );
}

// ---- CLI overrides of top-level settings ---------------------------------

#[test]
fn cli_model_overrides_config_model() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  model: config-model\n\
             rules:\n  - {{ name: m_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"m_rule": true}"#);
    let args_dump = p.path().join("args.txt");

    p.lint()
        .arg("--model")
        .arg("cli-model")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();

    let args = fs::read_to_string(&args_dump).unwrap();
    let model = args
        .lines()
        .skip_while(|l| *l != "--model")
        .nth(1)
        .expect("--model should be forwarded");
    assert_eq!(model, "cli-model");
}

#[test]
fn cli_schema_max_retries_overrides_config() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\noneharness:\n  schema_max_retries: 2\n\
             rules:\n  - {{ name: retry_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"retry_rule": true}"#);
    let args_dump = p.path().join("args.txt");

    p.lint()
        .arg("--schema-max-retries")
        .arg("9")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();

    let args = fs::read_to_string(&args_dump).unwrap();
    let retries = args
        .lines()
        .skip_while(|l| *l != "--schema-max-retries")
        .nth(1)
        .expect("--schema-max-retries should be forwarded");
    assert_eq!(retries, "9");
}

#[test]
fn cli_prompt_template_file_overrides_config_template() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\n\
         prompt_template: |\n  \
           CONFIG_TEMPLATE_MARKER\n  \
           {% for r in rules %}rule={{ r.name }}\n  \
           {% endfor %}\n\
         files:\n  include: [\"src/**\"]\n\
         rules:\n  \
           - name: templated_rule\n    \
             description: \"true when ok; false otherwise.\"\n",
    );
    p.write(
        "cli-template.md",
        "CLI_TEMPLATE_MARKER\n{% for r in rules %}rule={{ r.name }}\n{% endfor %}",
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"templated_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--prompt-template")
        .arg(p.path().join("cli-template.md"))
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("CLI_TEMPLATE_MARKER"), "system:\n{system}");
    assert!(
        !system.contains("CONFIG_TEMPLATE_MARKER"),
        "system:\n{system}"
    );
    assert!(system.contains("rule=templated_rule"), "system:\n{system}");
}

#[test]
fn plugin_top_level_scalars_resolve_nearest_root_wins() {
    let p = Project::new();
    // root -> mid -> leaf. The nearest config to set a scalar wins; a deeper
    // plugin only fills what shallower configs left unset.
    p.write(
        "leaf.yml",
        "rationales: true\noneharness:\n  model: leaf-model\n  timeout: 7\n\
         prompt_template: leaf-tmpl\nrules: []\n",
    );
    p.write(
        "mid.yml",
        "plugins:\n  - ./leaf.yml\noneharness:\n  model: mid-model\nrules: []\n",
    );
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nplugins:\n  - ./mid.yml\nrationales: false\n\
             files:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: root_rule, description: \"{RULE}\" }}\n"
        ),
    );

    let out = p.bare().arg("config").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let cfg = &v["config"];
    // root set rationales -> root wins over both plugins.
    assert_eq!(cfg["rationales"], false);
    // root left model unset; mid is nearer than leaf -> mid wins.
    assert_eq!(cfg["oneharness"]["model"], "mid-model");
    // only leaf set these -> they fill through.
    assert_eq!(cfg["oneharness"]["timeout"], 7);
    assert_eq!(cfg["prompt_template"], "leaf-tmpl");
}

#[test]
fn config_rationales_false_drops_rationale_without_a_cli_flag() {
    let p = Project::new();
    // Rationales disabled in the config, no CLI flag at all.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrationales: false\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: bare_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // The mock leaks a rationale; llmlint must still drop it.
    let verdicts = p.write_verdicts(
        r#"{"bare_rule": {"holds": false, "rationale": "leaked rationale",
            "violations": [{"file": "src/lib.rs", "line": 1, "message": "nope"}]}}"#,
    );
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL bare_rule"))
        .stdout(predicate::str::contains("rationale:").not())
        .stdout(predicate::str::contains("leaked rationale").not());

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    assert_eq!(
        schema["properties"]["bare_rule"]["required"],
        serde_json::json!(["name", "holds"])
    );
}

#[test]
fn per_rule_rationale_false_suppresses_only_that_rule_when_session_is_on() {
    let p = Project::new();
    // Session default is on (unset); one rule opts out. Both fail, so both are
    // shown at the default level — the opted-out rule must omit its rationale.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: kept_in, description: \"{RULE}\" }}\n  \
             - {{ name: opted_out, description: \"{RULE}\", rationale: false }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"kept_in": {"holds": false, "rationale": "kept reason",
                "violations": [{"file": "src/lib.rs", "line": 1, "message": "k"}]},
            "opted_out": {"holds": false, "rationale": "dropped reason",
                "violations": [{"file": "src/lib.rs", "line": 2, "message": "o"}]}}"#,
    );
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("rationale: kept reason"))
        .stdout(predicate::str::contains("dropped reason").not());

    // The schema agrees per rule: kept_in carries `rationale`, opted_out doesn't.
    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    assert_eq!(
        schema["properties"]["kept_in"]["required"],
        serde_json::json!(["name", "rationale", "holds"])
    );
    assert_eq!(
        schema["properties"]["opted_out"]["required"],
        serde_json::json!(["name", "holds"])
    );
}

#[test]
fn json_output_carries_rationale_for_every_rule() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: pass_rule, description: \"{RULE}\" }}\n  \
             - {{ name: fail_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"pass_rule": {"holds": true, "rationale": "all good"},
            "fail_rule": {"holds": false, "rationale": "broke at lib.rs:1",
                "violations": [{"file": "src/lib.rs", "line": 1, "message": "x"}]}}"#,
    );

    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let by_name = |name: &str| {
        v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    // Both the pass and the fail carry their rationale in the machine contract.
    assert_eq!(by_name("pass_rule")["rationale"], "all good");
    assert_eq!(by_name("fail_rule")["rationale"], "broke at lib.rs:1");
    // And `name` leads each rule object, mirroring the schema's field order.
    assert_eq!(
        key_order(&by_name("pass_rule"))[0..2],
        ["name", "rationale"]
    );
}

#[test]
fn rationale_guidance_reaches_the_prompt_only_when_enabled() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r_one, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"r_one": true}"#);
    let dump = p.path().join("system.txt");

    // Default (rationales on): the terse-rationale guidance is rendered in.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let on = fs::read_to_string(&dump).unwrap();
    assert!(on.contains("## Rationale"), "system:\n{on}");
    // The terseness guidance is present (the wrapped text is "terse and\npithy").
    assert!(on.contains("terse"), "system:\n{on}");

    // `--no-rationales`: the guidance block is gone.
    p.lint()
        .arg("--no-rationales")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let off = fs::read_to_string(&dump).unwrap();
    assert!(!off.contains("## Rationale"), "system:\n{off}");
}

// ---- relevance ------------------------------------------------------------

#[test]
fn conditional_relevance_gates_the_schema_with_an_if_then() {
    let p = Project::new();
    // One always-evaluated rule and one gated on a relevance condition, sharing a
    // file set so both land in one schema where they must differ per rule.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: always_rule, description: \"{RULE}\" }}\n  \
             - {{ name: scoped_rule, description: \"{RULE}\", \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"always_rule": true, "scoped_rule": {"relevant": true, "holds": true}}"#,
    );
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .success();

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    // The gated rule inserts `relevant` before the verdict and requires `holds`
    // only via an if/then on `relevant == true`.
    let scoped = &schema["properties"]["scoped_rule"];
    assert_eq!(
        key_order(&scoped["properties"]),
        ["name", "rationale", "relevant", "holds", "violations"]
    );
    assert_eq!(
        scoped["required"],
        serde_json::json!(["name", "rationale", "relevant"])
    );
    assert_eq!(scoped["if"]["properties"]["relevant"]["const"], true);
    assert_eq!(scoped["then"]["required"], serde_json::json!(["holds"]));
    // The always-evaluated rule has no relevance gate at all.
    let always = &schema["properties"]["always_rule"];
    assert_eq!(
        always["required"],
        serde_json::json!(["name", "rationale", "holds"])
    );
    assert!(always.get("if").is_none());
    assert!(always["properties"].get("relevant").is_none());
}

#[test]
fn judge_ruling_not_relevant_is_reported_distinctly_and_exits_clean() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: sql_rule, description: \"{RULE}\", \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // The judge rules it not applicable: no holds, just a rationale.
    let verdicts =
        p.write_verdicts(r#"{"sql_rule": {"relevant": false, "rationale": "change adds no SQL"}}"#);

    // Not relevant is not a failure -> exit 0, and it gets its own summary
    // segment so it isn't conflated with a pass.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("1 not relevant"))
        .stdout(predicate::str::contains("sql_rule").not());

    // `-v` itemizes it as N/A with the judge's reason, distinct from a PASS.
    p.lint_v()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("N/A sql_rule (not relevant)"))
        .stdout(predicate::str::contains("rationale: change adds no SQL"))
        .stdout(predicate::str::contains("PASS sql_rule").not());

    // The machine contract carries the not_relevant outcome + summary count.
    p.lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"not_relevant\": 1"))
        .stdout(predicate::str::contains("\"outcome\": \"not_relevant\""));
}

#[test]
fn relevant_rule_still_evaluates_its_verdict() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: sql_rule, description: \"{RULE}\", \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Relevant and violated -> a normal failure with its violation.
    let verdicts = p.write_verdicts(
        r#"{"sql_rule": {"relevant": true, "holds": false,
            "violations": [{"file": "src/lib.rs", "line": 1, "message": "inline SQL"}]}}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL sql_rule"))
        .stdout(predicate::str::contains("src/lib.rs:1: inline SQL"));
}

#[test]
fn relevance_false_skips_the_judge_entirely() {
    let p = Project::new();
    // `relevance: false` disables the rule deterministically — no judge call.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: off_rule, description: \"{RULE}\", relevance: false }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let runlog = p.path().join("runlog");

    p.lint_v()
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success()
        .stdout(predicate::str::contains("N/A off_rule (not relevant)"))
        .stdout(predicate::str::contains("1 not relevant"));
    // No oneharness invocation happened at all for the never-relevant rule.
    assert!(!runlog.exists() || fs::read_dir(&runlog).unwrap().count() == 0);
}

#[test]
fn multi_judge_relevance_majority_not_relevant_skips_the_verdict() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: scoped_rule, description: \"{RULE}\", judges: 3, \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Three sequential judges: two rule it not relevant, one finds it relevant
    // and violated. Relevance is decided first -> majority says not relevant, so
    // the lone violation never fails the build.
    let verdicts = p.write_verdicts(
        r#"{"scoped_rule": [
            {"relevant": false, "rationale": "no SQL in this change"},
            {"relevant": false, "rationale": "still no SQL"},
            {"relevant": true, "holds": false, "rationale": "raw SQL at lib.rs",
             "violations": [{"message": "raw SQL"}]}
        ]}"#,
    );
    let state = p.path().join("state");

    p.lint_v()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .success()
        .stdout(predicate::str::contains("N/A scoped_rule (not relevant)"))
        // The per-judge breakdown shows each judge's relevance, dissent included.
        .stdout(predicate::str::contains(
            "judge 1 not relevant: no SQL in this change",
        ))
        .stdout(predicate::str::contains(
            "judge 3 violated: raw SQL at lib.rs",
        ))
        .stdout(predicate::str::contains("1 not relevant"));
}

#[test]
fn multi_judge_relevance_majority_relevant_then_tallies_the_verdict() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: scoped_rule, description: \"{RULE}\", judges: 3, \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // Two judges find it relevant and holding, one abstains as not relevant. The
    // rule is relevant (2/3), and the verdict is tallied over the relevant judges
    // only (2/2 held) -> pass; the abstainer doesn't vote on the verdict.
    let verdicts = p.write_verdicts(
        r#"{"scoped_rule": [
            {"relevant": true, "holds": true, "rationale": "parameterized"},
            {"relevant": true, "holds": true, "rationale": "uses the query builder"},
            {"relevant": false, "rationale": "this hunk has no SQL"}
        ]}"#,
    );
    let state = p.path().join("state");

    p.lint_v()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .success()
        // Held fraction is over the relevant judges (2/2), not all three.
        .stdout(predicate::str::contains(
            "PASS scoped_rule (2/2 judges held)",
        ))
        .stdout(predicate::str::contains(
            "judge 3 not relevant: this hunk has no SQL",
        ));
}

#[test]
fn empty_relevance_condition_is_rejected() {
    // A relevance condition that is a blank string is a deterministic config
    // error (use `true`/`false` for always/never), surfaced as exit 2.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: blank_rule, description: \"{RULE}\", relevance: \"   \" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("empty relevance condition"));
}

#[test]
fn relevance_guidance_and_condition_reach_the_prompt_only_when_conditional() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: scoped_rule, description: \"{RULE}\", \
             relevance: the change touches SQL }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"scoped_rule": {"relevant": false, "rationale": "n/a"}}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("## Relevance"), "system:\n{system}");
    assert!(
        system.contains("Relevant only when: the change touches SQL"),
        "system:\n{system}"
    );

    // A config with no conditional rules renders no relevance guidance.
    let p2 = Project::new();
    p2.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: plain_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p2.write("src/lib.rs", "// code\n");
    let v2 = p2.write_verdicts(r#"{"plain_rule": true}"#);
    let dump2 = p2.path().join("system.txt");
    p2.lint()
        .env("LLMLINT_MOCK_VERDICTS", &v2)
        .env("LLMLINT_MOCK_DUMP", &dump2)
        .assert()
        .success();
    let off = fs::read_to_string(&dump2).unwrap();
    assert!(!off.contains("## Relevance"), "system:\n{off}");
}
