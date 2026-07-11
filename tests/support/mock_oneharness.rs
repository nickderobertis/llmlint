//! A deterministic stand-in for `oneharness run`, used by the e2e suite via
//! llmlint's `--oneharness-bin` override. It is the genuinely-external boundary
//! in llmlint, so mocking *it* (not llmlint's own logic) keeps e2e hermetic and
//! cross-platform — the same pattern oneharness uses for the real agent CLIs.
//!
//! It reads the `--schema` file to learn which rule names to answer, then emits
//! a real oneharness-shaped JSON report whose `results[0].structured` is driven
//! by fixture env vars:
//!
//! - `LLMLINT_MOCK_VERDICTS=<path>` — JSON map `rule -> spec`, where a spec is a
//!   bool (`holds`), an object (`{holds, violations}`), or an array of specs
//!   (one per judge call, advanced via a per-rule counter under
//!   `LLMLINT_MOCK_STATE`). Unlisted rules default to `holds=true`.
//! - `LLMLINT_MOCK_FAIL_SCHEMA=1` — emit `schema_valid=false` (validation fail).
//! - `LLMLINT_MOCK_NO_STRUCTURED=1` — emit `structured=null` + a non-ok status
//!   (the shape oneharness returns on a timeout / nonzero run).
//! - `LLMLINT_MOCK_FALLBACK=1` — emit a **fallback**-shaped report: a skipped
//!   `codex` entry listed first in `results`, the real verdict from the harness
//!   fell through to, and a top-level `fallback.ran` naming that winner (the
//!   issue-#146 scenario — llmlint must read the winner, not `results[0]`).
//!   Combined with `LLMLINT_MOCK_NO_STRUCTURED`, the fell-through harness also
//!   fails, so `ran` is null (the whole chain failed). Combines with
//!   `LLMLINT_MOCK_VERDICTS` so the winner's verdict content flows through.
//! - `LLMLINT_MOCK_FALLBACK_NO_RAN=1` — with `LLMLINT_MOCK_FALLBACK`, omit the
//!   `fallback.ran` name even on success, so llmlint must fall back to the first
//!   `results` entry that produced structured output.
//! - `LLMLINT_MOCK_VERSION=<v>` — the version string reported by `--version`
//!   (default `0.3.12`), so a test can drive llmlint's minimum-version gate.
//! - `LLMLINT_MOCK_GARBAGE=1` — print non-JSON to stdout (unparseable output).
//! - `LLMLINT_MOCK_DUMP_ARGS=<path>` — record the full `run` arg vector (one arg
//!   per line) so a test can assert which flags llmlint did/did not pass.
//! - `LLMLINT_MOCK_DUMP_SCHEMA=<path>` — copy the generated `--schema` JSON so a
//!   test can assert its shape (per-rule `name`/`rationale`/`holds` ordering).
//! - `LLMLINT_MOCK_RUNLOG=<dir>` — record one file per invocation listing the
//!   rule names it judged (comma-joined), so a test can count invocations and
//!   assert how rules were batched into oneharness calls.
//! - `LLMLINT_MOCK_BARRIER=<dir>` (+ `LLMLINT_MOCK_BARRIER_N`, default 1, and
//!   `LLMLINT_MOCK_BARRIER_MS`, default 2000) — a rendezvous: each invocation
//!   registers itself and blocks until `N` peers are present, or fails like a
//!   judge that couldn't complete after the timeout. With `N` peers required, a
//!   clean exit proves `N` invocations ran concurrently (i.e. `--max-parallel`
//!   actually overlapped them); a serial wave never reaches `N` and times out.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

fn flag(name: &str) -> bool {
    env::var(name)
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Resolve the effective system prompt from `--system` (inline) or
/// `--system-file` (a path; `-` for stdin), mirroring real oneharness. llmlint
/// always passes it by file, so the file branch is the live path here.
fn resolve_system(args: &[String]) -> Option<String> {
    if let Some(text) = arg_value(args, "--system") {
        return Some(text);
    }
    let path = arg_value(args, "--system-file")?;
    if path == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).ok()?;
        Some(buf)
    } else {
        fs::read_to_string(path).ok()
    }
}

