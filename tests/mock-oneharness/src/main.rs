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
//! - `LLMLINT_MOCK_REPLIES=<path>` — a JSON array of `structured` values, one
//!   per `run` call in arrival order (the last repeating), emitted verbatim as a
//!   schema-valid reply — e.g. `{}`, or a reply naming only some rules.
//! - `LLMLINT_MOCK_FAIL_SCHEMA=1` — emit `schema_valid=false` (validation fail).
//! - `LLMLINT_MOCK_NO_STRUCTURED=1` — emit `structured=null` + a non-ok status
//!   (the shape oneharness returns on a timeout / nonzero run).
//! - `LLMLINT_MOCK_DEFERRED=1` — emit a `failure_kind="tool_deferred"` result
//!   with an actionable `error` and null `structured` (the shape oneharness
//!   0.3.21+ returns when a bridged/managed harness defers a builtin tool
//!   instead of executing it — issue #142). Drives llmlint's specific
//!   `ToolDeferred` diagnostic and the `doctor --probe` deferred path.
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
//!   (default: llmlint's `MIN_VERSION`), so a test can drive the minimum-version gate.
//! - `config [--cwd DIR] [--config PATH]... --format json` — the effective
//!   layered configuration, shaped like real `oneharness config`: the named files
//!   (else `$ONEHARNESS_CONFIG` under the nearest `oneharness.toml` /
//!   `.oneharness.toml` from `--cwd` up), each beneath its `extends` chain, then
//!   every `ONEHARNESS_<FIELD>` override, as `{value, source}` leaves beside
//!   `config_files`. An unreadable or invalid file exits 2 naming it.
//! - `run` honors `--format` like oneharness >= 0.14.0: `json` prints the JSON
//!   report (every branch below), while `text` or no `--format` at all prints a
//!   human text view, and any other value is refused with exit 2.
//! - `LLMLINT_MOCK_GARBAGE=1` — print non-JSON to stdout (unparseable output).
//! - `LLMLINT_MOCK_DUMP_ARGS=<path>` — record the full `run` arg vector (one arg
//!   per line) so a test can assert which flags llmlint did/did not pass.
//! - `LLMLINT_MOCK_PROMPTLOG=<dir>` — one JSON file per `run` call holding the
//!   `system` prompt and the user `prompt` it received, so a test can assert
//!   every prompt's size and contents (not only the last one, as `DUMP` keeps).
//! - `LLMLINT_MOCK_DUMP_SCHEMA=<path>` — copy the generated `--schema` JSON so a
//!   test can assert its shape (per-rule `name`/`rationale`/`holds` ordering).
//! - `LLMLINT_MOCK_RUNLOG=<dir>` — record one file per invocation listing the
//!   rule names it judged (comma-joined), so a test can count invocations and
//!   assert how rules were batched into oneharness calls.
//! - `LLMLINT_MOCK_SPAWNLOG=<dir>` — record one file per **process spawn** (the
//!   arg vector), written before anything else so it covers the `--version`
//!   pre-flight too. `RUNLOG`/`DUMP` only prove no *judge* ran; an empty spawn
//!   log proves llmlint never executed the harness binary at all — what a guard
//!   that must cost nothing needs to show.
//! - `LLMLINT_MOCK_BARRIER=<dir>` (+ `LLMLINT_MOCK_BARRIER_N`, default 1, and
//!   `LLMLINT_MOCK_BARRIER_MS`, default 2000) — a rendezvous: each invocation
//!   registers itself and blocks until `N` peers are present, or fails like a
//!   judge that couldn't complete after the timeout. With `N` peers required, a
//!   clean exit proves `N` invocations ran concurrently (i.e. `--max-parallel`
//!   actually overlapped them); a serial wave never reaches `N` and times out.

// llmlint: ignore-file[contracts_have_one_source_or_a_drift_gate] this is the wire double of an external tool, moved here verbatim from tests/support/mock_oneharness.rs; the wire it restates is owned by oneharness, not this repo, and its drift gate is the live tier (tests/live, live.yml on every pull request), which drives llmlint's same response parsing against the real released oneharness

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use llmlint::io::oneharness::MIN_VERSION;
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

/// The `structured` value scripted by `LLMLINT_MOCK_REPLIES` for this `run`
/// call, if any: the file holds a JSON array, and the n-th call (counted
/// atomically under `<path>.calls`, so concurrent judges each take a distinct
/// slot) gets element n, the last element repeating once the array runs out.
/// Emitted verbatim as a schema-valid reply, so a test can hand llmlint a reply
/// that answers fewer — or other — rules than the call selected.
fn scripted_reply() -> Option<Value> {
    let path = env::var("LLMLINT_MOCK_REPLIES").ok()?;
    let text = fs::read_to_string(&path).ok()?;
    let replies: Vec<Value> = serde_json::from_str(&text).ok()?;
    let last = replies.len().checked_sub(1)?;
    let slot = claim_indexed(&PathBuf::from(format!("{path}.calls")), "call");
    let n: usize = slot
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("call-"))
        .and_then(|i| i.parse().ok())
        .unwrap_or(0);
    Some(replies[n.min(last)].clone())
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

