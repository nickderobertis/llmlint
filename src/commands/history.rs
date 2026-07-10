//! `llmlint history`: inspect the results logged by past runs.
//!
//! With no id it lists recent runs; with an id (or `latest`) it shows that run's
//! full results — the complete per-rule verdicts, votes, per-judge breakdown,
//! violations, and rationales the terminal report omits. `--status`/`--rule`
//! drill into part of a run, `--path` prints just the JSON record's location for
//! scripting, and `--format json` emits the raw record (or a JSON array when
//! listing).
//!
//! The history *directory* is resolved with the same precedence writing uses
//! (`--dir` > `LLMLINT_HISTORY_DIR` > config `history.dir` > platform default),
//! so inspection and logging always agree on where records live. Config loading
//! is best-effort — history is readable even outside a configured project.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::cli::{HistoryArgs, OutputFormat};
use crate::errors::{Error, Result};
use crate::io::{configfs, history};

/// Rule outcomes that `--status` accepts, matching the `outcome` field in a
/// stored record (see [`crate::domain::verdict::Outcome`]).
const VALID_STATUSES: &[&str] = &["pass", "fail", "skipped", "ignored", "not_relevant"];

pub fn run(args: HistoryArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };

    validate_filters(&args)?;

    let dir = resolve_dir(&args, &cwd)?;

    match &args.id {
        Some(id) => show(&dir, id, &args),
        None => list(&dir, &args),
    }
}

/// Resolve the history directory: the `--dir` override, else the same
/// config/env/default resolution logging uses. Config load is best-effort (a
/// missing or broken config falls back to env + platform default), since reading
/// history must work outside a configured project.
fn resolve_dir(args: &HistoryArgs, cwd: &Path) -> Result<PathBuf> {
    if let Some(dir) = &args.dir {
        return Ok(dir.clone());
    }
    let config = configfs::load(&[], cwd)
        .map(|l| l.config)
        .unwrap_or_default();
    // `resolve` reads the env + config + default; `force_off` is irrelevant to
    // where records live, so pass `false`.
    history::resolve(&config, false).dir.ok_or_else(|| {
        Error::History(
            "no history directory could be determined (set history.dir, \
             LLMLINT_HISTORY_DIR, or pass --dir)"
                .to_string(),
        )
    })
}

/// `--status`/`--rule` only make sense against a single run, and each `--status`
/// must name a real outcome. Reject early with an actionable message.
fn validate_filters(args: &HistoryArgs) -> Result<()> {
    if args.id.is_none() && (!args.status.is_empty() || !args.rule.is_empty()) {
        return Err(Error::History(
            "--status/--rule filter a single run; pass a run id (or `latest`)".to_string(),
        ));
    }
    for s in &args.status {
        if !VALID_STATUSES.contains(&s.as_str()) {
            return Err(Error::History(format!(
                "unknown --status {s:?}; valid statuses: {}",
                VALID_STATUSES.join(", ")
            )));
        }
    }
    Ok(())
}

/// Show one run. `--path` prints just the record's file path; otherwise the run's
/// results, optionally narrowed by `--status`/`--rule`, as human text or JSON.
fn show(dir: &Path, id: &str, args: &HistoryArgs) -> Result<i32> {
    let record = history::load(dir, id)?;
    if args.path {
        println!("{}", record.path.display());
        return Ok(0);
    }

    // Narrow the `rules` array in place when filters are given; everything else
    // (metadata, summary, errors) is left as recorded.
    let mut value = record.value.clone();
    if !args.status.is_empty() || !args.rule.is_empty() {
        if let Some(rules) = value.get("rules").and_then(Value::as_array) {
            let filtered = filter_rules(rules, &args.status, &args.rule);
            value["rules"] = Value::Array(filtered);
        }
    }

    match args.format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| Error::Io(e.to_string()))?
        ),
        OutputFormat::Human => print!("{}", render_run(&value, &record.id)),
    }
    Ok(0)
}

/// List recent runs, newest first, capped at `--limit` (default 20).
fn list(dir: &Path, args: &HistoryArgs) -> Result<i32> {
    if args.path {
        // `--path` with no id: the directory itself, for scripting.
        println!("{}", dir.display());
        return Ok(0);
    }
    let limit = args.limit.unwrap_or(20);
    let records: Vec<history::Record> = history::all(dir)?.into_iter().take(limit).collect();

    match args.format {
        OutputFormat::Json => {
            let arr: Vec<Value> = records.iter().map(list_entry).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&Value::Array(arr))
                    .map_err(|e| Error::Io(e.to_string()))?
            );
        }
        OutputFormat::Human => {
            if records.is_empty() {
                println!("No runs recorded in {}", dir.display());
                return Ok(0);
            }
            for r in &records {
                println!("{}", render_list_line(r));
            }
        }
    }
    Ok(0)
}

