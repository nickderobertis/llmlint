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
    /// The per-project history directory results logging writes to. Every command
    /// built here points `LLMLINT_HISTORY_DIR` at it (via [`Project::bare`]), so
    /// runs never touch the real user data dir and each project's history is
    /// isolated and cleaned with the tempdir.
    fn history_dir(&self) -> PathBuf {
        self.path().join(".llmlint-history")
    }
    /// A bare llmlint command (cwd = project), no oneharness wiring — for
    /// subcommands like `init`/`config` that take no `--oneharness-bin`.
    fn bare(&self) -> Command {
        let mut c = Command::cargo_bin("llmlint").unwrap();
        c.current_dir(self.path());
        // Isolate results logging to the project so tests never write to (or read
        // from) the real platform data dir. A `history` subcommand built here
        // reads the same isolated store a preceding `lint` wrote.
        c.env("LLMLINT_HISTORY_DIR", self.history_dir());
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
    /// `check-ignores`: the deterministic ignore-directive validation, wired with
    /// no `--oneharness-bin` (it never spawns a harness) so a green run proves the
    /// check is model-free.
    fn check_ignores(&self) -> Command {
        let mut c = self.bare();
        c.arg("check-ignores");
        c
    }
    /// `lint-config`: the `lint` engine with the bundled config-lint plugin forced
    /// on (no project config needed), wired to the mock harness.
    fn lint_config(&self) -> Command {
        let mut c = self.bare();
        c.arg("lint-config");
        c.arg("--oneharness-bin").arg(mock_path());
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

// ---- live progress view (audience detection) ------------------------------

/// A project with one passing and one failing rule, for the progress journeys.
fn progress_project() -> (Project, PathBuf) {
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
    (p, verdicts)
}

/// The captured stream carries no terminal control codes: no ESC (`\x1b`, the
/// lead byte of every cursor-move/erase/color sequence) and no bare carriage
/// return (`\r`, the in-place-rewrite mechanism). This is the "don't corrupt
/// captured output / don't blow up an agent's context" guarantee.
fn assert_no_control_bytes(stream: &[u8], label: &str) {
    assert!(
        !stream.contains(&0x1b),
        "{label} leaked an ESC control byte: {:?}",
        String::from_utf8_lossy(stream)
    );
    assert!(
        !stream.contains(&b'\r'),
        "{label} leaked a carriage return: {:?}",
        String::from_utf8_lossy(stream)
    );
}

#[test]
fn progress_view_never_leaks_into_captured_output() {
    // `assert_cmd` captures through pipes (not a TTY), so the live view must be
    // fully suppressed: the report lands on stdout and stderr carries no progress
    // control codes — under the default `auto`, an explicit `--progress always`
    // (which still refuses to animate a non-terminal), and `--progress never`.
    let (p, verdicts) = progress_project();
    for extra in [&[][..], &["--progress", "always"], &["--progress", "never"]] {
        let out = p
            .lint()
            .args(extra)
            .env("LLMLINT_MOCK_VERDICTS", &verdicts)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "args {extra:?}");
        // The report is on stdout, unchanged by the progress plumbing.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("FAIL failing_rule"), "args {extra:?}");
        assert!(
            stdout.contains("2 rules: 1 passed, 1 failed, 0 skipped"),
            "args {extra:?}"
        );
        // Neither stream carries the animation's control bytes.
        assert_no_control_bytes(&out.stdout, &format!("stdout {extra:?}"));
        assert_no_control_bytes(&out.stderr, &format!("stderr {extra:?}"));
    }
}

#[test]
fn agent_env_forces_plain_output_with_no_progress_leak() {
    // Inside an AI coding agent (detected via `CLAUDECODE`), even `--progress
    // always` + `--color auto` must stay plain: no animation escapes, and the
    // report carries no ANSI — captured ANSI is unreliable in an agent, so the
    // safe plain path is taken regardless of the (piped) TTY state.
    let (p, verdicts) = progress_project();
    let out = p
        .lint()
        .arg("--progress")
        .arg("always")
        .env("CLAUDECODE", "1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL failing_rule"));
    assert_no_control_bytes(&out.stdout, "stdout (agent)");
    assert_no_control_bytes(&out.stderr, "stderr (agent)");
}

#[test]
fn progress_json_format_is_untouched() {
    // `--format json` is the machine channel: the live view is never drawn for it,
    // and stdout stays pure JSON regardless of `--progress`.
    let (p, verdicts) = progress_project();
    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .arg("--progress")
        .arg("always")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // Parses as JSON (no progress prefix/suffix corrupts it) and stderr is clean.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["failed"], 1);
    assert_no_control_bytes(&out.stderr, "stderr (json)");
}

#[test]
fn progress_invalid_value_is_rejected() {
    let (p, verdicts) = progress_project();
    p.lint()
        .arg("--progress")
        .arg("sometimes")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid value 'sometimes'"));
}

// Note: the live view only *animates* when stderr is a real terminal, which
// `assert_cmd`'s pipes can't provide. That interactive path is verified without a
// heavyweight PTY dependency: the renderer's frames + self-erase are asserted on a
// `vt100`-backed `InMemoryTerm` in `commands::progress` (including the `animate`
// steady-tick path). A real-OS PTY round-trip (incl. Windows ConPTY) is a deferred
// separate tier, like `win-color` — see `docs/design/interactive-progress.md`.

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
    // The config-lint rules require line attribution, so a violation cites the
    // config file + the line of the offending rule.
    let verdicts = p.write_verdicts(
        r#"{"name_describes_what_the_rule_checks":
              {"holds": false, "violations": [{"file": "llmlint.yml", "line": 3, "message": "rule named 'foo'"}]},
            "description_yields_clear_verdict": true,
            "relevance_scopes_conditional_rules": true}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL name_describes_what_the_rule_checks",
        ))
        .stdout(predicate::str::contains("rule named 'foo'"));
}

#[test]
fn config_lint_plugin_flags_relevance_where_files_globs_belong() {
    // The `path_scoped_rules_use_files_not_relevance` check: a rule scoped to a
    // file type/location via `relevance` (where `files` globs belong) is flagged.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nplugins:\n  - {CONFIG_LINT}\n"),
    );
    let verdicts = p.write_verdicts(
        r#"{"path_scoped_rules_use_files_not_relevance":
              {"holds": false, "violations": [{"file": "llmlint.yml", "line": 3, "message": "relevance restates a path scope"}]}}"#,
    );

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL path_scoped_rules_use_files_not_relevance",
        ))
        .stdout(predicate::str::contains("relevance restates a path scope"));
}

#[test]
fn lint_config_lints_a_config_without_the_plugin_declared() {
    // `lint-config` includes the bundled config-lint rules by default, so it
    // catches a bad rule in a config that never declared the plugin itself.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: foo, description: \"{RULE}\" }}\n"),
    );
    let verdicts = p.write_verdicts(
        r#"{"name_describes_what_the_rule_checks":
              {"holds": false, "violations": [{"file": "llmlint.yml", "line": 2, "message": "rule named 'foo' is a placeholder"}]}}"#,
    );

    p.lint_config()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "FAIL name_describes_what_the_rule_checks",
        ))
        .stdout(predicate::str::contains(
            "rule named 'foo' is a placeholder",
        ));
}

#[test]
fn lint_config_passes_a_clean_config() {
    // Well-named, clearly-described rules: every config-lint check holds -> exit 0.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: public_items_are_documented, description: \"{RULE}\" }}\n"
        ),
    );
    // No verdicts file: the mock defaults every config-lint rule to holds=true.
    p.lint_config()
        .assert()
        .success()
        .stdout(predicate::str::contains("passed"));
}

#[test]
fn lint_config_runs_the_comment_check_before_judging() {
    // Phase 1 is the deterministic ignore-directive (comment) check: a malformed
    // directive in a config file is a hard exit-2 error before any judge call, even
    // though the mock harness is wired in.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: a_rule, description: \"{RULE}\" }}\n  \
             # llmlint: ignore[name_describes_what_the_rule_checks]\n"
        ),
    );
    // Even with verdicts available, the run never reaches the model: the comment
    // check fails first (the directive names a rule but gives no reason).
    p.lint_config().assert().code(2).stderr(
        predicate::str::contains("llmlint.yml").and(predicate::str::contains("give a reason")),
    );
}

#[test]
fn lint_config_with_no_config_files_is_a_clean_skip() {
    // Nothing matches the config-lint globs -> every rule is skipped, not failed.
    let p = Project::new();
    p.write("src/lib.rs", "// not a config\n");
    p.lint_config()
        .assert()
        .success()
        .stdout(predicate::str::contains("skipped"));
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
fn no_files_block_lints_every_file_in_the_tree() {
    // A config with no `files` block is the repo-wide "lint everything under cwd"
    // default: every file in the tree is a target, not zero. Files sit at the root
    // and in nested directories to prove the whole subtree is walked.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: whole_tree, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("README.md", "# readme\n");
    p.write("src/a.rs", "// a\n");
    p.write("docs/guide.md", "# guide\n");
    let verdicts = p.write_verdicts(r#"{"whole_tree": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // Every file across the tree is a target (paths render with forward slashes).
    for target in ["README.md", "src/a.rs", "docs/guide.md"] {
        assert!(system.contains(target), "missing {target}:\n{system}");
    }
}

#[test]
fn no_files_block_still_respects_gitignore_and_exclude() {
    // The whole-tree default is narrowed by both `exclude` and the gitignore-aware
    // walk, so it never reintroduces vendored/build/ignored files. Run in a real
    // git repo, since gitignore only applies inside one.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  exclude: [\"vendor/**\"]\nrules:\n  \
             - {{ name: whole_tree, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/keep.rs", "// keep\n");
    p.write("vendor/skip.rs", "// excluded by config\n");
    p.write("build/ignored.rs", "// gitignored\n");
    p.write(".gitignore", "build/\n");
    init_repo(p.path());

    let verdicts = p.write_verdicts(r#"{"whole_tree": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("src/keep.rs"), "system:\n{system}");
    assert!(
        !system.contains("vendor/skip.rs"),
        "config `exclude` must still narrow the whole-tree default:\n{system}"
    );
    assert!(
        !system.contains("build/ignored.rs"),
        "gitignored file must not leak into the whole-tree default:\n{system}"
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

// ---- --diff (changed-line context in the prompt) --------------------------

/// Run `git` in `dir`, asserting success. `std::process::Command` is spelled out
/// because `assert_cmd::Command` is imported as `Command` in this file.
fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Like `git`, but returns trimmed stdout — for capturing a commit SHA to use
/// as an explicit `--diff-base`.
fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `git init` + identity + a `main` branch, so commits don't depend on the
/// host's git defaults.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["checkout", "-q", "-b", "main"]);
}

/// A project rooted at a git repo: write a config (with `files.include: src/**`
/// and the given `rules` YAML block) plus the `initial` files, then commit them
/// as the baseline so a test can edit afterward and diff against `HEAD`.
fn committed_repo(rules: &str, initial: &[(&str, &str)]) -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n{rules}"),
    );
    for (path, body) in initial {
        p.write(path, body);
    }
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    p
}

#[test]
fn diff_flag_adds_changed_lines_to_the_prompt() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: changed_rule, description: \"{RULE}\" }}\n"
        ),
    );
    // Two files committed as the baseline; only one is changed afterward.
    p.write("src/a.rs", "fn a() {}\n");
    p.write("src/b.rs", "fn b() {}\n");
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    p.write("src/a.rs", "fn a() { let x = 1; }\n");

    let verdicts = p.write_verdicts(r#"{"changed_rule": true}"#);
    let dump = p.path().join("system.txt");

    // Bare `--diff` defaults to the git backend.
    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // The changed file's diff is inlined under its target line, carrying the added
    // line as a `+` diff line — exactly which lines to review.
    assert!(system.contains("```diff"), "system:\n{system}");
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { let x = 1; }"),
        "system:\n{system}"
    );
    // The unchanged file gets no diff (nothing changed in it).
    assert!(
        !system.contains("diff --git a/src/b.rs"),
        "system:\n{system}"
    );
    // Both files are still listed as targets (diffs are additive context, not a
    // file filter).
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
    assert!(system.contains("- src/b.rs"), "system:\n{system}");
}

#[test]
fn without_diff_flag_no_changed_lines_section() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "fn a() {}\n");
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    p.write("src/a.rs", "fn a() { let x = 1; }\n");

    let verdicts = p.write_verdicts(r#"{"r": true}"#);
    let dump = p.path().join("system.txt");

    // No `--diff`: the prompt is unchanged — no diff section even in a git repo
    // with pending changes.
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(!system.contains("```diff"), "system:\n{system}");
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
}

#[test]
fn diff_outside_a_git_repo_is_a_clear_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "fn a() {}\n");
    let verdicts = p.write_verdicts(r#"{"r": true}"#);

    // No `git init`: `--diff git` can't produce diffs, so it fails up front
    // (exit 2) rather than silently reviewing nothing.
    p.lint()
        .arg("--diff")
        .arg("git")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("diff (git)"));
}

