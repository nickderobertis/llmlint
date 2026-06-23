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
//! - `LLMLINT_MOCK_GARBAGE=1` — print non-JSON to stdout (unparseable output).

use std::env;
use std::fs;
use std::path::PathBuf;

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

    // `llmlint doctor` calls `<bin> --version`.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("oneharness 0.2.529 (mock)");
        return;
    }

    let harness = arg_value(&args, "--harness").unwrap_or_else(|| "claude-code".into());

    // Optionally record the rendered system prompt so the e2e suite can assert
    // on which files/rules reached the judge (file globbing + template render).
    if let Some(dump) = env::var_os("LLMLINT_MOCK_DUMP") {
        if let Some(system) = arg_value(&args, "--system") {
            let _ = fs::write(PathBuf::from(dump), system);
        }
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
        let schema_path = arg_value(&args, "--schema").unwrap_or_default();
        let verdicts: Map<String, Value> = env::var("LLMLINT_MOCK_VERDICTS")
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut structured = Map::new();
        for rule in rule_names(&schema_path) {
            let v = verdict_for(&rule, &verdicts);
            structured.insert(rule, v);
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

    let report = json!({
        "schema_version": "0.1",
        "oneharness_version": "mock",
        "results": [result],
    });
    println!("{}", serde_json::to_string(&report).unwrap());

    // A real run exits 0 on ok/skipped, 1 otherwise; mirror that loosely.
    let ok = !(flag("LLMLINT_MOCK_FAIL_SCHEMA") || flag("LLMLINT_MOCK_NO_STRUCTURED"));
    std::process::exit(if ok { 0 } else { 1 });
}