fn rule_names(schema_path: &str) -> Vec<String> {
    let text = fs::read_to_string(schema_path).unwrap_or_default();
    let schema: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Resolve a rule's verdict object from its fixture spec, advancing the
/// per-rule call counter when the spec is a sequence (array).
fn verdict_for(rule: &str, verdicts: &Map<String, Value>) -> Value {
    let spec = verdicts.get(rule).cloned().unwrap_or(Value::Bool(true));
    let chosen = match spec {
        Value::Array(seq) if !seq.is_empty() => {
            let idx = next_count(rule).min(seq.len() - 1);
            seq[idx].clone()
        }
        other => other,
    };
    match chosen {
        Value::Bool(b) => json!({ "holds": b }),
        Value::Object(_) => chosen,
        _ => json!({ "holds": true }),
    }
}

/// Atomically claim the next free `{prefix}-{i}` file in `dir`. `create_new`
/// (O_EXCL) makes concurrent claimers pick distinct indices, so the file count
/// is an exact invocation count even under parallelism.
fn claim_indexed(dir: &Path, prefix: &str) -> PathBuf {
    let _ = fs::create_dir_all(dir);
    let mut i = 0usize;
    loop {
        let p = dir.join(format!("{prefix}-{i}"));
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return p,
            Err(_) => i += 1,
        }
    }
}

/// Rendezvous on `LLMLINT_MOCK_BARRIER`: register, then block until `N` peers
/// are present (concurrency proof) or the timeout elapses. Returns `false` on
/// timeout so the caller can fail like a judge that couldn't complete.
fn barrier_ok() -> bool {
    let Some(dir) = env::var_os("LLMLINT_MOCK_BARRIER") else {
        return true;
    };
    let dir = PathBuf::from(dir);
    let n: usize = env::var("LLMLINT_MOCK_BARRIER_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let timeout = Duration::from_millis(
        env::var("LLMLINT_MOCK_BARRIER_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000),
    );
    claim_indexed(&dir, "peer");
    let start = Instant::now();
    loop {
        let present = fs::read_dir(&dir).map(|rd| rd.count()).unwrap_or(0);
        if present >= n {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Print a oneharness report whose single result carries no structured output
/// (the shape returned on a timeout / nonzero run).
fn emit_no_structured(harness: &str) {
    let result = json!({
        "harness": harness,
        "status": "timeout",
        "exit_code": null,
        "structured": null,
        "schema_valid": null,
        "schema_attempts": null,
        "schema_error": null,
        "error": "mock: barrier timed out (ran alone in its wave)",
    });
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "results": [result],
        }))
        .unwrap()
    );
}