/// Keep only the rules matching every active filter: an `--status` set (any of
/// its outcomes) and/or an `--rule` set (any of its names).
fn filter_rules(rules: &[Value], statuses: &[String], names: &[String]) -> Vec<Value> {
    rules
        .iter()
        .filter(|r| {
            let status_ok = statuses.is_empty()
                || r.get("outcome")
                    .and_then(Value::as_str)
                    .is_some_and(|o| statuses.iter().any(|s| s == o));
            let name_ok = names.is_empty()
                || r.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| names.iter().any(|x| x == n));
            status_ok && name_ok
        })
        .cloned()
        .collect()
}

/// The compact per-run object emitted by `history --format json` when listing.
fn list_entry(r: &history::Record) -> Value {
    let g = |k: &str| r.value.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "id": r.id,
        "timestamp": g("timestamp"),
        "command": g("command"),
        "exit_code": g("exit_code"),
        "summary": g("summary"),
        "cwd": g("cwd"),
        "path": r.path.display().to_string(),
    })
}

/// One line in the human run listing: id, timestamp, exit code, and a terse
/// counts summary.
fn render_list_line(r: &history::Record) -> String {
    let s = |k: &str| r.value.get(k).and_then(Value::as_str).unwrap_or("");
    let exit = r
        .value
        .get("exit_code")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let counts = r
        .value
        .get("summary")
        .map(counts_summary)
        .unwrap_or_default();
    format!("{}  {}  exit {exit}  {counts}", r.id, s("timestamp"))
}

/// Render one run's full results as human-readable text.
fn render_run(value: &Value, id: &str) -> String {
    let s = |k: &str| value.get(k).and_then(Value::as_str).unwrap_or("");
    let exit = value.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
    let mut out = String::new();
    out.push_str(&format!("Run {id}  {}\n", s("timestamp")));
    out.push_str(&format!(
        "  command: {}   exit: {exit}   cwd: {}\n",
        s("command"),
        s("cwd")
    ));
    if let Some(files) = value.get("config_files").and_then(Value::as_array) {
        let joined: Vec<&str> = files.iter().filter_map(Value::as_str).collect();
        if !joined.is_empty() {
            out.push_str(&format!("  config: {}\n", joined.join(", ")));
        }
    }
    if let Some(summary) = value.get("summary") {
        out.push_str(&format!("  {}\n", counts_summary(summary)));
    }
    out.push('\n');

    if let Some(rules) = value.get("rules").and_then(Value::as_array) {
        if rules.is_empty() {
            out.push_str("  (no rules match)\n");
        }
        for rule in rules {
            push_rule(&mut out, rule);
        }
    }
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        for e in errors.iter().filter_map(Value::as_str) {
            out.push_str(&format!("  ERROR {e}\n"));
        }
    }
    out
}

/// Append one rule's line(s): the status label + name, its rationale (if any),
/// each judge's breakdown (multi-judge rules), and each violation's location.
fn push_rule(out: &mut String, rule: &Value) {
    let name = rule.get("name").and_then(Value::as_str).unwrap_or("");
    let outcome = rule.get("outcome").and_then(Value::as_str).unwrap_or("");
    let label = match outcome {
        "pass" => "PASS",
        "fail" => "FAIL",
        "skipped" => "SKIP",
        "ignored" => "IGN ",
        "not_relevant" => "N/A ",
        _ => "?   ",
    };
    let votes = match (
        rule.get("votes_total").and_then(Value::as_u64),
        rule.get("votes_hold").and_then(Value::as_u64),
    ) {
        (Some(total), Some(hold)) if total > 1 => format!(" ({hold}/{total} judges held)"),
        _ => String::new(),
    };
    out.push_str(&format!("  {label} {name}{votes}\n"));
    if let Some(r) = rule.get("rationale").and_then(Value::as_str) {
        let r = r.trim();
        if !r.is_empty() {
            out.push_str(&format!("       rationale: {r}\n"));
        }
    }
    if let Some(judges) = rule.get("judges").and_then(Value::as_array) {
        for (i, j) in judges.iter().enumerate() {
            let verdict = if j.get("relevant") == Some(&Value::Bool(false)) {
                "not relevant"
            } else if j.get("holds").and_then(Value::as_bool).unwrap_or(false) {
                "held"
            } else {
                "violated"
            };
            match j.get("rationale").and_then(Value::as_str) {
                Some(r) if !r.trim().is_empty() => {
                    out.push_str(&format!("       judge {} {verdict}: {}\n", i + 1, r.trim()))
                }
                _ => out.push_str(&format!("       judge {} {verdict}\n", i + 1)),
            }
        }
    }
    if let Some(violations) = rule.get("violations").and_then(Value::as_array) {
        for v in violations {
            out.push_str(&format!("       {}\n", format_violation(v)));
        }
    }
}