#[test]
fn diff_explicit_git_backend_adds_changed_lines() {
    // The explicit `--diff git` form behaves like bare `--diff` (git is default).
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    p.write("src/a.rs", "fn a() { explicit(); }\n");
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("git")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("```diff"), "system:\n{system}");
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { explicit(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_renders_additions_and_deletions_across_files() {
    // One file gains a line, another loses one: both get a block, with the `+`
    // and `-` lines a reviewer needs.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(
        &rules,
        &[
            ("src/a.rs", "fn a() {}\n"),
            ("src/b.rs", "fn keep() {}\nfn remove_me() {}\n"),
        ],
    );
    p.write("src/a.rs", "fn a() { added(); }\n"); // addition
    p.write("src/b.rs", "fn keep() {}\n"); // remove_me deleted
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("diff --git a/src/b.rs"),
        "system:\n{system}"
    );
    assert!(system.contains("+fn a() { added(); }"), "system:\n{system}");
    assert!(system.contains("-fn remove_me() {}"), "system:\n{system}");
}

#[test]
fn diff_includes_both_staged_and_unstaged_changes() {
    // `git diff HEAD` is the base, so staged *and* unstaged edits both show.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(
        &rules,
        &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
    );
    p.write("src/a.rs", "fn a() { staged(); }\n");
    git(p.path(), &["add", "src/a.rs"]); // a.rs: staged
    p.write("src/b.rs", "fn b() { unstaged(); }\n"); // b.rs: left unstaged
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("+fn a() { staged(); }"),
        "staged change missing:\n{system}"
    );
    assert!(
        system.contains("+fn b() { unstaged(); }"),
        "unstaged change missing:\n{system}"
    );
}

#[test]
fn diff_untracked_new_file_is_a_target_without_a_block() {
    // A brand-new untracked file has no diff vs HEAD, so it carries no block —
    // but it is still a target (reviewed whole, since diffs are additive context).
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    p.write("src/new.rs", "fn brand_new() {}\n"); // never committed
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // Listed as a target...
    assert!(system.contains("- src/new.rs"), "target missing:\n{system}");
    // ...but no diff block, and with nothing else changed, no section at all.
    assert!(
        !system.contains("diff --git a/src/new.rs"),
        "unexpected diff block:\n{system}"
    );
    assert!(!system.contains("```diff"), "system:\n{system}");
}

#[test]
fn diff_clean_worktree_renders_no_section() {
    // `--diff` on a pristine checkout: nothing changed, so no section — but the
    // files are still linted (whole-file review), and the run is clean.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(!system.contains("```diff"), "system:\n{system}");
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
}

#[test]
fn diff_unborn_head_uses_cached_fallback() {
    // A repo with no commit has an unborn HEAD; the git backend falls back to a
    // `--cached` diff so a staged new file still shows as added (no fatal).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/a.rs", "fn a() {}\n");
    init_repo(p.path()); // init only — no commit
    git(p.path(), &["add", "src/a.rs"]); // stage so --cached sees it
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(system.contains("+fn a() {}"), "system:\n{system}");
}

#[test]
fn diff_is_scoped_to_each_rules_files() {
    // Two rules, each scoped to its own file; both files change. Each rule's
    // judge prompt must carry only its own file's diff — never the other's.
    let rules = format!(
        "  - {{ name: rule_a, description: \"{RULE}\", files: {{ include: [\"src/a.rs\"] }} }}\n  \
         - {{ name: rule_b, description: \"{RULE}\", files: {{ include: [\"src/b.rs\"] }} }}\n"
    );
    let p = committed_repo(
        &rules,
        &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
    );
    p.write("src/a.rs", "fn a() { aaa(); }\n");
    p.write("src/b.rs", "fn b() { bbb(); }\n");

    // Isolate rule_a: its prompt has a.rs's diff and not b.rs's.
    let dump_a = p.path().join("a.txt");
    p.lint()
        .arg("--diff")
        .arg("--rule")
        .arg("rule_a")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump_a)
        .assert()
        .success();
    let sys_a = fs::read_to_string(&dump_a).unwrap();
    assert!(sys_a.contains("diff --git a/src/a.rs"), "system:\n{sys_a}");
    assert!(sys_a.contains("+fn a() { aaa(); }"), "system:\n{sys_a}");
    assert!(
        !sys_a.contains("diff --git a/src/b.rs"),
        "b leaked into a:\n{sys_a}"
    );

    // Isolate rule_b: the mirror image.
    let dump_b = p.path().join("b.txt");
    p.lint()
        .arg("--diff")
        .arg("--rule")
        .arg("rule_b")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump_b)
        .assert()
        .success();
    let sys_b = fs::read_to_string(&dump_b).unwrap();
    assert!(sys_b.contains("diff --git a/src/b.rs"), "system:\n{sys_b}");
    assert!(sys_b.contains("+fn b() { bbb(); }"), "system:\n{sys_b}");
    assert!(
        !sys_b.contains("diff --git a/src/a.rs"),
        "a leaked into b:\n{sys_b}"
    );
}