fn next_count(rule: &str) -> usize {
    let Some(dir) = env::var_os("LLMLINT_MOCK_STATE") else {
        return 0;
    };
    let dir = PathBuf::from(dir);
    let _ = fs::create_dir_all(&dir);
    let file = dir.join(format!("{rule}.count"));
    let current: usize = fs::read_to_string(&file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = fs::write(&file, (current + 1).to_string());
    current
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // `llmlint doctor` (and lint's pre-flight version gate) call `<bin>
    // --version`. Default to a version that satisfies llmlint's minimum;
    // `LLMLINT_MOCK_VERSION` overrides it so a test can drive the too-old path.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let version = env::var("LLMLINT_MOCK_VERSION").unwrap_or_else(|_| "0.3.12".into());
        println!("oneharness {version} (mock)");
        return;
    }

    let harness = arg_value(&args, "--harness").unwrap_or_else(|| "claude-code".into());

    // Optionally record the raw arg vector so a test can assert which flags
    // llmlint passed (e.g. that `--harness` is omitted when not configured).
    if let Some(dump) = env::var_os("LLMLINT_MOCK_DUMP_ARGS") {
        let _ = fs::write(PathBuf::from(dump), args[1..].join("\n"));
    }

    // Optionally record the rendered system prompt so the e2e suite can assert
    // on which files/rules reached the judge (file globbing + template render).
    // llmlint passes the system prompt by file (`--system-file <path>`), so read
    // it back the way real oneharness does.
    if let Some(dump) = env::var_os("LLMLINT_MOCK_DUMP") {
        if let Some(system) = resolve_system(&args) {
            let _ = fs::write(PathBuf::from(dump), system);
        }
    }

    let schema_path = arg_value(&args, "--schema").unwrap_or_default();
    let names = rule_names(&schema_path);

    // Optionally copy the generated `--schema` JSON so a test can assert its
    // shape (e.g. that each rule requires/orders `name`, `rationale`, `holds`).
    // The real schema file is a tempfile llmlint deletes after the run.
    if let Some(dump) = env::var_os("LLMLINT_MOCK_DUMP_SCHEMA") {
        if let Ok(text) = fs::read_to_string(&schema_path) {
            let _ = fs::write(PathBuf::from(dump), text);
        }
    }

    // Optionally record one file per invocation listing the rules it judged, so
    // a test can count oneharness calls and assert how rules were batched.
    if let Some(dir) = env::var_os("LLMLINT_MOCK_RUNLOG") {
        let path = claim_indexed(&PathBuf::from(dir), "run");
        let _ = fs::write(&path, names.join(","));
    }

    // Optionally rendezvous with concurrent invocations to prove `--max-parallel`
    // actually overlapped them. A timeout means this judge ran alone in its wave
    // -> behave like one that couldn't complete (no structured output).
    if !barrier_ok() {
        emit_no_structured(&harness);
        std::process::exit(1);
    }

    if flag("LLMLINT_MOCK_GARBAGE") {
        println!("this is not json");
        return;
    }

    if flag("LLMLINT_MOCK_EMPTY_RESULTS") {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "schema_version": "0.1", "oneharness_version": "mock", "results": []
            }))
            .unwrap()
        );
        return;
    }

    let result = if flag("LLMLINT_MOCK_BAD_SHAPE") {
        // Valid JSON, valid against no schema check here, but not a verdict map.
        json!({
            "harness": harness,
            "status": "ok",
            "exit_code": 0,
            "structured": { "some_rule": "this should be an object, not a string" },
            "schema_valid": true,
            "schema_attempts": 1,
            "schema_error": null,
            "error": null,
        })
    } else if flag("LLMLINT_MOCK_FAIL_SCHEMA") {
        json!({
            "harness": harness,
            "status": "nonzero",
            "exit_code": 1,
            "structured": {},
            "schema_valid": false,
            "schema_attempts": 3,
            "schema_error": "mock: forced schema-validation failure",
            "error": null,
        })
    } else if flag("LLMLINT_MOCK_NO_STRUCTURED") {
        json!({
            "harness": harness,
            "status": env::var("LLMLINT_MOCK_STATUS").unwrap_or_else(|_| "timeout".into()),
            "exit_code": null,
            "structured": null,
            "schema_valid": null,
            "schema_attempts": null,
            "schema_error": null,
            "error": "mock: no structured output",
        })
    } else {
        let verdicts: Map<String, Value> = env::var("LLMLINT_MOCK_VERDICTS")
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut structured = Map::new();
        for rule in &names {
            let v = verdict_for(rule, &verdicts);
            structured.insert(rule.clone(), v);
        }
        json!({
            "harness": harness,
            "status": "ok",
            "exit_code": 0,
            "structured": Value::Object(structured),
            "schema_valid": true,
            "schema_attempts": 1,
            "schema_error": null,
            "error": null,
        })
    };

    // `LLMLINT_MOCK_FALLBACK=1` reproduces oneharness fallback mode (issue #146):
    // the primary harness (`codex`) is skipped as unavailable and listed *first*
    // in `results`, while the real verdict comes from the harness oneharness fell
    // through to (`result`, above). The top-level `fallback.ran` names that
    // winner. A correct llmlint reads the winner, not the skipped `results[0]`.
    let report = if flag("LLMLINT_MOCK_FALLBACK") {
        let winner = arg_value(&args, "--harness").unwrap_or_else(|| "claude-code".into());
        let skipped = json!({
            "harness": "codex",
            "status": "skipped",
            "available": false,
            "exit_code": null,
            "structured": null,
            "schema_valid": null,
            "schema_attempts": null,
            "schema_error": null,
            "error": "`codex` not found on PATH; harness skipped. Install it: npm install -g @openai/codex",
        });
        // `fallback.ran` names a harness only when one actually produced a
        // verdict; if the fell-through harness also failed (no structured output)
        // the whole chain failed and nothing ran. `LLMLINT_MOCK_FALLBACK_NO_RAN`
        // drops the name even on success, exercising llmlint's defensive
        // "first result that answered" path.
        let produced_output = result.get("structured").is_some_and(|s| !s.is_null());
        let ran = if produced_output && !flag("LLMLINT_MOCK_FALLBACK_NO_RAN") {
            Value::String(winner)
        } else {
            Value::Null
        };
        json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "fallback": {
                "ran": ran,
                "fell_through": [{ "harness": "codex", "reason": "not-installed" }],
            },
            "results": [skipped, result],
        })
    } else {
        json!({
            "schema_version": "0.1",
            "oneharness_version": "mock",
            "results": [result],
        })
    };
    println!("{}", serde_json::to_string(&report).unwrap());

    // A real run exits 0 on ok/skipped, 1 otherwise; mirror that loosely.
    let ok = !(flag("LLMLINT_MOCK_FAIL_SCHEMA") || flag("LLMLINT_MOCK_NO_STRUCTURED"));
    std::process::exit(if ok { 0 } else { 1 });
}