/// Format a stored violation JSON as `file:line[-end]: message`, matching the
/// terminal report's spelling.
fn format_violation(v: &Value) -> String {
    let mut loc = String::new();
    if let Some(file) = v.get("file").and_then(Value::as_str) {
        loc.push_str(file);
        if let Some(line) = v.get("line").and_then(Value::as_u64) {
            loc.push_str(&format!(":{line}"));
            if let Some(end) = v.get("end_line").and_then(Value::as_u64) {
                loc.push_str(&format!("-{end}"));
            }
        }
    }
    let msg = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("violation");
    if loc.is_empty() {
        msg.to_string()
    } else {
        format!("{loc}: {msg}")
    }
}

/// A terse counts line from a stored `summary` object: `N rules: X passed, Y
/// failed, Z skipped[, W not relevant][, E errored]` — the same shape the
/// terminal report prints.
fn counts_summary(summary: &Value) -> String {
    let g = |k: &str| summary.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut s = format!(
        "{} rules: {} passed, {} failed, {} skipped",
        g("total"),
        g("passed"),
        g("failed"),
        g("skipped")
    );
    if g("ignored") > 0 {
        s.push_str(&format!(", {} ignored", g("ignored")));
    }
    if g("not_relevant") > 0 {
        s.push_str(&format!(", {} not relevant", g("not_relevant")));
    }
    if g("errored") > 0 {
        s.push_str(&format!(", {} errored", g("errored")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Value {
        json!({
            "id": "20260704T000000Z-00001",
            "timestamp": "2026-07-04T00:00:00Z",
            "command": "lint",
            "cwd": "/proj",
            "exit_code": 1,
            "config_files": ["llmlint.yml"],
            "summary": {"total": 3, "passed": 1, "failed": 1, "skipped": 1, "not_relevant": 0, "errored": 0},
            "rules": [
                {"name": "ok_rule", "outcome": "pass", "votes_total": 1, "votes_hold": 1},
                {"name": "bad_rule", "outcome": "fail", "rationale": "raw SQL",
                 "violations": [{"file": "src/db.rs", "line": 12, "message": "inline SQL"}]},
                {"name": "no_files", "outcome": "skipped"}
            ],
            "errors": []
        })
    }

    #[test]
    fn filter_by_status_and_name() {
        let rules = record()["rules"].as_array().unwrap().clone();
        // Status filter keeps only failures.
        let fails = filter_rules(&rules, &["fail".into()], &[]);
        assert_eq!(fails.len(), 1);
        assert_eq!(fails[0]["name"], "bad_rule");
        // Name filter keeps only the named rule.
        let named = filter_rules(&rules, &[], &["ok_rule".into()]);
        assert_eq!(named.len(), 1);
        assert_eq!(named[0]["name"], "ok_rule");
        // Combined: both must match (no overlap here).
        assert!(filter_rules(&rules, &["pass".into()], &["bad_rule".into()]).is_empty());
        // No filters keeps everything.
        assert_eq!(filter_rules(&rules, &[], &[]).len(), 3);
    }

    #[test]
    fn render_run_shows_metadata_and_located_violation() {
        let text = render_run(&record(), "20260704T000000Z-00001");
        assert!(text.contains("Run 20260704T000000Z-00001  2026-07-04T00:00:00Z"));
        assert!(text.contains("command: lint   exit: 1   cwd: /proj"));
        assert!(text.contains("config: llmlint.yml"));
        assert!(text.contains("3 rules: 1 passed, 1 failed, 1 skipped"));
        assert!(text.contains("PASS ok_rule"));
        assert!(text.contains("FAIL bad_rule"));
        assert!(text.contains("rationale: raw SQL"));
        assert!(text.contains("src/db.rs:12: inline SQL"));
        assert!(text.contains("SKIP no_files"));
    }

    #[test]
    fn counts_summary_appends_optional_segments() {
        let base = json!({"total": 2, "passed": 2, "failed": 0, "skipped": 0});
        assert_eq!(
            counts_summary(&base),
            "2 rules: 2 passed, 0 failed, 0 skipped"
        );
        let extra = json!({"total": 3, "passed": 1, "failed": 0, "skipped": 0,
                           "not_relevant": 1, "errored": 1});
        let s = counts_summary(&extra);
        assert!(s.contains("1 not relevant"));
        assert!(s.contains("1 errored"));
    }

    #[test]
    fn multi_judge_breakdown_renders() {
        let rule = json!({
            "name": "voted", "outcome": "fail", "votes_total": 3, "votes_hold": 1,
            "judges": [
                {"holds": false, "rationale": "raw SQL"},
                {"holds": true, "rationale": "query layer"},
                {"relevant": false}
            ]
        });
        let mut out = String::new();
        push_rule(&mut out, &rule);
        assert!(out.contains("FAIL voted (1/3 judges held)"));
        assert!(out.contains("judge 1 violated: raw SQL"));
        assert!(out.contains("judge 2 held: query layer"));
        assert!(out.contains("judge 3 not relevant"));
    }
}