#[test]
fn diff_respects_cwd_as_the_git_root() {
    // The git work tree lives under `repo/`, reached via `--cwd`; the outer dir
    // is not a repo. Diffs must run in `--cwd`, not the process cwd.
    let p = Project::new();
    let repo = p.path().join("repo");
    p.write(
        "repo/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("repo/src/a.rs", "fn a() {}\n");
    init_repo(&repo);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "baseline"]);
    p.write("repo/src/a.rs", "fn a() { in_cwd(); }\n");
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--cwd")
        .arg(&repo)
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { in_cwd(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_invalid_backend_is_rejected() {
    // An unknown backend is a clap usage error (exit 2) listing valid values —
    // it never reaches a run.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    p.lint()
        .arg("--diff")
        .arg("svn")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid value 'svn'"));
}

#[test]
fn diff_base_reviews_changes_against_a_branch() {
    // The PR-review case: a baseline on `main`, then a committed change on a
    // feature branch. The worktree is clean vs HEAD, so the default `--diff`
    // would show nothing — but `--diff-base main` surfaces exactly what the
    // branch changed, which is what a reviewer wants the judge to focus on.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { feature(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("```diff"), "system:\n{system}");
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { feature(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_default_head_shows_no_committed_branch_change() {
    // The mirror of the test above: without `--diff-base`, the default `HEAD`
    // base sees the clean worktree and renders no section — proving the branch
    // change only surfaces because of the explicit base, not by accident.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { feature(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(!system.contains("```diff"), "system:\n{system}");
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
}

#[test]
fn diff_base_requires_the_diff_flag() {
    // `--diff-base` is meaningless without `--diff`; clap rejects it up front
    // (exit 2) rather than silently ignoring the base.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    p.lint()
        .arg("--diff-base")
        .arg("main")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--diff"));
}

#[test]
fn diff_base_unknown_ref_is_a_clear_error() {
    // An explicit base is trusted, not probed: a ref that doesn't resolve is a
    // clear exit-2 diff error, never a silent fallback to a different base.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    p.write("src/a.rs", "fn a() { changed(); }\n");
    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("no-such-ref")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("diff (git)"));
}

#[test]
fn diff_base_with_explicit_git_backend() {
    // `--diff git --diff-base main` (explicit backend + base together) behaves
    // like the bare `--diff --diff-base main` form.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { feature(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("git")
        .arg("--diff-base")
        .arg("main")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { feature(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_accepts_a_commit_sha() {
    // A raw commit SHA is a valid base, not just a branch name: diff the
    // worktree against the baseline commit's hash.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    let base = git_out(p.path(), &["rev-parse", "HEAD"]);
    p.write("src/a.rs", "fn a() { by_sha(); }\n");
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg(&base)
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { by_sha(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_accepts_a_tag() {
    // A tag resolves as a base just like a branch or SHA.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["tag", "v0"]);
    p.write("src/a.rs", "fn a() { by_tag(); }\n");
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("v0")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { by_tag(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_plain_ref_includes_uncommitted_worktree() {
    // `git diff <base>` (a plain ref) compares the *working tree* to the base,
    // so a committed branch change AND an uncommitted edit on top both show —
    // exactly what a reviewer wants when iterating before pushing.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn committed() {}\n");
    git(p.path(), &["commit", "-q", "-am", "committed change"]);
    p.write("src/a.rs", "fn committed() {}\nfn uncommitted() {}\n"); // left unstaged
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("+fn committed() {}"),
        "committed change missing:\n{system}"
    );
    assert!(
        system.contains("+fn uncommitted() {}"),
        "uncommitted change missing:\n{system}"
    );
}

#[test]
fn diff_base_two_dot_range_is_commit_to_commit() {
    // `git diff <base>..HEAD` (a two-dot range) compares two commits, so it
    // ignores the working tree: the committed change shows, the uncommitted one
    // does not. This is the contrast to the plain-ref case above.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn committed() {}\n");
    git(p.path(), &["commit", "-q", "-am", "committed change"]);
    p.write("src/a.rs", "fn committed() {}\nfn uncommitted() {}\n"); // not in any commit
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main..HEAD")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("+fn committed() {}"),
        "committed change missing:\n{system}"
    );
    assert!(
        !system.contains("uncommitted"),
        "uncommitted change leaked into a commit-range diff:\n{system}"
    );
}

#[test]
fn diff_base_three_dot_range_uses_merge_base() {
    // `git diff <base>...HEAD` (a three-dot range) diffs from the *merge base*,
    // so it shows only what this branch changed — not commits the base branch
    // made independently. Here `main` advances `b.rs` after `feature` forks; a
    // three-dot diff must show `feature`'s `a.rs` change and never `main`'s
    // `b.rs` change. This is the correct "what does this PR change" semantics.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(
        &rules,
        &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
    );
    // feature forks off the baseline and changes a.rs.
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { feat(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature changes a"]);
    // main independently advances b.rs.
    git(p.path(), &["checkout", "-q", "main"]);
    p.write("src/b.rs", "fn b() { main_moved(); }\n");
    git(p.path(), &["commit", "-q", "-am", "main changes b"]);
    git(p.path(), &["checkout", "-q", "feature"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main...HEAD")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // feature's own change is present...
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(system.contains("+fn a() { feat(); }"), "system:\n{system}");
    // ...but main's independent b.rs change is not part of this branch's diff.
    assert!(
        !system.contains("diff --git a/src/b.rs"),
        "main's change leaked into the three-dot diff:\n{system}"
    );
    assert!(!system.contains("main_moved"), "system:\n{system}");
}

#[test]
fn diff_base_renders_additions_and_deletions_across_files() {
    // Against a branch base, one file gains a line and another loses one across
    // the branch's commits; both get a block with the `+`/`-` lines.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(
        &rules,
        &[
            ("src/a.rs", "fn a() {}\n"),
            ("src/b.rs", "fn keep() {}\nfn remove_me() {}\n"),
        ],
    );
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { added(); }\n"); // addition
    p.write("src/b.rs", "fn keep() {}\n"); // remove_me deleted
    git(p.path(), &["commit", "-q", "-am", "feature edits"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("diff --git a/src/b.rs"),
        "system:\n{system}"
    );
    assert!(system.contains("+fn a() { added(); }"), "system:\n{system}");
    assert!(system.contains("-fn remove_me() {}"), "system:\n{system}");
}

#[test]
fn diff_base_is_scoped_to_each_rules_files() {
    // Diff scoping still holds with an explicit base: each rule's judge prompt
    // carries only its own file's diff vs the base, never a sibling's.
    let rules = format!(
        "  - {{ name: rule_a, description: \"{RULE}\", files: {{ include: [\"src/a.rs\"] }} }}\n  \
         - {{ name: rule_b, description: \"{RULE}\", files: {{ include: [\"src/b.rs\"] }} }}\n"
    );
    let p = committed_repo(
        &rules,
        &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
    );
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { aaa(); }\n");
    p.write("src/b.rs", "fn b() { bbb(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature edits"]);

    let dump_a = p.path().join("a.txt");
    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--rule")
        .arg("rule_a")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump_a)
        .assert()
        .success();
    let sys_a = fs::read_to_string(&dump_a).unwrap();
    assert!(sys_a.contains("diff --git a/src/a.rs"), "system:\n{sys_a}");
    assert!(sys_a.contains("+fn a() { aaa(); }"), "system:\n{sys_a}");
    assert!(
        !sys_a.contains("diff --git a/src/b.rs"),
        "b leaked into a:\n{sys_a}"
    );

    let dump_b = p.path().join("b.txt");
    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--rule")
        .arg("rule_b")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump_b)
        .assert()
        .success();
    let sys_b = fs::read_to_string(&dump_b).unwrap();
    assert!(sys_b.contains("diff --git a/src/b.rs"), "system:\n{sys_b}");
    assert!(sys_b.contains("+fn b() { bbb(); }"), "system:\n{sys_b}");
    assert!(
        !sys_b.contains("diff --git a/src/a.rs"),
        "a leaked into b:\n{sys_b}"
    );
}

#[test]
fn diff_base_respects_cwd_as_the_git_root() {
    // Base resolution runs in `--cwd`, not the process cwd: the work tree lives
    // under `repo/`, and `--diff-base main` resolves there.
    let p = Project::new();
    let repo = p.path().join("repo");
    p.write(
        "repo/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: r, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("repo/src/a.rs", "fn a() {}\n");
    init_repo(&repo);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "baseline"]);
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    p.write("repo/src/a.rs", "fn a() { in_cwd(); }\n");
    git(&repo, &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main")
        .arg("--cwd")
        .arg(&repo)
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { in_cwd(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_equal_to_tip_renders_no_section() {
    // An explicit base that equals the current tip (here the branch you're on)
    // means nothing differs: no ````diff` section, but the files are
    // still linted (whole-file review) and the run is clean.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo(&rules, &[("src/a.rs", "fn a() {}\n")]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("main") // we are on main, work tree clean -> no diff vs main
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(!system.contains("```diff"), "system:\n{system}");
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
}

/// Like `committed_repo`, but the config also sets a top-level `diff_base`, so a
/// repo can make `--diff` compare against a chosen branch without the flag.
fn committed_repo_with_diff_base(base: &str, rules: &str, initial: &[(&str, &str)]) -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\ndiff_base: {base}\nfiles:\n  include: [\"src/**\"]\nrules:\n{rules}"),
    );
    for (path, body) in initial {
        p.write(path, body);
    }
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    p
}

#[test]
fn diff_base_from_config_sets_the_default_base() {
    // A repo bakes `diff_base: main` into its config, so bare `--diff` (no
    // `--diff-base` flag) reviews what the current branch changed versus `main` —
    // the quality-gate default — even though the worktree is clean vs HEAD.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo_with_diff_base("main", &rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { from_config(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("```diff"), "system:\n{system}");
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { from_config(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_flag_overrides_config() {
    // The `--diff-base` flag wins over the config's `diff_base`: config says
    // `main`, but `--diff-base other` diffs against the `other` branch instead.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo_with_diff_base("main", &rules, &[("src/a.rs", "fn a() {}\n")]);
    // `other` branch holds a different baseline for a.rs.
    git(p.path(), &["checkout", "-q", "-b", "other"]);
    p.write("src/a.rs", "fn a() { on_other(); }\n");
    git(p.path(), &["commit", "-q", "-am", "other baseline"]);
    // Back on a feature branch off `other`, make the change under review.
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { under_review(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--diff")
        .arg("--diff-base")
        .arg("other")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // vs `other`: only the line that changed since `other` (not since `main`).
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(
        system.contains("+fn a() { under_review(); }"),
        "system:\n{system}"
    );
    assert!(
        system.contains("-fn a() { on_other(); }"),
        "system:\n{system}"
    );
}

#[test]
fn diff_base_from_config_is_inert_without_diff() {
    // `diff_base` in config only tunes the base for `--diff`; without `--diff`
    // it does nothing — no diff section, whole-file review as before.
    let rules = format!("  - {{ name: r, description: \"{RULE}\" }}\n");
    let p = committed_repo_with_diff_base("main", &rules, &[("src/a.rs", "fn a() {}\n")]);
    git(p.path(), &["checkout", "-q", "-b", "feature"]);
    p.write("src/a.rs", "fn a() { ignored(); }\n");
    git(p.path(), &["commit", "-q", "-am", "feature change"]);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(!system.contains("```diff"), "system:\n{system}");
    assert!(system.contains("- src/a.rs"), "system:\n{system}");
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
        r#"{"description_yields_clear_verdict": true,
            "name_describes_what_the_rule_checks": true,
            "relevance_scopes_conditional_rules": true,
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
    assert!(names.contains(&"name_describes_what_the_rule_checks"));
}

#[test]
fn config_default_omits_sources_block() {
    // Provenance is opt-in: a bare `config` stays lean (file list + config), and
    // `--sources` is the documented way to add the per-item trace.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("rules:\n  - {{ name: my_rule, description: \"{RULE}\" }}\n"),
    );
    let plain: Value = serde_json::from_slice(&p.bare().arg("config").output().unwrap().stdout)
        .expect("config is JSON");
    assert!(plain.get("config_files").is_some());
    assert!(plain.get("config").is_some());
    assert!(plain.get("sources").is_none(), "default must omit sources");

    let with: Value = serde_json::from_slice(
        &p.bare()
            .args(["config", "--sources"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("config --sources is JSON");
    assert!(with["sources"]["rules"]["my_rule"]["source"].is_string());
}

#[test]
fn config_command_traces_every_item_to_its_source() {
    // One `config --sources` run exercising the whole `sources` block through the
    // real binary: a local plugin file, a remote plugin URL, the root file, an
    // agent, the top-level settings (first-writer-wins), and an `override` rule
    // whose field provenance points at the file that set the field.
    let p = Project::new();
    // Local plugin: contributes a base rule, an agent, and a setting the root
    // leaves unset (so the plugin is that setting's source).
    p.write(
        "team.yml",
        &format!(
            "oneharness:\n  model: team-model\nagents:\n  team_agent:\n    harness: claude-code\n\
             rules:\n  - {{ name: team_rule, description: \"{RULE}\" }}\n"
        ),
    );
    // Root: sets version + rationales, plugins the local file and the bundled
    // URL, adds its own rule, and overrides the plugin's `team_rule`.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrationales: false\nplugins:\n  - ./team.yml\n  - {CONFIG_LINT}\n\
             rules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n  \
             - {{ name: team_rule, override: true, judges: 3 }}\n"
        ),
    );

    let out = p.bare().args(["config", "--sources"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let s = &v["sources"];
    let ends = |val: &Value, suffix: &str| {
        let got = val.as_str().unwrap().to_string();
        assert!(got.ends_with(suffix), "expected {suffix:?}, got {got:?}");
    };

    // Settings: root set version + rationales; only the plugin set the model.
    ends(&s["settings"]["version"], "llmlint.yml");
    ends(&s["settings"]["rationales"], "llmlint.yml");
    ends(&s["settings"]["oneharness.model"], "team.yml");

    // Agent declared only by the local plugin.
    ends(&s["agents"]["team_agent"], "team.yml");

    // Rule from the root file, and one from the remote plugin URL. A rule with
    // no override reports only its definition site (no per-field `fields` block).
    ends(&s["rules"]["root_rule"]["source"], "llmlint.yml");
    assert!(s["rules"]["root_rule"]["fields"].is_null());
    assert_eq!(
        s["rules"]["name_describes_what_the_rule_checks"]["source"]
            .as_str()
            .unwrap(),
        CONFIG_LINT
    );

    // `team_rule` is defined in the local plugin, but the root `override` set
    // `judges` — so the rule's definition site is the plugin while `judges`
    // traces to the root file, the field that would actually need editing there.
    ends(&s["rules"]["team_rule"]["source"], "team.yml");
    ends(&s["rules"]["team_rule"]["fields"]["judges"], "llmlint.yml");
    // And the override actually resolved into the merged rule.
    let team_rule = v["config"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "team_rule")
        .unwrap();
    assert_eq!(team_rule["judges"], 3);
    assert_eq!(team_rule["description"].as_str().unwrap(), RULE);
}

#[test]
fn where_command_returns_one_source_path_for_scripting() {
    // The focused lookup: `where <path>` prints exactly the source of an item —
    // and an `override` field resolves to the file that set it, not the base.
    let p = Project::new();
    p.write(
        "team.yml",
        &format!(
            "oneharness:\n  model: team-model\nagents:\n  team_agent:\n    harness: claude-code\n\
             rules:\n  - {{ name: team_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nplugins:\n  - ./team.yml\n  - {CONFIG_LINT}\n\
             rules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n  \
             - {{ name: team_rule, override: true, judges: 3 }}\n"
        ),
    );

    // Output is exactly the source path plus a trailing newline — scriptable.
    let trimmed = |args: &[&str]| -> (i32, String) {
        let out = p.bare().args(args).output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap().trim().to_string(),
        )
    };

    // A dotted setting and a non-dotted one (both kinds of setting key).
    assert!(trimmed(&["where", "oneharness.model"])
        .1
        .ends_with("team.yml"));
    assert!(trimmed(&["where", "version"]).1.ends_with("llmlint.yml"));
    // An agent the local plugin supplies.
    assert!(trimmed(&["where", "agents.team_agent"])
        .1
        .ends_with("team.yml"));
    // A rule's definition site vs. an overridden field's file.
    assert!(trimmed(&["where", "rules.team_rule"])
        .1
        .ends_with("team.yml"));
    assert!(trimmed(&["where", "rules.team_rule.judges"])
        .1
        .ends_with("llmlint.yml"));
    // Fields nobody overrode (a normal field and `name`) resolve to the
    // definition site.
    assert!(trimmed(&["where", "rules.team_rule.description"])
        .1
        .ends_with("team.yml"));
    assert!(trimmed(&["where", "rules.team_rule.name"])
        .1
        .ends_with("team.yml"));
    let (code, root) = trimmed(&["where", "rules.root_rule"]);
    assert_eq!(code, 0);
    assert!(root.ends_with("llmlint.yml"));
    // A rule contributed by a remote plugin resolves to the plugin URL verbatim,
    // not a local path — the source you'd pin/upgrade to change it.
    assert_eq!(
        trimmed(&["where", "rules.name_describes_what_the_rule_checks"]).1,
        CONFIG_LINT
    );
}

#[test]
fn where_honors_explicit_config_and_cwd() {
    // `where` resolves config the same way as `lint`/`config`: `--config` (a path
    // relative to `--cwd`) replaces discovery. If `--cwd` were ignored the
    // relative `--config` would resolve against the process cwd and fail to load.
    let p = Project::new();
    p.write(
        "proj/custom.yml",
        &format!("version: 1\nrules:\n  - {{ name: explicit_rule, description: \"{RULE}\" }}\n"),
    );
    let proj = p.path().join("proj");
    let out = p
        .bare()
        .args(["where", "rules.explicit_rule", "--config", "custom.yml"])
        .arg("--cwd")
        .arg(&proj)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .ends_with("custom.yml"));
}

#[test]
fn where_command_errors_clearly_on_an_unknown_path() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "agents:\n  reviewer: {{}}\n\
             rules:\n  - {{ name: my_rule, description: \"{RULE}\" }}\n"
        ),
    );
    // An unknown rule name exits 2 and names what's available.
    p.bare()
        .args(["where", "rules.nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no rule named"))
        .stderr(predicate::str::contains("my_rule"));
    // An unknown agent name likewise lists the configured agents.
    p.bare()
        .args(["where", "agents.nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no agent named"))
        .stderr(predicate::str::contains("reviewer"));
    // An unknown field of a real rule lists the valid fields.
    p.bare()
        .args(["where", "rules.my_rule.bogus"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown rule field"))
        .stderr(predicate::str::contains("judges"));
    // A real setting left at its default says so rather than pretending a source.
    p.bare()
        .args(["where", "oneharness.bin"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("built-in default"));
    // An unrecognized path shows the accepted forms.
    p.bare()
        .args(["where", "bogus.path"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("expected a setting"));
}

#[test]
fn where_fails_clearly_with_no_config_and_an_invalid_config() {
    // `where` shares the load+validate preflight with the other commands, so its
    // own entry point must surface a missing config and a structurally invalid
    // one as exit-2 errors rather than a panic or a misleading "not found".
    let missing = Project::new();
    missing
        .bare()
        .args(["where", "version"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no llmlint config"));

    let invalid = Project::new();
    // Two rules share a name without `override` -> validation error, not a lookup.
    invalid.write(
        "llmlint.yml",
        &format!(
            "rules:\n  - {{ name: dup, description: \"{RULE}\" }}\n  \
             - {{ name: dup, description: \"{RULE}\" }}\n"
        ),
    );
    invalid
        .bare()
        .args(["where", "rules.dup"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("duplicate rule name"));
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

#[test]
fn doctor_fails_clearly_when_oneharness_is_too_old() {
    // A pre-0.3.0 oneharness can't run read-only mode, so doctor rejects it.
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .env("LLMLINT_MOCK_VERSION", "0.2.9")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("too old"))
        .stderr(predicate::str::contains("0.3.0"));
}

#[test]
fn doctor_fails_clearly_when_oneharness_version_is_unparseable() {
    // A `--version` output with no numeric version can't be checked against the
    // minimum, so the read-only-mode requirement can't be honored: hard error.
    let p = Project::new();
    p.bare()
        .arg("doctor")
        .env("LLMLINT_ONEHARNESS_BIN", mock_path())
        .env("LLMLINT_MOCK_VERSION", "unreleased")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("could not determine"))
        .stderr(predicate::str::contains("0.3.0"));
}

// ---- sibling oneharness resolution -----------------------------------------

/// Copy the built llmlint binary and the mock (named `oneharness`) into one
/// directory, mimicking how `uv tool install` / `pipx` lay out the llmlint-cli
/// wheel and its oneharness-cli dependency inside a private venv `bin/`: both
/// binaries side by side, but only llmlint linked onto PATH.
fn tool_venv_layout(dir: &Path) -> PathBuf {
    let exe = std::env::consts::EXE_SUFFIX;
    let llmlint = dir.join(format!("llmlint{exe}"));
    fs::copy(cargo_bin("llmlint"), &llmlint).unwrap();
    fs::copy(mock_path(), dir.join(format!("oneharness{exe}"))).unwrap();
    llmlint
}

#[test]
fn doctor_finds_a_sibling_oneharness_when_path_has_none() {
    // With no override and no oneharness on PATH, resolution falls back to the
    // binary sitting beside llmlint itself — and doctor's output names that
    // sibling path, so the fallback is visible, not silent.
    let bin = TempDir::new().unwrap();
    let llmlint = tool_venv_layout(bin.path());
    // canonicalize: current_exe resolves symlinked temp roots (e.g. macOS
    // /var -> /private/var), so the printed sibling path is the canonical one.
    let canonical_bin = bin.path().canonicalize().unwrap();
    let p = Project::new();
    Command::from_std(std::process::Command::new(&llmlint))
        .arg("doctor")
        .current_dir(p.path())
        .env("PATH", p.path()) // a real dir, but no oneharness in it
        .assert()
        .success()
        .stdout(predicate::str::contains(canonical_bin.to_str().unwrap()));
}

#[test]
fn lint_uses_a_sibling_oneharness_when_path_has_none() {
    // The full lint engine works through the sibling fallback with no
    // --oneharness-bin flag and no oneharness on PATH: a uv-tool-style install
    // lints out of the box.
    let bin = TempDir::new().unwrap();
    let llmlint = tool_venv_layout(bin.path());
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: sibling_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"sibling_rule": true}"#);
    Command::from_std(std::process::Command::new(&llmlint))
        .current_dir(p.path())
        .env("PATH", p.path())
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn path_oneharness_wins_over_the_sibling() {
    // An environment's chosen oneharness (on PATH) is never shadowed by one
    // bundled beside llmlint: with both present, resolution stays the bare
    // PATH lookup — doctor prints `(oneharness)`, not the sibling's directory.
    let bin = TempDir::new().unwrap();
    let llmlint = tool_venv_layout(bin.path());
    let pathdir = TempDir::new().unwrap();
    let exe = std::env::consts::EXE_SUFFIX;
    fs::copy(mock_path(), pathdir.path().join(format!("oneharness{exe}"))).unwrap();
    let p = Project::new();
    Command::from_std(std::process::Command::new(&llmlint))
        .arg("doctor")
        .current_dir(p.path())
        .env("PATH", pathdir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("(oneharness)"));
}

#[test]
fn lint_fails_clearly_when_oneharness_is_too_old() {
    // The pre-flight version gate stops the run before any judge call.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: old_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let runlog = p.path().join("runlog");
    p.lint()
        .env("LLMLINT_MOCK_VERSION", "0.2.529")
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("too old"));
    // The gate fires before planning, so no judge ran.
    assert!(
        !runlog.exists() || fs::read_dir(&runlog).unwrap().count() == 0,
        "no oneharness `run` should happen when the version gate fails"
    );
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
fn oneharness_runs_in_read_only_mode() {
    // llmlint is a judge, never an editor: every `run` must carry
    // `--mode read-only` so the harness can read target files but not edit them.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: ro_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"ro_rule": true}"#);
    let args_dump = p.path().join("ro-args.txt");
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .assert()
        .success();
    let dumped = fs::read_to_string(&args_dump).unwrap();
    let mode = dumped
        .lines()
        .skip_while(|l| *l != "--mode")
        .nth(1)
        .expect("--mode flag should be forwarded on every run");
    assert_eq!(mode, "read-only");
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

// ---- nested discovery (walk up the tree; most-local wins) -----------------

#[test]
fn nested_configs_are_discovered_up_the_tree_and_most_local_wins() {
    let p = Project::new();
    // A user/project/local layout: a "user" config at the project root, a project
    // config in a subdir, and a local config in the leaf where linting runs. All
    // three are discovered by walking up; every config contributes its rules and
    // the most-local config wins each top-level scalar.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"**/*.rs\"]\noneharness:\n  model: user-model\n\
             rules:\n  - {{ name: user_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "proj/llmlint.yml",
        &format!(
            "oneharness:\n  model: proj-model\n\
             rules:\n  - {{ name: proj_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "proj/src/llmlint.yml",
        &format!("rules:\n  - {{ name: local_rule, description: \"{RULE}\" }}\n"),
    );
    p.write("proj/src/lib.rs", "// code\n");
    let leaf = p.path().join("proj/src");
    let verdicts =
        p.write_verdicts(r#"{"user_rule": true, "proj_rule": true, "local_rule": true}"#);
    let args_dump = p.path().join("args.txt");

    let out = p
        .lint()
        .arg("--cwd")
        .arg(&leaf)
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args_dump)
        .output()
        .unwrap();
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
    // Every level along the walk contributes its rule.
    assert!(names.contains(&"local_rule"), "got: {names:?}");
    assert!(names.contains(&"proj_rule"), "got: {names:?}");
    assert!(names.contains(&"user_rule"), "got: {names:?}");

    // The most-local config that set `oneharness.model` wins: local left it unset,
    // so the project config (nearer than the user root) supplies it.
    let dumped = fs::read_to_string(&args_dump).unwrap();
    let model = dumped
        .lines()
        .skip_while(|l| *l != "--model")
        .nth(1)
        .expect("--model forwarded to oneharness");
    assert_eq!(model, "proj-model");
}

#[test]
fn cascade_scopes_a_subtree_configs_globs_to_its_own_directory() {
    let p = Project::new();
    // Run from the project root. A subtree config in `frontend/` governs its own
    // files: its `*.txt` is rooted at `frontend/`, not the run cwd. The root
    // config lints `**/*.rs` tree-wide.
    p.write(
        "llmlint.yml",
        &format!("version: 1\nfiles:\n  include: [\"**/*.rs\"]\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "frontend/llmlint.yml",
        &format!("files:\n  include: [\"*.txt\"]\nrules:\n  - {{ name: front_rule, description: \"{RULE}\" }}\n"),
    );
    p.write("top.rs", "// code\n");
    p.write("frontend/note.txt", "frontend text\n");
    p.write("outside.txt", "root text\n"); // a .txt OUTSIDE the frontend glob root

    // Select only front_rule -> a single oneharness run, so the dumped system
    // prompt is exactly that rule's file set.
    let front_dump = p.path().join("front.txt");
    let verdicts = p.write_verdicts(r#"{"front_rule": true}"#);
    p.lint()
        .arg("--rule")
        .arg("front_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &front_dump)
        .assert()
        .success();
    let front = fs::read_to_string(&front_dump).unwrap();
    // front_rule sees its subtree file (reported relative to cwd as
    // `frontend/note.txt`) and NOT the same-extension file outside its directory.
    assert!(front.contains("frontend/note.txt"), "system:\n{front}");
    assert!(
        !front.contains("outside.txt"),
        "a subtree config's `*.txt` must not reach files outside its directory:\n{front}"
    );

    // The root config still lints `**/*.rs` tree-wide; it does not pick up .txt.
    let root_dump = p.path().join("root.txt");
    let verdicts = p.write_verdicts(r#"{"root_rule": true}"#);
    p.lint()
        .arg("--rule")
        .arg("root_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &root_dump)
        .assert()
        .success();
    let root = fs::read_to_string(&root_dump).unwrap();
    assert!(root.contains("top.rs"), "system:\n{root}");
    assert!(!root.contains("note.txt"), "system:\n{root}");
}

#[test]
fn cascade_subtree_config_with_no_files_block_lints_its_whole_subtree() {
    // The whole-tree default applied at a cascade scope: a subtree config with
    // rules but no `files` block lints EVERY file under its own directory (any
    // extension, nested included), still bounded to that subtree — it never
    // reaches sibling/root files.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nfiles:\n  include: [\"*.rs\"]\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "frontend/llmlint.yml",
        &format!("rules:\n  - {{ name: front_rule, description: \"{RULE}\" }}\n"),
    );
    p.write("top.rs", "// root code\n");
    p.write("frontend/note.txt", "text\n");
    p.write("frontend/deep/app.ts", "// ts\n");

    // Select only front_rule -> its dumped prompt is exactly that rule's file set.
    let front_dump = p.path().join("front.txt");
    let verdicts = p.write_verdicts(r#"{"front_rule": true}"#);
    p.lint()
        .arg("--rule")
        .arg("front_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &front_dump)
        .assert()
        .success();
    let front = fs::read_to_string(&front_dump).unwrap();
    // Every file under frontend/ (reported relative to cwd), regardless of
    // extension and depth; nothing from the root.
    assert!(front.contains("frontend/note.txt"), "system:\n{front}");
    assert!(front.contains("frontend/deep/app.ts"), "system:\n{front}");
    assert!(
        !front.contains("top.rs"),
        "a subtree config's whole-tree default must not reach root files:\n{front}"
    );
}

#[test]
fn subtree_agent_used_by_an_outside_rule_is_a_footgun_error() {
    // A subtree config's agent must not silently retune how a rule OUTSIDE that
    // subtree is judged. Here a root rule references an agent defined only in the
    // subtree — a hard exit-2 error, naming the rule, the agent, and the subtree
    // config, rather than letting the nested folder change linting for the root.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\", agent: shared }}\n"
        ),
    );
    p.write(
        "frontend/llmlint.yml",
        "agents:\n  shared:\n    model: cheap\n",
    );
    p.write("top.rs", "// code\n");

    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("root_rule"))
        .stderr(predicate::str::contains("shared"))
        .stderr(predicate::str::contains("subtree config"));
}

#[test]
fn subtree_agent_used_by_its_own_subtree_rule_is_allowed() {
    // The legitimate case the footgun guard must NOT break: a subtree rule using an
    // agent defined in the same subtree resolves cleanly (the agent lives at/above
    // the rule).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "frontend/llmlint.yml",
        &format!(
            "agents:\n  area:\n    harness: claude-code\nrules:\n  \
             - {{ name: area_rule, description: \"{RULE}\", agent: area }}\n"
        ),
    );
    p.write("top.rs", "// code\n");
    p.write("frontend/app.rs", "// area code\n");
    let verdicts = p.write_verdicts(r#"{"root_rule": true, "area_rule": true}"#);

    p.lint()
        .arg("--rule")
        .arg("area_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn nested_discovery_traces_sources_up_and_down_the_tree() {
    // The intersection of nested discovery (walk up + cascade down) with source
    // tracking: `config --sources` and `where` must trace every item to the exact
    // file it came from across the whole tree, and a descendant config's settings
    // must neither take effect nor appear as a setting's source.
    let p = Project::new();
    // Ancestor of the run cwd: sets `rationales` and a rule.
    p.write(
        "llmlint.yml",
        &format!("rationales: false\nrules:\n  - {{ name: user_rule, description: \"{RULE}\" }}\n"),
    );
    // The run cwd: sets the model + a rule.
    p.write(
        "proj/llmlint.yml",
        &format!(
            "version: 1\noneharness:\n  model: proj-model\nrules:\n  \
             - {{ name: proj_rule, description: \"{RULE}\" }}\n"
        ),
    );
    // A subtree under cwd: a scoped rule, plus a session setting (`timeout`) that
    // nothing above sets — a descendant must NOT be able to retune the run.
    p.write(
        "proj/frontend/llmlint.yml",
        &format!(
            "oneharness:\n  timeout: 5\nrules:\n  - {{ name: front_rule, description: \"{RULE}\" }}\n"
        ),
    );
    let proj = p.path().join("proj");

    let out = p
        .bare()
        .args(["config", "--sources", "--cwd"])
        .arg(&proj)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let s = &v["sources"];
    // Provenance reports OS-native paths; normalize separators so the suffix
    // assertions below hold on Windows too.
    let src = |val: &Value| val.as_str().unwrap_or_default().replace('\\', "/");

    // Each rule traces to its own file across the whole tree — including a
    // descendant rule to the subtree config.
    let user_src = src(&s["rules"]["user_rule"]["source"]);
    assert!(
        user_src.ends_with("llmlint.yml") && !user_src.contains("proj"),
        "ancestor rule should trace to the root config: {user_src}"
    );
    assert!(
        src(&s["rules"]["proj_rule"]["source"]).ends_with("proj/llmlint.yml"),
        "cwd rule: {}",
        src(&s["rules"]["proj_rule"]["source"])
    );
    assert!(
        src(&s["rules"]["front_rule"]["source"]).ends_with("proj/frontend/llmlint.yml"),
        "subtree rule: {}",
        src(&s["rules"]["front_rule"]["source"])
    );
    // Settings trace to cwd-and-up only.
    assert!(src(&s["settings"]["oneharness.model"]).ends_with("proj/llmlint.yml"));
    let rat_src = src(&s["settings"]["rationales"]);
    assert!(
        rat_src.ends_with("llmlint.yml") && !rat_src.contains("proj"),
        "rationales is set by the ancestor: {rat_src}"
    );
    // The descendant-only setting neither takes effect nor appears as a source.
    assert!(
        v["config"]["oneharness"]["timeout"].is_null(),
        "a descendant must not retune the session: {}",
        v["config"]["oneharness"]
    );
    assert!(
        s["settings"].get("oneharness.timeout").is_none(),
        "a descendant setting must not be a provenance source: {s}"
    );

    // `where` resolves the same way for a single item.
    let trimmed = |args: &[&str]| -> (i32, String) {
        let out = p
            .bare()
            .args(args)
            .arg("--cwd")
            .arg(&proj)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout)
                .unwrap()
                .trim()
                .replace('\\', "/"),
        )
    };
    let (code, front) = trimmed(&["where", "rules.front_rule"]);
    assert_eq!(code, 0, "where rules.front_rule should resolve");
    assert!(front.ends_with("proj/frontend/llmlint.yml"), "got {front}");
    // The model resolves to cwd, never the descendant that also names a setting.
    assert!(trimmed(&["where", "oneharness.model"])
        .1
        .ends_with("proj/llmlint.yml"));
    let user = trimmed(&["where", "rules.user_rule"]).1;
    assert!(
        user.ends_with("llmlint.yml") && !user.contains("proj"),
        "got {user}"
    );
}

#[test]
fn cascade_override_across_the_tree_traces_each_field_to_its_file() {
    // Nesting introduces a new way an `override` spans files: a base rule defined
    // up the tree (a user/project-level config) refined by the local config at the
    // run cwd. Field-level provenance must resolve across that directory split —
    // the definition stays the ancestor, the overridden field points at the cwd
    // file — and a subtree agent traces to the subtree.
    let p = Project::new();
    // Ancestor: the base rule (its definition site).
    p.write(
        "llmlint.yml",
        &format!("rules:\n  - {{ name: shared, description: \"{RULE}\" }}\n"),
    );
    // Run cwd: overrides the ancestor's rule, changing only `judges`.
    p.write(
        "proj/llmlint.yml",
        "version: 1\nrules:\n  - { name: shared, override: true, judges: 3 }\n",
    );
    // A subtree: an agent (referenced by its own rule) to trace through `where`.
    p.write(
        "proj/area/llmlint.yml",
        &format!(
            "agents:\n  area_agent:\n    harness: claude-code\nrules:\n  \
             - {{ name: area_rule, agent: area_agent, description: \"{RULE}\" }}\n"
        ),
    );
    let proj = p.path().join("proj");

    let out = p
        .bare()
        .args(["config", "--sources", "--cwd"])
        .arg(&proj)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    // The override took effect: the merged rule carries judges = 3.
    let shared = v["config"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "shared")
        .expect("shared rule present");
    assert_eq!(shared["judges"], 3);
    // Provenance splits the rule across files: definition at the ancestor, the
    // overridden `judges` at the cwd config.
    let sh = &v["sources"]["rules"]["shared"];
    // Provenance reports OS-native paths; normalize separators so the suffix
    // assertions below hold on Windows too.
    let src = |val: &Value| val.as_str().unwrap_or_default().replace('\\', "/");
    assert!(
        src(&sh["source"]).ends_with("llmlint.yml") && !src(&sh["source"]).contains("proj"),
        "definition site is the ancestor: {}",
        src(&sh["source"])
    );
    assert!(
        src(&sh["fields"]["judges"]).ends_with("proj/llmlint.yml"),
        "overridden field points at the cwd file: {}",
        src(&sh["fields"]["judges"])
    );
    // The subtree agent traces to the subtree config.
    assert!(src(&v["sources"]["agents"]["area_agent"]).ends_with("proj/area/llmlint.yml"));

    // `where` agrees, for scripting.
    let where1 = |path: &str| -> String {
        let out = p
            .bare()
            .args(["where", path, "--cwd"])
            .arg(&proj)
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .replace('\\', "/")
    };
    assert!(where1("rules.shared.judges").ends_with("proj/llmlint.yml"));
    let def = where1("rules.shared");
    assert!(
        def.ends_with("llmlint.yml") && !def.contains("proj"),
        "{def}"
    );
    assert!(where1("agents.area_agent").ends_with("proj/area/llmlint.yml"));
}

#[test]
fn duplicate_rule_name_across_sibling_subtrees_is_rejected() {
    // Rule names are one namespace across the whole discovered tree, so two sibling
    // subtrees that both define the same rule (without `override`) is a clear
    // exit-2 config error, not a silent last-writer-wins.
    let p = Project::new();
    p.write(
        "proj/llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "proj/a/llmlint.yml",
        &format!("rules:\n  - {{ name: dup, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "proj/b/llmlint.yml",
        &format!("rules:\n  - {{ name: dup, description: \"{RULE}\" }}\n"),
    );
    let proj = p.path().join("proj");

    let out = p
        .bare()
        .arg("config")
        .arg("--cwd")
        .arg(&proj)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("duplicate rule name"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cascade_per_rule_files_root_at_the_subtree_directory() {
    // A subtree rule's *own* `files` glob roots at the subtree directory, just like
    // the config-level default — not at the run cwd. So a per-rule `*.md` in a
    // subtree reaches that subtree's markdown and nothing above it.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "area/llmlint.yml",
        &format!(
            "rules:\n  - name: md_rule\n    description: \"{RULE}\"\n    files:\n      include: [\"*.md\"]\n"
        ),
    );
    p.write("area/note.md", "# area note\n");
    p.write("top.md", "# top note\n"); // a .md ABOVE the subtree — must not match

    let dump = p.path().join("system.txt");
    let verdicts = p.write_verdicts(r#"{"md_rule": true}"#);
    p.lint()
        .arg("--rule")
        .arg("md_rule")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("area/note.md"), "system:\n{system}");
    assert!(
        !system.contains("top.md"),
        "a subtree rule's per-rule glob must not escape its directory:\n{system}"
    );
}

#[test]
fn lint_runs_when_the_only_config_is_in_a_subtree() {
    // Running from a directory with no config of its own — but with a configured
    // subtree below it — is a valid run (the configured part of the project), not a
    // ConfigNotFound. The subtree config lints its own files.
    let p = Project::new();
    p.write(
        "area/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"*.rs\"]\nrules:\n  \
             - {{ name: area_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("area/code.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"area_rule": true}"#);

    // Process cwd is the project root, which has no config; discovery finds the
    // subtree config and lints `area/code.rs` (reported relative to cwd).
    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
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
    assert_eq!(names, ["area_rule"]);
}

/// Collect every rule name in a JSON report's `rules` array, in report order
/// (the report sorts by name), failing loudly on a non-zero exit.
fn lint_rule_names(out: &std::process::Output) -> Vec<String> {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn explicit_file_outside_a_subtree_is_not_judged_by_that_subtrees_rule() {
    // Regression: with explicit files on the command line, a subtree config's rule
    // used to be evaluated against *every* passed file — even ones outside its own
    // directory, because the CLI list short-circuited the rule's directory scope.
    // Pass one file under the subtree and one above it; the subtree rule must judge
    // only the file under its directory (the "consolidated up from each leaf"
    // scoping), never the unrelated one.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"**/*.rs\"]\nrules:\n  \
             - {{ name: root_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "backend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.rs\"]\nrules:\n  \
             - {{ name: backend_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("app.rs", "// top-level code\n");
    p.write("backend/svc.rs", "// backend code\n");

    // Select backend_rule alone so the single oneharness run's dumped system
    // prompt is exactly that rule's resolved file set.
    let dump = p.path().join("backend.txt");
    let verdicts = p.write_verdicts(r#"{"backend_rule": true}"#);
    p.lint()
        .arg("app.rs")
        .arg("backend/svc.rs")
        .arg("--rule")
        .arg("backend_rule")
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("backend/svc.rs"),
        "the file under the subtree should be judged:\n{system}"
    );
    assert!(
        !system.contains("app.rs"),
        "a subtree rule must not judge an explicit file outside its directory:\n{system}"
    );
}

#[test]
fn a_sibling_subtrees_name_clash_does_not_break_an_unrelated_explicit_file_run() {
    // Regression: two independent areas each name a rule the same in their own
    // subtree config. They never collide in practice — you lint one area at a time
    // — but the cascade used to load *every* subtree config regardless of the files
    // being linted, so the duplicate name aborted an unrelated run (exit 2) before
    // any judge call. Relevance-gated discovery loads only the subtree a passed
    // file lives under, so linting one area never trips the other's config.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "frontend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.js\"]\nrules:\n  \
             - {{ name: no_todos, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "backend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.py\"]\nrules:\n  \
             - {{ name: no_todos, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("frontend/app.js", "// code\n");
    p.write("backend/svc.py", "# code\n");

    // Lint only a frontend file: backend's clashing config must not be loaded.
    let verdicts = p.write_verdicts(r#"{"root_rule": true, "no_todos": true}"#);
    let out = p
        .lint()
        .arg("frontend/app.js")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    let names = lint_rule_names(&out);
    assert_eq!(
        names,
        ["no_todos", "root_rule"],
        "only frontend's rule (plus the root rule) should run; backend's clashing \
         config must be gated out: {names:?}"
    );
}

#[test]
fn subtree_rules_are_loaded_only_when_an_explicit_file_falls_under_them() {
    // New behavior: with explicit files, each subtree config is picked up only when
    // a passed file lives under it — the rule set is the union of "walk up from each
    // leaf". The root rule (rooted at cwd) spans every passed file; a subtree rule
    // joins the run only for its own area.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "frontend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.js\"]\nrules:\n  \
             - {{ name: front_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "backend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.py\"]\nrules:\n  \
             - {{ name: back_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("frontend/app.js", "// code\n");
    p.write("backend/svc.py", "# code\n");
    let verdicts =
        p.write_verdicts(r#"{"root_rule": true, "front_rule": true, "back_rule": true}"#);

    // Only a frontend file: the backend subtree never joins the run.
    let out = p
        .lint()
        .arg("frontend/app.js")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(
        lint_rule_names(&out),
        ["front_rule", "root_rule"],
        "an irrelevant subtree must not be loaded for an explicit-file run"
    );

    // Files from both areas: each area's rule joins, and the root rule spans both.
    let out = p
        .lint()
        .arg("frontend/app.js")
        .arg("backend/svc.py")
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(
        lint_rule_names(&out),
        ["back_rule", "front_rule", "root_rule"],
        "every subtree with a passed file under it should join the run"
    );
}

#[test]
fn check_ignores_scopes_explicit_files_to_subtrees_like_lint() {
    // The fast, model-free `check-ignores` must resolve the same subtree-scoped
    // files `lint` does: an explicit file outside a subtree never pulls that
    // subtree's directives into scope, and a malformed directive in a subtree file
    // is only checked once that file is in scope.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: root_rule, description: \"{RULE}\" }}\n"),
    );
    p.write(
        "backend/llmlint.yml",
        &format!(
            "files:\n  include: [\"**/*.py\"]\nrules:\n  \
             - {{ name: back_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("app.rs", "// code\n");
    // A malformed directive (names a rule but gives no reason) in the backend file.
    p.write("backend/svc.py", "# llmlint: ignore[back_rule]\n");

    // Checking only the top-level file: the backend subtree is out of scope, so its
    // malformed directive is never reached — clean exit.
    p.check_ignores().arg("app.rs").assert().success();

    // Passing the backend file pulls it into scope: the malformed directive fails.
    p.check_ignores()
        .arg("backend/svc.py")
        .assert()
        .code(2)
        .stderr(
            predicate::str::contains("backend/svc.py")
                .and(predicate::str::contains("give a reason")),
        );
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

// ---- per-rule files precedence over global globs --------------------------

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
fn agent_files_is_a_removed_field() {
    // `agent.files` was removed (it duplicated per-rule `files` and let a subtree
    // agent silently retarget outside rules). A config that still sets it is a
    // clear exit-2 error, not a silent accept.
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

    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("files"));
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

#[test]
fn batches_are_balanced_rather_than_packed() {
    // 4 rules at batch_size 3: the fewest batches that respect the cap is 2, and
    // those 2 are balanced into 2+2 — not packed into 3+1.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  default:\n    batch_size: 3\n\
             rules:\n  \
             - {{ name: rule_a, description: \"{RULE}\" }}\n  \
             - {{ name: rule_b, description: \"{RULE}\" }}\n  \
             - {{ name: rule_c, description: \"{RULE}\" }}\n  \
             - {{ name: rule_d, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts =
        p.write_verdicts(r#"{"rule_a": true, "rule_b": true, "rule_c": true, "rule_d": true}"#);
    let runlog = p.path().join("runlog");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success();

    let calls = runlog_calls(&runlog);
    assert_eq!(calls.len(), 2, "4 rules / cap 3 -> 2 batches: {calls:?}");
    let mut sizes: Vec<usize> = calls.iter().map(|c| c.split(',').count()).collect();
    sizes.sort_unstable();
    assert_eq!(
        sizes,
        vec![2, 2],
        "the 2 batches are balanced 2+2, not packed 3+1: {calls:?}"
    );
    // Every rule still appears exactly once across the batches.
    let joined = calls.join(",");
    for name in ["rule_a", "rule_b", "rule_c", "rule_d"] {
        assert_eq!(
            joined.matches(name).count(),
            1,
            "{name} should appear exactly once: {calls:?}"
        );
    }
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
fn default_prompt_documents_line_and_block_ignore_directives() {
    // llmlint now enforces ignores deterministically after the judge answers, so
    // the prompt keeps only the line/block guidance (as a backstop) — the
    // file-scoped form's guidance is dropped since it can't be missed.
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
        "prompt should document the line-scoped directive: {prompt}"
    );
    assert!(
        prompt.contains("ignore-block"),
        "prompt should document the block-scoped form: {prompt}"
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

#[test]
fn well_formed_block_directives_pass_validation() {
    // A block opened for two rules and closed at different points, all matched.
    let p = ignore_project(
        "// llmlint: ignore-block[no_todo] legacy region, tracked in JIRA-9\n\
         fn legacy() {}\n\
         // llmlint: ignore-end[no_todo]\n",
    );
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();
}

#[test]
fn unclosed_block_directive_is_rejected() {
    let p = ignore_project("// llmlint: ignore-block[no_todo] forgot to close it\nfn f() {}\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unclosed ignore-block"))
        .stderr(predicate::str::contains("src/lib.rs:1:"));
}

#[test]
fn block_end_without_a_matching_open_is_rejected() {
    let p = ignore_project("// llmlint: ignore-end[no_todo]\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no open ignore-block"))
        .stderr(predicate::str::contains("src/lib.rs:1:"));
}

#[test]
fn block_open_without_reason_is_rejected() {
    let p = ignore_project("// llmlint: ignore-block[no_todo]\n// llmlint: ignore-end[no_todo]\n");
    p.lint()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("give a reason"));
}

#[test]
fn default_prompt_documents_block_directives() {
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
        prompt.contains("ignore-block"),
        "prompt should document the block-open directive: {prompt}"
    );
    assert!(
        prompt.contains("ignore-end"),
        "prompt should document the block-close directive: {prompt}"
    );
}

// ---- standalone `check-ignores` command -----------------------------------

#[test]
fn check_ignores_validates_well_formed_directives_with_no_harness() {
    // The point of the standalone command: a deterministic, model-free check.
    // It is wired with no `--oneharness-bin` and never spawns oneharness, yet
    // validates the directives and exits 0 — so it belongs in the fast loop.
    let p = ignore_project(
        "// llmlint: ignore[no_todo] tracked in JIRA-1\n\
         // llmlint: ignore-block[no_todo] legacy region, see #7\n\
         fn legacy() {}\n\
         // llmlint: ignore-end[no_todo]\n",
    );
    p.check_ignores()
        .assert()
        .success()
        .stdout(predicate::str::contains("ignore directives OK"));
}

#[test]
fn check_ignores_rejects_a_malformed_directive() {
    let p = ignore_project("// llmlint: ignore[no_todo]\n");
    p.check_ignores()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("give a reason"))
        .stderr(predicate::str::contains("src/lib.rs:1:"));
}

#[test]
fn check_ignores_reports_every_malformed_directive_across_files() {
    // Same as the lint pre-flight: all problems surface in one located error.
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
    p.check_ignores()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("src/a.rs:1:"))
        .stderr(predicate::str::contains("src/b.rs:1:"))
        .stderr(predicate::str::contains("give a reason"))
        .stderr(predicate::str::contains("unknown rule"));
}

#[test]
fn check_ignores_scopes_to_explicit_files_like_lint() {
    // Passing files overrides the config globs: a malformed directive in a file
    // not listed on the CLI is not scanned, exactly as for a lint run — so a
    // pre-commit hook can pass just the changed files.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/good.rs", "// llmlint: ignore[no_todo] handled here\n");
    p.write("src/bad.rs", "// llmlint: ignore[no_todo]\n");
    p.check_ignores().arg("src/good.rs").assert().success();
}

#[test]
fn check_ignores_known_set_is_the_full_config() {
    // A directive may name any configured rule, not just one a lint run would
    // select; an unconfigured name is still rejected.
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
    p.check_ignores().assert().success();
}

#[test]
fn check_ignores_rejects_an_invalid_config() {
    // Config is validated first, so a bad config is a clear exit-2 error rather
    // than a confusing scan failure.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
         - { name: dupe, description: \"x\" }\n  \
         - { name: dupe, description: \"y\" }\n",
    );
    p.write("src/lib.rs", "// code\n");
    p.check_ignores()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid config"));
}

#[test]
fn check_ignores_honors_explicit_config_flag() {
    // `-c/--config` replaces upward discovery: with no `llmlint.yml` at the root,
    // the run only succeeds because the custom-named config is loaded explicitly.
    let p = Project::new();
    p.write(
        "custom.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// llmlint: ignore[no_todo] handled here\n");
    p.check_ignores()
        .arg("--config")
        .arg("custom.yml")
        .assert()
        .success()
        .stdout(predicate::str::contains("ignore directives OK"));
}

#[test]
fn check_ignores_skips_files_of_a_disabled_rule_like_lint() {
    // Parity with the lint pre-flight: a `relevance: false` rule never runs, so
    // its target files are not scanned. A malformed directive reachable only
    // through the disabled rule is therefore not caught — were the rule enabled,
    // this would be an exit-2 error.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: active, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: off_rule, description: \"{RULE}\", relevance: false, \
                files: {{ include: [\"legacy/**\"] }} }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write("legacy/old.rs", "// llmlint: ignore[active]\n");
    p.check_ignores().assert().success();
}

#[test]
fn check_ignores_skips_binary_files_in_the_target_set() {
    // A non-UTF-8 file matched by the globs can't carry a text directive; it is
    // skipped rather than failing the scan, so a real directive elsewhere still
    // validates and the run is clean.
    let p = ignore_project("// llmlint: ignore[no_todo] handled here\n");
    fs::write(p.path().join("src/blob.bin"), [0xff, 0xfe, 0x00, 0xff]).unwrap();
    p.check_ignores().assert().success();
}

#[test]
fn check_ignores_honors_cwd_for_discovery_and_scanning() {
    // `--cwd` is the base for config discovery and the glob root, so a malformed
    // directive under it is caught even when the process cwd is elsewhere.
    let p = Project::new();
    p.write(
        "proj/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("proj/src/lib.rs", "// llmlint: ignore[no_todo]\n");
    let proj = p.path().join("proj");
    p.check_ignores()
        .arg("--cwd")
        .arg(&proj)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("give a reason"))
        .stderr(predicate::str::contains("src/lib.rs:1:"));
}

#[test]
fn check_ignores_resolves_subtree_files_via_the_cascade_like_lint() {
    // `check-ignores` resolves the same files a lint run would under nested
    // discovery: a subtree config's rule scopes its directive scan to that
    // subtree (globs rooted there). A malformed directive in the subtree is
    // caught; a same-extension file *above* the subtree is out of that rule's
    // scope and is not scanned — so the two never disagree about coverage.
    let p = Project::new();
    p.write(
        "proj/llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"**/*.rs\"]\nrules:\n  \
             - {{ name: root_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write(
        "proj/area/llmlint.yml",
        &format!(
            "rules:\n  - name: area_rule\n    description: \"{RULE}\"\n    \
             files:\n      include: [\"*.md\"]\n"
        ),
    );
    // Malformed (no reason) in the subtree — must be caught, located relative to
    // cwd as `area/note.md`.
    p.write("proj/area/note.md", "<!-- llmlint: ignore[area_rule] -->\n");
    // A same-extension file ABOVE the subtree, with its own malformed directive:
    // it is outside `area_rule`'s subtree-rooted `*.md`, so it is never scanned.
    p.write("proj/top.md", "<!-- llmlint: ignore[area_rule] -->\n");
    let proj = p.path().join("proj");

    let out = p.check_ignores().arg("--cwd").arg(&proj).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("area/note.md:1:"),
        "subtree file should be scanned: {stderr}"
    );
    assert!(
        !stderr.contains("top.md"),
        "a file above the subtree rule's scope must not be scanned: {stderr}"
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

// ---- line attribution -----------------------------------------------------

#[test]
fn require_line_attribution_marks_file_and_line_required_in_the_schema() {
    let p = Project::new();
    // One rule opts into line attribution and one doesn't, sharing a file set so
    // both land in one schema where they must differ per rule.
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: located_rule, description: \"{RULE}\", require_line_attribution: true }}\n  \
             - {{ name: free_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"located_rule": true, "free_rule": true}"#);
    let schema_dump = p.path().join("schema.json");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP_SCHEMA", &schema_dump)
        .assert()
        .success();

    let schema: Value = serde_json::from_str(&fs::read_to_string(&schema_dump).unwrap()).unwrap();
    // The opted-in rule requires both file and line on every violation item...
    let located = &schema["properties"]["located_rule"]["properties"]["violations"]["items"];
    assert_eq!(located["required"], serde_json::json!(["file", "line"]));
    assert_eq!(located["properties"]["line"]["minimum"], 1);
    // ...while the plain rule keeps every violation field optional.
    let free = &schema["properties"]["free_rule"]["properties"]["violations"]["items"];
    assert!(free.get("required").is_none());
}

#[test]
fn line_attribution_guidance_and_marker_reach_the_prompt_only_when_required() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: located_rule, description: \"{RULE}\", require_line_attribution: true }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"located_rule": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let system = fs::read_to_string(&dump).unwrap();
    assert!(system.contains("## Line attribution"), "system:\n{system}");
    assert!(
        system.contains("Every violation must cite a `file` and `line`."),
        "system:\n{system}"
    );

    // A config with no opted-in rule renders no line-attribution guidance.
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
    assert!(!off.contains("## Line attribution"), "system:\n{off}");
}

#[test]
fn a_localized_violation_passes_through_for_a_require_line_attribution_rule() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: located_rule, description: \"{RULE}\", require_line_attribution: true }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // The judge localized the violation -> a normal failure (exit 1), no error.
    let verdicts = p.write_verdicts(
        r#"{"located_rule": {"holds": false,
            "violations": [{"file": "src/lib.rs", "line": 1, "message": "bad here"}]}}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL located_rule"))
        .stdout(predicate::str::contains("src/lib.rs:1: bad here"))
        .stdout(predicate::str::contains("requires a file and line").not());
}

#[test]
fn an_unlocalized_violation_for_a_require_line_attribution_rule_is_an_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: located_rule, description: \"{RULE}\", require_line_attribution: true }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    // The judge failed the rule but reported a violation with no file/line. The
    // schema would have oneharness re-prompt; the mock doesn't, so llmlint's
    // backstop turns the unlocalized violation into a clear exit-2 error that
    // batches the offending messages.
    let verdicts = p.write_verdicts(
        r#"{"located_rule": {"holds": false,
            "violations": [{"message": "drifted"}, {"message": "also drifted"}]}}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "rule \"located_rule\" requires a file and line for every violation",
        ))
        .stdout(predicate::str::contains("2 violations"))
        .stdout(predicate::str::contains("\"drifted\""))
        .stdout(predicate::str::contains("\"also drifted\""));

    // The machine contract carries the same error in its `errors` array (exit 2).
    p.lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"errored\": 1"))
        .stdout(predicate::str::contains("requires a file and line"));
}

// ---- per-file applicability, scope validation + deterministic ignores ------

#[test]
fn agent_rules_with_distinct_files_merge_into_one_call_with_per_file_context() {
    // Two default-agent rules scoped to different directories now share ONE
    // oneharness call over the union of their files (fewer invocations), and the
    // prompt tells the judge, per file, which rules apply — picking the shorter of
    // an apply-list or a skip-list so the context stays token-cheap.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// code\n");
    p.write("docs/b.md", "# doc\n");
    let verdicts = p.write_verdicts(r#"{"rule_src": true, "rule_docs": true}"#);
    let dump = p.path().join("system.txt");
    let runlog = p.path().join("runlog");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success();

    // One merged call carrying both rules.
    let calls = runlog_calls(&runlog);
    assert_eq!(
        calls.len(),
        1,
        "rules over distinct files merge into one call: {calls:?}"
    );
    assert!(
        calls[0].contains("rule_src") && calls[0].contains("rule_docs"),
        "{calls:?}"
    );

    // Per-file context: src/a.rs lists the shorter apply-list (only rule_src);
    // docs/b.md lists the shorter skip-list (all apply except rule_src).
    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("src/a.rs — only these rules apply: rule_src"),
        "system:\n{system}"
    );
    assert!(
        system.contains("docs/b.md — all rules apply except: rule_src"),
        "system:\n{system}"
    );
}

#[test]
fn wrong_file_violation_triggers_a_rework_then_passes() {
    // A judge flags rule_src in docs/b.md — a file rule_src does not cover. llmlint
    // rejects the verdict and re-asks (a second oneharness call) with the correct
    // per-file rule scope; the corrected verdict holds, so the run exits clean.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// code\n");
    p.write("docs/b.md", "# doc\n");
    // First call: rule_src wrongly reports a violation in docs/b.md (out of its
    // scope). Second call (the rework): it holds.
    let verdicts = p.write_verdicts(
        r#"{"rule_src": [{"holds": false, "violations": [
                {"file": "docs/b.md", "line": 1, "message": "wrong file"}]}, true],
            "rule_docs": true}"#,
    );
    let state = p.path().join("state");
    let runlog = p.path().join("runlog");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success()
        .stdout(predicate::str::contains("wrong file").not());

    let calls = runlog_calls(&runlog);
    assert_eq!(
        calls.len(),
        2,
        "the wrong-file verdict is reworked once: {calls:?}"
    );
}

#[test]
fn unfixed_wrong_file_violation_is_dropped_after_the_rework() {
    // If the judge keeps reporting the wrong-file violation even after the rework,
    // llmlint drops it deterministically and — since that was the verdict's only
    // basis — flips the fail to a pass, so a wrong-file finding never reddens the
    // build. Exactly MAX_REWORKS (1) corrective call is made, then it stops.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// code\n");
    p.write("docs/b.md", "# doc\n");
    // Constant spec: every call reports the same out-of-scope violation.
    let verdicts = p.write_verdicts(
        r#"{"rule_src": {"holds": false, "violations": [
                {"file": "docs/b.md", "line": 1, "message": "still wrong"}]},
            "rule_docs": true}"#,
    );
    let runlog = p.path().join("runlog");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success()
        .stdout(predicate::str::contains("still wrong").not())
        .stdout(predicate::str::contains("2 rules: 2 passed"));

    let calls = runlog_calls(&runlog);
    assert_eq!(
        calls.len(),
        2,
        "one original call + one bounded rework: {calls:?}"
    );
}

#[test]
fn file_scoped_ignore_excludes_the_file_so_the_rule_is_not_judged() {
    // A file-top `ignore-file` is honored by the *planner*: when it covers a
    // rule's only file, the rule is dropped from the run entirely — no judge call
    // (nothing left to review), reported as *ignored* rather than
    // judged-then-suppressed. The mock verdict is never consulted.
    let p = ignore_project(
        "/* llmlint: ignore-file[no_todo] vendored, reviewed upstream */\n// TODO: later\n",
    );
    let verdicts = p.write_verdicts(
        r#"{"no_todo": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 2, "message": "stray TODO"}]}}"#,
    );
    let runlog = p.path().join("runlog");
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .success()
        .stdout(predicate::str::contains("stray TODO").not())
        .stdout(predicate::str::contains(
            "1 rules: 0 passed, 0 failed, 0 skipped, 1 ignored",
        ));
    // The file-ignored rule never reached oneharness — the runlog dir, which the
    // mock creates only on invocation, does not exist.
    assert!(
        !runlog.exists(),
        "the fully-ignored rule must not be judged (no oneharness call)"
    );
}

#[test]
fn file_scoped_ignore_narrows_scope_but_keeps_the_rule_for_its_other_files() {
    // A rule spanning two files, one `ignore-file`d: the ignored file is dropped
    // from the rule's scope, but the rule is still judged over its other file.
    // A violation the judge nonetheless pins to the ignored file is discarded.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: no_todo, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n"
        ),
    );
    p.write(
        "src/vendored.rs",
        "/* llmlint: ignore-file[no_todo] vendored */\n// TODO: later\n",
    );
    p.write("src/app.rs", "// TODO: real\n");
    let verdicts = p.write_verdicts(
        r#"{"no_todo": {"holds": false, "violations": [
                {"file": "src/vendored.rs", "line": 2, "message": "ignored TODO"},
                {"file": "src/app.rs", "line": 1, "message": "real TODO"}]}}"#,
    );
    let dump = p.path().join("system.txt");
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("src/app.rs:1: real TODO"))
        .stdout(predicate::str::contains("ignored TODO").not());
    // The ignored file is not even presented to the judge (excluded from the union).
    let system = std::fs::read_to_string(&dump).unwrap();
    assert!(
        !system.contains("src/vendored.rs"),
        "the ignore-file'd file must be excluded from the prompt:\n{system}"
    );
    assert!(system.contains("src/app.rs"), "system:\n{system}");
}

#[test]
fn plan_only_prints_the_batching_and_makes_no_judge_call() {
    // `--plan-only` explains how the runs would batch and exits — needing no
    // harness at all (run via `bare`, with no `--oneharness-bin`) and writing no
    // history record.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: rule_a, description: \"{RULE}\" }}\n  \
             - {{ name: rule_b, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.bare()
        .arg("--plan-only")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Plan: 1 judge call(s) across 1 agent(s)",
        ))
        .stdout(predicate::str::contains("batch 1: [rule_a, rule_b]"))
        .stdout(predicate::str::contains("src/lib.rs"));
    // A dry inspection logs nothing.
    assert_eq!(history_record_count(&p), 0);
}

#[test]
fn plan_only_shows_excluded_files_and_ignored_rules() {
    // A file every declaring rule `ignore-file`s is shown as excluded; a rule whose
    // every file is ignored is listed under "not judged".
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: keep, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: gone, description: \"{RULE}\", files: {{ include: [\"vendor/**\"] }} }}\n"
        ),
    );
    p.write(
        "src/gen.rs",
        "// llmlint: ignore-file[keep] generated\n// code\n",
    );
    p.write("src/app.rs", "// code\n");
    p.write(
        "vendor/x.rs",
        "/* llmlint: ignore-file[gone] vendored */\n// code\n",
    );
    p.bare()
        .arg("--plan-only")
        .assert()
        .success()
        .stdout(predicate::str::contains("excluded src/gen.rs"))
        .stdout(predicate::str::contains("not judged:"))
        .stdout(predicate::str::contains(
            "gone — all matching files ignored (ignore-file)",
        ));
}

#[test]
fn plan_only_groups_shared_scopes_and_reports_the_saving() {
    // Four rules over two scopes, interleaved so order-based chunking (batch_size 2)
    // would split each scope across both batches. Affinity groups by shared file,
    // and `--plan-only` reports the reuse and the counterfactual.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nagents:\n  grp: {{ batch_size: 2 }}\nrules:\n  \
             - {{ name: a, description: \"{RULE}\", agent: grp, files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: c, description: \"{RULE}\", agent: grp, files: {{ include: [\"docs/**\"] }} }}\n  \
             - {{ name: b, description: \"{RULE}\", agent: grp, files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: d, description: \"{RULE}\", agent: grp, files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write("docs/x.md", "# doc\n");
    p.bare()
        .arg("--plan-only")
        .assert()
        .success()
        .stdout(predicate::str::contains("grouped: shares src/lib.rs"))
        .stdout(predicate::str::contains("batching: ~"))
        .stdout(predicate::str::contains("per-rule exposure"));
}

#[test]
fn verbose_report_appends_the_plan_section() {
    // At `-v` the human report carries the plan explanation so a reader can see how
    // the run was batched.
    let p = ignore_project("// code\n");
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    p.lint_v()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success()
        .stdout(predicate::str::contains("Plan: 1 judge call(s)"))
        .stdout(predicate::str::contains("batch 1: [no_todo]"));
}

#[test]
fn json_report_carries_the_plan_and_ignored_count() {
    // `--format json` exposes the ignored count and the structured plan, so tooling
    // and the history record both explain the batching from one source.
    let p = ignore_project("/* llmlint: ignore-file[no_todo] vendored */\n// code\n");
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    let out = p
        .lint()
        .arg("--format")
        .arg("json")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["ignored"], 1);
    let skipped = v["plan"]["skipped"].as_array().unwrap();
    assert!(
        skipped
            .iter()
            .any(|s| s["rule"] == "no_todo" && s["reason"] == "all_files_ignored"),
        "plan.skipped should record the ignored rule: {v:#}"
    );
}

#[test]
fn diff_omits_change_runs_wholly_ignored_by_a_block() {
    // Under `--diff`, a contiguous run of changed lines fully covered by an
    // `ignore-block` (for the only applicable rule) is replaced with a marker in the
    // prompt, while an adjacent non-ignored change is kept in full.
    let p = ignore_project("// header\n// middle\n// footer\n");
    // Commit the baseline so the diff is against a real HEAD.
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    // New content: an ignored block (added lines 2–4) separated by the unchanged
    // `// middle` (line 5) from a non-ignored change (`// TODO real`, line 6).
    p.write(
        "src/lib.rs",
        "// header\n\
         // llmlint: ignore-block[no_todo] legacy region, tracked in JIRA-1\n\
         // TODO ignored\n\
         // llmlint: ignore-end[no_todo]\n\
         // middle\n\
         // TODO real\n\
         // footer\n",
    );
    let verdicts = p.write_verdicts(r#"{"no_todo": true}"#);
    let dump = p.path().join("system.txt");
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .arg("--diff")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();
    let system = std::fs::read_to_string(&dump).unwrap();
    // The wholly-ignored run is replaced by an honest marker…
    assert!(
        system.contains("omitted — ignored for all applicable rules"),
        "system:\n{system}"
    );
    assert!(!system.contains("// TODO ignored"), "system:\n{system}");
    // …while the adjacent, non-ignored change is shown verbatim.
    assert!(system.contains("+// TODO real"), "system:\n{system}");
}

#[test]
fn line_scoped_ignore_suppresses_only_the_covered_line() {
    // A line directive covers its own line and the one below it; a violation there
    // is dropped, but a violation on an uncovered line still fails.
    let p = ignore_project(
        "// llmlint: ignore[no_todo] the line below is exempt\nbad two\nbad three\n",
    );
    let verdicts = p.write_verdicts(
        r#"{"no_todo": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 2, "message": "ignored todo"},
                {"file": "src/lib.rs", "line": 3, "message": "live todo"}]}}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("src/lib.rs:3: live todo"))
        .stdout(predicate::str::contains("ignored todo").not());
}

#[test]
fn block_scoped_ignore_suppresses_violations_inside_the_block() {
    // A block covers every line from open to close; a violation inside is dropped,
    // one outside still fails.
    let p = ignore_project(
        "// llmlint: ignore-block[no_todo] legacy region, tracked in JIRA-9\n\
         bad two\n\
         // llmlint: ignore-end[no_todo]\n\
         bad four\n",
    );
    let verdicts = p.write_verdicts(
        r#"{"no_todo": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 2, "message": "inside block"},
                {"file": "src/lib.rs", "line": 4, "message": "outside block"}]}}"#,
    );
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("src/lib.rs:4: outside block"))
        .stdout(predicate::str::contains("inside block").not());
}

#[test]
fn per_file_applicability_and_diff_compose_in_one_prompt() {
    // The per-file applicability context and the `--diff` changed-lines block are
    // independent sections that must coexist: a merged call over two distinct
    // file scopes shows both the per-file rule lists and the diff of the changed
    // file (and only the changed file).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// before\n");
    p.write("docs/b.md", "# doc\n");
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    // Change only src/a.rs, so the diff block carries it and docs/b.md does not.
    p.write("src/a.rs", "// after the change\n");

    let verdicts = p.write_verdicts(r#"{"rule_src": true, "rule_docs": true}"#);
    let dump = p.path().join("system.txt");
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .arg("--diff")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    // Per-file applicability is present for both files.
    assert!(
        system.contains("src/a.rs — only these rules apply: rule_src"),
        "system:\n{system}"
    );
    assert!(
        system.contains("docs/b.md — all rules apply except: rule_src"),
        "system:\n{system}"
    );
    // The diff is inlined under src/a.rs (the only changed file).
    assert!(system.contains("```diff"), "system:\n{system}");
    assert!(
        system.contains("diff --git a/src/a.rs"),
        "system:\n{system}"
    );
    assert!(system.contains("+// after the change"), "system:\n{system}");
    assert!(
        !system.contains("diff --git a/docs/b.md"),
        "unchanged file has no diff:\n{system}"
    );
}

#[test]
fn rework_prompt_lists_the_correct_rules_for_the_wrong_file() {
    // The corrective re-ask must actually reach oneharness — naming the wrong-file
    // violation and restating which rules apply to each file (the validation's
    // whole point), not just happening to run a second time.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// code\n");
    p.write("docs/b.md", "# doc\n");
    // First call: rule_src wrongly flags docs/b.md (out of its scope). Rework: holds.
    let verdicts = p.write_verdicts(
        r#"{"rule_src": [{"holds": false, "violations": [
                {"file": "docs/b.md", "line": 1, "message": "wrong file"}]}, true],
            "rule_docs": true}"#,
    );
    let state = p.path().join("state");
    let args = p.path().join("args.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .env("LLMLINT_MOCK_DUMP_ARGS", &args)
        .assert()
        .success();

    // DUMP_ARGS keeps the LAST call's args — the rework — whose `--prompt` names
    // the offending (rule, file) and the rules that apply to each file.
    let dumped = fs::read_to_string(&args).unwrap();
    assert!(
        dumped.contains("`rule_src` reported a violation in `docs/b.md`"),
        "rework prompt missing the wrong-file callout:\n{dumped}"
    );
    assert!(dumped.contains("does not apply to"), "args:\n{dumped}");
    assert!(
        dumped.contains("src/a.rs — only these rules apply: rule_src"),
        "rework prompt missing the per-file rule scope:\n{dumped}"
    );
}

#[test]
fn cross_cutting_violation_without_a_file_survives_scope_filtering() {
    // A violation with no `file` can't be mislocated, so the scope filter keeps it
    // (we never over-drop a legitimate cross-cutting finding): the rule still fails,
    // and no rework is triggered (there is no wrong file to correct).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nrules:\n  \
             - {{ name: rule_src, description: \"{RULE}\", files: {{ include: [\"src/**\"] }} }}\n  \
             - {{ name: rule_docs, description: \"{RULE}\", files: {{ include: [\"docs/**\"] }} }}\n"
        ),
    );
    p.write("src/a.rs", "// code\n");
    p.write("docs/b.md", "# doc\n");
    let verdicts = p.write_verdicts(
        r#"{"rule_src": {"holds": false, "violations": [{"message": "cross-cutting drift"}]},
            "rule_docs": true}"#,
    );
    let runlog = p.path().join("runlog");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_RUNLOG", &runlog)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL rule_src"))
        .stdout(predicate::str::contains("cross-cutting drift"));

    // No rework: a file-less violation is never a wrong-file problem.
    assert_eq!(
        runlog_calls(&runlog).len(),
        1,
        "a file-less violation must not trigger a rework"
    );
}

#[test]
fn per_file_context_says_all_rules_apply_when_every_rule_covers_a_file() {
    // When every rule in the call covers a file, the skip-list is empty and the
    // cheapest spelling is the bare "all rules apply" (the exclude-empty branch).
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: rule_a, description: \"{RULE}\" }}\n  \
             - {{ name: rule_b, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"rule_a": true, "rule_b": true}"#);
    let dump = p.path().join("system.txt");

    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_DUMP", &dump)
        .assert()
        .success();

    let system = fs::read_to_string(&dump).unwrap();
    assert!(
        system.contains("src/lib.rs — all rules apply\n"),
        "system:\n{system}"
    );
}

// ---- results logging + `history` command ---------------------------------

/// A project with a passing, a failing (located), and a skipped rule — the
/// shape the history journeys inspect. Returns the verdicts path.
fn history_project() -> (Project, PathBuf) {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: ok_rule, description: \"{RULE}\" }}\n  \
             - {{ name: bad_rule, description: \"{RULE}\" }}\n  \
             - {{ name: no_files, description: \"{RULE}\", files: {{ include: [\"none/**\"] }} }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(
        r#"{"ok_rule": true,
            "bad_rule": {"holds": false, "violations": [
                {"file": "src/lib.rs", "line": 7, "message": "inline SQL"}]}}"#,
    );
    (p, verdicts)
}

/// Count the JSON records written under a project's isolated history dir.
fn history_record_count(p: &Project) -> usize {
    match fs::read_dir(p.history_dir()) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count(),
        Err(_) => 0,
    }
}

#[test]
fn a_run_is_logged_and_can_be_listed_and_shown() {
    // Default-on: a lint run writes one record, prints the run id + how to fetch
    // it on stderr (stdout stays the clean report), and `history` reads it back.
    let (p, verdicts) = history_project();

    let out = p
        .lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // The report is on stdout, unchanged; the history note is on stderr only.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL bad_rule"));
    assert!(
        !stdout.contains("llmlint history"),
        "note must not touch stdout"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("See full results with `llmlint history"),
        "stderr:\n{stderr}"
    );
    // Exactly one record landed in the isolated store.
    assert_eq!(history_record_count(&p), 1);

    // `history` (no id) lists the run with a terse summary.
    p.bare()
        .arg("history")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "3 rules: 1 passed, 1 failed, 1 skipped",
        ));

    // `history latest` shows the full results — including the located violation
    // and the skipped rule the terminal report omitted.
    p.bare()
        .arg("history")
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains("command: lint"))
        .stdout(predicate::str::contains("PASS ok_rule"))
        .stdout(predicate::str::contains("FAIL bad_rule"))
        .stdout(predicate::str::contains("src/lib.rs:7: inline SQL"))
        .stdout(predicate::str::contains("SKIP no_files"));
}

#[test]
fn history_path_and_json_extract_the_record() {
    let (p, verdicts) = history_project();
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1);

    // `--path` prints just the record file path, which exists and is under the
    // history dir.
    let out = p
        .bare()
        .arg("history")
        .arg("latest")
        .arg("--path")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(Path::new(&path).is_file(), "path should exist: {path}");
    assert!(path.ends_with(".json"));

    // `--format json` emits the raw record: metadata + the full rules array.
    let out = p
        .bare()
        .arg("history")
        .arg("latest")
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "lint");
    assert_eq!(v["exit_code"], 1);
    assert_eq!(v["summary"]["failed"], 1);
    let names: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ok_rule") && names.contains(&"bad_rule"));

    // Listing as JSON is an array of run summaries carrying the record path.
    let out = p
        .bare()
        .arg("history")
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();
    let arr: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(arr.as_array().unwrap().len(), 1);
    assert_eq!(arr[0]["command"], "lint");
    assert!(arr[0]["path"].as_str().unwrap().ends_with(".json"));
}

#[test]
fn history_filters_by_status_and_rule() {
    let (p, verdicts) = history_project();
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1);

    // `--status fail` narrows to the failing rule only.
    p.bare()
        .arg("history")
        .arg("latest")
        .arg("--status")
        .arg("fail")
        .assert()
        .success()
        .stdout(predicate::str::contains("FAIL bad_rule"))
        .stdout(predicate::str::contains("ok_rule").not())
        .stdout(predicate::str::contains("no_files").not());

    // `--rule ok_rule` narrows to that named rule only.
    p.bare()
        .arg("history")
        .arg("latest")
        .arg("--rule")
        .arg("ok_rule")
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS ok_rule"))
        .stdout(predicate::str::contains("bad_rule").not());

    // An unknown status is a clear exit-2 error listing the valid ones.
    p.bare()
        .arg("history")
        .arg("latest")
        .arg("--status")
        .arg("bogus")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown --status"))
        .stderr(predicate::str::contains("valid statuses"));

    // A filter with no id is rejected (filters need a single run).
    p.bare()
        .arg("history")
        .arg("--status")
        .arg("fail")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("pass a run id"));
}

#[test]
fn history_can_be_disabled_via_config() {
    // `history.enabled: false` turns logging off: no record, no stderr note, and
    // the `history` listing reports an empty store.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nhistory:\n  enabled: false\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true}"#);

    let out = p
        .lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("llmlint history"));
    assert_eq!(history_record_count(&p), 0);

    p.bare()
        .arg("history")
        .assert()
        .success()
        .stdout(predicate::str::contains("No runs recorded"));
}

#[test]
fn no_history_flag_suppresses_logging_for_one_run() {
    let (p, verdicts) = history_project();
    p.lint()
        .arg("--no-history")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("llmlint history").not());
    assert_eq!(history_record_count(&p), 0);
}

#[test]
fn history_prunes_to_max_runs() {
    // `history.max_runs: 2` keeps only the two most recent records across runs.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nhistory:\n  max_runs: 2\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true}"#);
    for _ in 0..3 {
        p.lint()
            .env("LLMLINT_MOCK_VERDICTS", &verdicts)
            .assert()
            .success();
    }
    assert_eq!(history_record_count(&p), 2, "only the last 2 runs are kept");
}

#[test]
fn history_max_runs_zero_is_a_config_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nhistory:\n  max_runs: 0\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("history.max_runs is 0"));
}

#[test]
fn history_unknown_id_is_a_clear_error() {
    let (p, verdicts) = history_project();
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1);
    p.bare()
        .arg("history")
        .arg("nonexistent-id")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no run with id"));
}

#[test]
fn lint_config_run_is_recorded_with_its_command() {
    // The `lint-config` engine logs too, tagged with its own command name.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nrules:\n  - {{ name: public_items_are_documented, description: \"{RULE}\" }}\n"),
    );
    p.lint_config().assert().success();
    let out = p
        .bare()
        .arg("history")
        .arg("latest")
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "lint-config");
}

#[test]
fn history_records_and_filters_an_ignored_rule() {
    // A rule whose only file is `ignore-file`d is recorded as ignored: the listing
    // summary counts it, `history latest` labels it IGN, and `--status ignored`
    // narrows to it.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: normal, description: \"{RULE}\" }}\n  \
             - {{ name: vendored, description: \"{RULE}\", files: {{ include: [\"vendor/**\"] }} }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    p.write(
        "vendor/x.rs",
        "/* llmlint: ignore-file[vendored] third-party */\n// code\n",
    );
    let verdicts = p.write_verdicts(r#"{"normal": true}"#);
    p.lint()
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .success();

    p.bare()
        .arg("history")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 ignored"));
    p.bare()
        .arg("history")
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains("IGN  vendored"));
    p.bare()
        .arg("history")
        .arg("latest")
        .arg("--status")
        .arg("ignored")
        .assert()
        .success()
        .stdout(predicate::str::contains("IGN  vendored"))
        .stdout(predicate::str::contains("normal").not());
}

#[test]
fn history_explicit_dir_and_path_listing() {
    // A run logs into an explicit `--dir`, and `history --dir … --path` (no id)
    // prints that directory — the scripting hook for locating the store.
    let (p, verdicts) = history_project();
    let store = p.path().join("my-history");
    p.lint()
        .env("LLMLINT_HISTORY_DIR", &store)
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1);

    // `--dir` overrides the env; `--path` with no id prints the directory.
    p.bare()
        .arg("history")
        .arg("--dir")
        .arg(&store)
        .arg("--path")
        .assert()
        .success()
        .stdout(predicate::str::contains(store.display().to_string()));

    // Listing that explicit dir shows the run.
    p.bare()
        .arg("history")
        .arg("--dir")
        .arg(&store)
        .arg("--limit")
        .arg("5")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 passed, 1 failed"));
}

#[test]
fn history_env_off_switch_matches_the_flag() {
    // `LLMLINT_NO_HISTORY=1` disables logging for a run, like `--no-history`.
    let (p, verdicts) = history_project();
    p.lint()
        .env("LLMLINT_NO_HISTORY", "1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("llmlint history").not());
    assert_eq!(history_record_count(&p), 0);
}

#[test]
fn history_latest_on_empty_store_is_a_clear_error() {
    // Nothing recorded yet: `history latest` is a clear exit-2 error, while a bare
    // `history` listing reports the empty store (exit 0).
    let p = Project::new();
    p.bare()
        .arg("history")
        .arg("latest")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no runs recorded"));
    p.bare()
        .arg("history")
        .assert()
        .success()
        .stdout(predicate::str::contains("No runs recorded"));
}

#[test]
fn history_records_a_run_error_with_its_errors_and_exit_code() {
    // A run that could not complete (oneharness produced no structured output) is
    // still logged: the record carries exit_code 2 and the non-empty errors array,
    // so a failed run is inspectable after the fact.
    let (p, _verdicts) = history_project();
    p.lint()
        .env("LLMLINT_MOCK_NO_STRUCTURED", "1")
        .assert()
        .code(2);
    let out = p
        .bare()
        .arg("history")
        .arg("latest")
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["exit_code"], 2);
    assert!(!v["errors"].as_array().unwrap().is_empty());
    // The human view surfaces the ERROR line too.
    p.bare()
        .arg("history")
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains("exit: 2"))
        .stdout(predicate::str::contains("ERROR"));
}

#[test]
fn history_shows_multi_judge_breakdown() {
    // A multi-judge rule's per-judge results + rationales are recorded and shown
    // by `history` — the disagreement the terminal report collapses is retrievable.
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
        r#"{"voted_rule": [
            {"holds": false, "rationale": "raw SQL"},
            {"holds": true, "rationale": "query layer"},
            {"holds": false, "rationale": "string built"}
        ]}"#,
    );
    let state = p.path().join("state");
    p.lint()
        .arg("--max-parallel")
        .arg("1")
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", &state)
        .assert()
        .code(1);

    p.bare()
        .arg("history")
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "FAIL voted_rule (1/3 judges held)",
        ))
        .stdout(predicate::str::contains("judge 1 violated: raw SQL"))
        .stdout(predicate::str::contains("judge 2 held: query layer"))
        .stdout(predicate::str::contains("judge 3 violated: string built"));
}

#[test]
fn history_limit_truncates_the_listing() {
    // Three runs, `--limit 2` shows exactly the two most recent lines.
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n  \
             - {{ name: a_rule, description: \"{RULE}\" }}\n"
        ),
    );
    p.write("src/lib.rs", "// code\n");
    let verdicts = p.write_verdicts(r#"{"a_rule": true}"#);
    for _ in 0..3 {
        p.lint()
            .env("LLMLINT_MOCK_VERDICTS", &verdicts)
            .assert()
            .success();
    }
    let out = p
        .bare()
        .arg("history")
        .arg("--limit")
        .arg("2")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let lines = String::from_utf8_lossy(&out.stdout);
    let n = lines.lines().filter(|l| l.contains("rules:")).count();
    assert_eq!(n, 2, "expected 2 listed runs, got:\n{lines}");
    // JSON listing respects the limit identically.
    let out = p
        .bare()
        .arg("history")
        .arg("--format")
        .arg("json")
        .arg("--limit")
        .arg("2")
        .output()
        .unwrap();
    let arr: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(arr.as_array().unwrap().len(), 2);
}
