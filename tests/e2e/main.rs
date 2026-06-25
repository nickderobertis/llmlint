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

const RULE: &str = "TRUE when ok; FALSE otherwise.";

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
        .success()
        .stdout(predicate::str::contains("0 rules"));
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