/// Every value of a repeatable `key` flag, in order.
fn arg_values(args: &[String], key: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == key)
        .map(|w| w[1].clone())
        .collect()
}

/// A TOML value as oneharness's `{value, source}` leaves: tables recurse, and
/// every other value becomes a leaf naming the file it came from.
fn annotate(value: toml::Value, source: &str) -> Value {
    match value {
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(k, v)| (k, annotate(v, source)))
                .collect(),
        ),
        other => json!({
            "value": serde_json::to_value(other).unwrap_or(Value::Null),
            "source": source,
        }),
    }
}

/// True for a `{value, source}` leaf (as opposed to a table of settings).
fn is_leaf(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == 2 && m.contains_key("value") && m.contains_key("source"))
}

/// Layer `over` onto `base`: tables merge key by key, and a leaf replaces.
fn merge_into(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) if !is_leaf(&Value::Object(o.clone())) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) if !is_leaf(existing) && !is_leaf(&v) => merge_into(existing, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// Load one config file beneath its `extends` chain (a parent path relative to
/// the file's directory), recording each file read. `Err` is the message
/// oneharness would refuse with.
fn load_layer(path: &Path, files: &mut Vec<String>, depth: usize) -> Result<Value, String> {
    if depth > 32 {
        return Err(format!("`extends` chain too deep at {}", path.display()));
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("reading config {}: {e}", path.display()))?;
    let mut table: toml::Table =
        toml::from_str(&text).map_err(|e| format!("parsing config {}: {e}", path.display()))?;
    let source = fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string();
    let mut merged = Value::Object(Map::new());
    if let Some(parent) = table.remove("extends") {
        let parent = parent
            .as_str()
            .ok_or_else(|| format!("`extends` in {} is not a path", path.display()))?;
        let parent = path.parent().unwrap_or(Path::new(".")).join(parent);
        merged = load_layer(&parent, files, depth + 1)?;
    }
    files.push(source.clone());
    merge_into(&mut merged, annotate(toml::Value::Table(table), &source));
    Ok(merged)
}

/// `config [--cwd DIR] [--config PATH]... --format json`: the named files (each
/// beneath its `extends` chain) or, with none named, `$ONEHARNESS_CONFIG` under
/// the nearest `oneharness.toml`/`.oneharness.toml` from `--cwd` up, then every
/// `ONEHARNESS_<FIELD>` override (`ONEHARNESS_HISTORY_LABELS` as `k=v` labels).
/// Prints what real oneharness prints — `config_files` plus a `{value, source}`
/// leaf per setting — and returns the exit code.
fn config_command(args: &[String]) -> i32 {
    if arg_value(args, "--format").as_deref() != Some("json") {
        eprintln!("mock: `config` answers only --format json");
        return 2;
    }
    let cwd = arg_value(args, "--cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap());
    let mut paths: Vec<PathBuf> = arg_values(args, "--config")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        if let Some(user) = env::var_os("ONEHARNESS_CONFIG").filter(|v| !v.is_empty()) {
            paths.push(PathBuf::from(user));
        }
        if let Some(project) = cwd.ancestors().find_map(|dir| {
            ["oneharness.toml", ".oneharness.toml"]
                .iter()
                .map(|name| dir.join(name))
                .find(|p| p.is_file())
        }) {
            paths.push(project);
        }
    }
    let mut files = Vec::new();
    let mut merged = Value::Object(Map::new());
    for path in &paths {
        match load_layer(path, &mut files, 0) {
            Ok(layer) => merge_into(&mut merged, layer),
            Err(message) => {
                eprintln!("error: {message}");
                return 2;
            }
        }
    }
    let mut from_env = false;
    for (key, value) in env::vars() {
        let Some(field) = key.strip_prefix("ONEHARNESS_") else {
            continue;
        };
        if matches!(field, "CONFIG" | "NO_CONFIG") || value.is_empty() {
            continue;
        }
        from_env = true;
        let field = field.to_lowercase();
        let leaf = if field == "history_labels" {
            Value::Object(
                value
                    .split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| {
                        (
                            k.trim().to_string(),
                            json!({"value": v.trim(), "source": "environment"}),
                        )
                    })
                    .collect(),
            )
        } else {
            json!({"value": value, "source": "environment"})
        };
        merge_into(&mut merged, json!({ field: leaf }));
    }
    if from_env {
        files.push("environment".into());
    }
    let mut report = Map::new();
    report.insert("schema_version".into(), json!("mock"));
    report.insert("config_files".into(), json!(files));
    if let Value::Object(fields) = merged {
        report.extend(fields);
    }
    println!("{}", serde_json::to_string(&Value::Object(report)).unwrap());
    0
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

    // Record the spawn itself, before any subcommand branching, so a test can
    // prove the binary was never executed — including the `--version` pre-flight,
    // which the run/judge-shaped recorders below never see.
    if let Some(dir) = env::var_os("LLMLINT_MOCK_SPAWNLOG") {
        let path = claim_indexed(&PathBuf::from(dir), "spawn");
        let _ = fs::write(&path, args[1..].join("\n"));
    }

    // `llmlint doctor` (and lint's pre-flight version gate) call `<bin>
    // --version`. Default to exactly llmlint's minimum (one source, so the
    // mock can never drift below the floor); `LLMLINT_MOCK_VERSION` overrides
    // it so a test can drive the too-old path.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        if let Some(dir) = env::var_os("LLMLINT_MOCK_DUMP_HISTORY_LABELS") {
            let value = env::var("ONEHARNESS_HISTORY_LABELS").unwrap_or_default();
            let _ = fs::write(PathBuf::from(dir).join("version"), value);
        }
        let version = env::var("LLMLINT_MOCK_VERSION").unwrap_or_else(|_| {
            let (major, minor, patch) = MIN_VERSION;
            format!("{major}.{minor}.{patch}")
        });
        println!("oneharness {version} (mock)");
        return;
    }

    // `oneharness config`: the effective layered configuration, as `llmlint
    // fingerprint` queries it. Never a judge call.
    if args.get(1).map(String::as_str) == Some("config") {
        std::process::exit(config_command(&args));
    }

    let harness = arg_value(&args, "--harness").unwrap_or_else(|| "claude-code".into());

    if let Some(dir) = env::var_os("LLMLINT_MOCK_DUMP_HISTORY_LABELS") {
        let value = env::var("ONEHARNESS_HISTORY_LABELS").unwrap_or_default();
        let _ = fs::write(PathBuf::from(dir).join("run"), value);
    }

    // Optionally record the raw arg vector so a test can assert which flags
    // llmlint passed (e.g. that `--harness` is omitted when not configured).
    if let Some(dump) = env::var_os("LLMLINT_MOCK_DUMP_ARGS") {
        let _ = fs::write(PathBuf::from(dump), args[1..].join("\n"));
    }

    // `--format` takes exactly `text` (oneharness's default: a human-readable
    // view) or `json` (the machine report every branch below prints). Mirror
    // that, so a run that stops asking for JSON gets text llmlint cannot parse
    // instead of silently keeping the old default. A bare trailing `--format`
    // is a usage error, as it is for oneharness's clap parser, not the default.
    let format = arg_value(&args, "--format");
    if format.is_none() && args.iter().any(|a| a == "--format") {
        eprintln!("error: a value is required for '--format <FORMAT>' but none was supplied");
        std::process::exit(2);
    }
    match format.as_deref() {
        Some("json") => {}
        None | Some("text") => {
            println!("{harness}: ok (mock text view; pass --format json for the report)");
            return;
        }
        Some(other) => {
            eprintln!(
                "error: invalid value '{other}' for '--format <FORMAT>' \
                 [possible values: text, json]"
            );
            std::process::exit(2);
        }
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

    // Optionally record every call's prompt — the system prompt and the user
    // prompt sent with it — one JSON file per `run` call, so a test can assert
    // the size of each prompt the harness received and what it carried.
    if let Some(dir) = env::var_os("LLMLINT_MOCK_PROMPTLOG") {
        let path = claim_indexed(&PathBuf::from(dir), "prompt");
        let record = json!({
            "system": resolve_system(&args).unwrap_or_default(),
            "prompt": arg_value(&args, "--prompt").unwrap_or_default(),
        });
        let _ = fs::write(&path, record.to_string());
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
    } else if flag("LLMLINT_MOCK_DEFERRED") {
        // oneharness >= 0.3.21: the harness exited cleanly but only deferred a
        // builtin tool, so `status` is ok yet there is no verdict and
        // `failure_kind` names the deferral. oneharness fails the run non-zero.
        json!({
            "harness": harness,
            "status": "ok",
            "exit_code": 0,
            "structured": null,
            "schema_valid": null,
            "schema_attempts": null,
            "schema_error": null,
            "failure_kind": "tool_deferred",
            "failure_kind_source": "claude-code",
            "error": "harness claude-code deferred a tool call (`Read`) instead of executing it; you appear to be in a bridged/managed deployment where builtin tools are deferred.",
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
    } else if let Some(reply) = scripted_reply() {
        json!({
            "harness": harness,
            "status": "ok",
            "exit_code": 0,
            "structured": reply,
            "schema_valid": true,
            "schema_attempts": 1,
            "schema_error": null,
            "error": null,
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
    let ok = !(flag("LLMLINT_MOCK_FAIL_SCHEMA")
        || flag("LLMLINT_MOCK_NO_STRUCTURED")
        || flag("LLMLINT_MOCK_DEFERRED"));
    std::process::exit(if ok { 0 } else { 1 });
}
