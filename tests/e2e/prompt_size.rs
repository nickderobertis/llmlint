//! Prompt-size-bounded batching: `max_prompt_chars` / `--max-prompt-chars`.
//! Each journey drives `llmlint --diff` with the stand-in harness logging every
//! prompt it receives (`LLMLINT_MOCK_PROMPTLOG`), and reads back what each call
//! carried from the rendered prompt itself — its target-file list and each rule's
//! `Scope:` line — so "every rule judged every file once" is checked against what
//! the judge was actually shown.

// llmlint: ignore-file[e2e_not_mocked] the stand-in oneharness is this suite's double for the oneharness subprocess, the one genuinely external boundary (AGENTS.md); the prompt each judge call receives can only be recorded at that seam, and the journeys still drive the real freshly built llmlint binary over real git repos

use std::collections::BTreeMap;
use std::fs;

use super::*;

const FILES: [&str; 6] = [
    "src/f0.rs",
    "src/f1.rs",
    "src/f2.rs",
    "src/f3.rs",
    "src/f4.rs",
    "src/f5.rs",
];

/// `a_rule` covers every file; `b_rule` only `f1` and `f4`.
const RULES: &str = "  - { name: a_rule, description: \"true when ok; false otherwise.\" }\n  \
     - { name: b_rule, description: \"true when ok; false otherwise.\", \
     files: { include: [\"src/f1.rs\", \"src/f4.rs\"] } }\n";

/// A committed repo whose six files each gain `lines` changed lines, so `--diff`
/// selects all six with a diff of roughly `lines * 40` characters each.
fn changed_repo(extra_config: &str, lines: usize) -> Project {
    let initial: Vec<(&str, &str)> = FILES.iter().map(|f| (*f, "fn base() {}\n")).collect();
    let p = committed_repo(RULES, &initial);
    if !extra_config.is_empty() {
        let cfg = fs::read_to_string(p.path().join("llmlint.yml")).unwrap();
        p.write("llmlint.yml", &format!("{extra_config}{cfg}"));
        git(p.path(), &["commit", "-qam", "config"]);
    }
    for f in FILES {
        let body: String = (0..lines)
            .map(|i| format!("fn changed_line_number_{i:04}() {{}}\n"))
            .collect();
        p.write(f, &format!("fn base() {{}}\n{body}"));
    }
    p
}

/// One logged harness call: its prompt size, the files it carried, and the files
/// each rule was scoped to in it.
struct Call {
    chars: usize,
    files: Vec<String>,
    scopes: BTreeMap<String, Vec<String>>,
}

fn parse_call(record: &Value) -> Call {
    let system = record["system"].as_str().unwrap();
    let prompt = record["prompt"].as_str().unwrap();
    let chars = system.chars().count() + prompt.chars().count();
    let targets = system
        .split("## Target files")
        .nth(1)
        .unwrap()
        .split("## Rules to evaluate")
        .next()
        .unwrap();
    let files: Vec<String> = targets
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(str::to_string)
        .collect();
    let rules = system
        .split("## Rules to evaluate")
        .nth(1)
        .unwrap()
        .split("## Response")
        .next()
        .unwrap();
    let mut scopes = BTreeMap::new();
    for section in rules.split("### ").skip(1) {
        let name = section.lines().next().unwrap().trim().to_string();
        let scope = section
            .lines()
            .find_map(|l| l.strip_prefix("Scope: "))
            .unwrap();
        let covered: Vec<String> = if let Some(rest) = scope.strip_prefix("all target files") {
            let except: Vec<&str> = rest
                .strip_prefix(" except: ")
                .map(|e| e.split(", ").collect())
                .unwrap_or_default();
            files
                .iter()
                .filter(|f| !except.contains(&f.as_str()))
                .cloned()
                .collect()
        } else {
            scope.split(", ").map(str::to_string).collect()
        };
        scopes.insert(name, covered);
    }
    Call {
        chars,
        files,
        scopes,
    }
}

/// Every call the harness received, in no particular order.
fn calls(log: &Path) -> Vec<Call> {
    fs::read_dir(log)
        .map(|rd| {
            rd.map(|e| {
                let text = fs::read_to_string(e.unwrap().path()).unwrap();
                parse_call(&serde_json::from_str(&text).unwrap())
            })
            .collect()
        })
        .unwrap_or_default()
}

/// How many times each rule was shown each file, across every call.
fn coverage(calls: &[Call]) -> BTreeMap<(String, String), usize> {
    let mut seen = BTreeMap::new();
    for call in calls {
        for (rule, files) in &call.scopes {
            for file in files {
                *seen.entry((rule.clone(), file.clone())).or_insert(0) += 1;
            }
        }
    }
    seen
}

/// Run `lint --diff --format json` with every prompt logged under `log`.
fn run(p: &Project, log: &str, extra: &[&str]) -> (std::process::Output, Vec<Call>) {
    let log = p.path().join(log);
    let verdicts = p.write_verdicts(
        r#"{"a_rule": true, "b_rule": {"holds": false, "violations": [{"message": "b broke"}]}}"#,
    );
    let out = p
        .lint()
        .args(["--diff", "--format", "json"])
        .args(extra)
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .output()
        .unwrap();
    (out, calls(&log))
}

/// Each rule's outcome, votes and violations from a `--format json` report — the
/// verdicts a bounded run must leave unchanged.
fn verdicts(out: &std::process::Output) -> Vec<(String, Value, Value, Value)> {
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["outcome"].clone(),
                r["votes_total"].clone(),
                r["violations"].clone(),
            )
        })
        .collect()
}

#[test]
fn a_diff_over_the_bound_is_split_and_every_rule_still_judges_every_file_once() {
    let p = changed_repo("", 200);
    let (whole, whole_calls) = run(&p, "whole", &[]);
    assert_eq!(
        whole.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&whole.stderr)
    );
    assert_eq!(whole_calls.len(), 1, "the default bound fits one call");
    let full = whole_calls[0].chars;

    let bound = full * 2 / 5;
    let (split, split_calls) = run(&p, "split", &["--max-prompt-chars", &bound.to_string()]);
    assert!(split_calls.len() >= 3, "{} calls", split_calls.len());
    for call in &split_calls {
        assert!(
            call.chars <= bound,
            "a {}-char prompt over {bound}",
            call.chars
        );
        assert!(!call.files.is_empty());
    }
    // Every rule saw every file it selects exactly once — the same coverage as
    // the one unbounded call.
    let expected = coverage(&whole_calls);
    assert_eq!(expected.len(), FILES.len() + 2);
    assert!(expected.values().all(|n| *n == 1));
    assert_eq!(coverage(&split_calls), expected);
    // And the verdicts are those of the unbounded run.
    assert_eq!(split.status.code(), Some(1));
    assert_eq!(verdicts(&split), verdicts(&whole));
}

#[test]
fn a_file_whose_one_diff_line_cannot_fit_is_an_error_and_is_never_sent() {
    // Splitting stops at a line: a single changed line longer than the bound
    // leaves no part that could carry it, so the file is refused, never dropped.
    let p = changed_repo("", 2);
    let line: String = "x".repeat(12_000);
    p.write(
        "src/f3.rs",
        &format!("fn base() {{}}\nconst HUGE: &str = \"{line}\";\n"),
    );
    let bound = 8_000;
    let (out, calls) = run(&p, "log", &["--max-prompt-chars", &bound.to_string()]);
    assert_eq!(out.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let errors = report["errors"].to_string();
    assert!(
        errors.contains("file src/f3.rs: its judge prompt is "),
        "{errors}"
    );
    assert!(
        errors.contains(&format!(
            "characters, over the max_prompt_chars bound of {bound}"
        )),
        "{errors}"
    );
    assert!(
        errors.contains("Exclude the file if it is generated"),
        "{errors}"
    );
    assert!(errors.contains("split the change"), "{errors}");
    assert!(errors.contains("raise max_prompt_chars"), "{errors}");
    // No prompt over the bound reached the harness, and none carried the file;
    // the other files were still judged.
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(
            call.chars <= bound,
            "a {}-char prompt over {bound}",
            call.chars
        );
        assert!(!call.files.contains(&"src/f3.rs".to_string()));
    }
    let judged: Vec<String> = coverage(&calls).into_keys().map(|(_, f)| f).collect();
    assert!(judged.contains(&"src/f5.rs".to_string()));
}

/// A table of `ROWS` rows, every tenth of which a regeneration rewrites: one
/// small hunk per rewritten row, the shape of a large generated file whose
/// whole diff is over the bound.
const ROWS: usize = 3_000;
const TABLE_RULE: &str = "  - { name: t_rule, description: \"true when ok; false otherwise.\" }\n";

fn table(rewritten: bool) -> String {
    (0..ROWS)
        .map(|i| {
            let cell = if rewritten && i % 10 == 5 {
                "regenerated"
            } else {
                "original"
            };
            format!("| row {i:04} | {cell} |\n")
        })
        .collect()
}

/// A repo whose one table file was rewritten in `ROWS / 10` places.
fn regenerated_table() -> Project {
    let p = committed_repo(TABLE_RULE, &[("src/table.md", &table(false))]);
    p.write("src/table.md", &table(true));
    p
}

/// One judge call over a part of the table's diff: its part number, how many
/// parts there were, its size, and the hunks it carried (the diff block from its
/// first `@@` line).
struct TablePart {
    index: usize,
    of: usize,
    chars: usize,
    hunks: String,
    files: Vec<String>,
}

fn table_parts(log: &Path) -> Vec<TablePart> {
    let mut parts: Vec<TablePart> = fs::read_dir(log)
        .unwrap()
        .map(|e| {
            let record: Value =
                serde_json::from_str(&fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
            let call = parse_call(&record);
            let system = record["system"].as_str().unwrap();
            let note = system
                .split("This is part ")
                .nth(1)
                .unwrap_or_else(|| panic!("no part note:\n{system}"));
            let (index, rest) = note.split_once(" of ").unwrap();
            let of = rest.split(' ').next().unwrap();
            let diff = system.split("```diff\n").nth(1).unwrap();
            let diff = diff.split("```").next().unwrap();
            // The diff's own text, without the blank line closing the fence.
            let hunks = format!("{}\n", diff[diff.find("@@").unwrap()..].trim_end());
            TablePart {
                index: index.parse().unwrap(),
                of: of.parse().unwrap(),
                chars: call.chars,
                hunks,
                files: call.files,
            }
        })
        .collect();
    parts.sort_by_key(|p| p.index);
    parts
}

const TABLE_BOUND: usize = 20_000;

#[test]
fn an_oversized_diff_is_judged_in_parts_each_within_the_bound() {
    let p = regenerated_table();
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json"])
        .args(["--max-prompt-chars", &TABLE_BOUND.to_string()])
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    let parts = table_parts(&log);
    assert!(parts.len() >= 3, "{} parts", parts.len());
    let whole = git_out(p.path(), &["diff", "HEAD", "--", "src/table.md"]);
    let whole_hunks = &whole[whole.find("@@").unwrap()..];
    assert!(
        whole_hunks.len() > TABLE_BOUND,
        "the diff alone is over the bound"
    );
    for (i, part) in parts.iter().enumerate() {
        assert_eq!((part.index, part.of), (i + 1, parts.len()));
        assert_eq!(part.files, vec!["src/table.md".to_string()]);
        assert!(part.chars <= TABLE_BOUND, "a {}-char prompt", part.chars);
        // Cut only between hunks: every header is one of the whole diff's.
        for header in part.hunks.lines().filter(|l| l.starts_with("@@")) {
            assert!(whole_hunks.contains(&format!("{header}\n")), "{header}");
        }
        // Packed: the next part's hunks would not have fit beside this one's.
        if let Some(next) = parts.get(i + 1) {
            assert!(
                part.chars + next.hunks.chars().count() > TABLE_BOUND,
                "parts {} and {} would fit one prompt",
                part.index,
                next.index
            );
        }
    }
    // Every hunk of the change was judged, once and in order.
    let judged: String = parts.iter().map(|p| p.hunks.as_str()).collect();
    assert_eq!(judged, format!("{}\n", whole_hunks.trim_end()));
    // The parts are one judge's vote: the file passes when every part does.
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "pass", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");

    // The plan names each call's part, and the same input splits the same way.
    let plan = p
        .lint()
        .args(["--diff", "--plan-only"])
        .args(["--max-prompt-chars", &TABLE_BOUND.to_string()])
        .output()
        .unwrap();
    let plan = String::from_utf8_lossy(&plan.stdout);
    let n = parts.len();
    assert!(
        plan.contains(&format!("src/table.md (diff part {n} of {n})")),
        "{plan}"
    );
    let again = p.path().join("again");
    p.lint()
        .args(["--diff", "--max-prompt-chars", &TABLE_BOUND.to_string()])
        .env("LLMLINT_MOCK_PROMPTLOG", &again)
        .output()
        .unwrap();
    let again: Vec<String> = table_parts(&again).into_iter().map(|p| p.hunks).collect();
    let first: Vec<String> = parts.into_iter().map(|p| p.hunks).collect();
    assert_eq!(again, first);
}

#[test]
fn a_rule_failing_in_one_part_of_an_oversized_file_fails_the_file() {
    let p = regenerated_table();
    // Serially, the parts arrive in order: the rule fails in the second alone.
    let spec = p.write_verdicts(
        r#"{"t_rule": [true, {"holds": false, "violations": [{"file": "src/table.md", "line": 106, "message": "bad cell"}]}, true]}"#,
    );
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json", "--max-parallel", "1"])
        .args(["--max-prompt-chars", &TABLE_BOUND.to_string()])
        .env("LLMLINT_MOCK_VERDICTS", &spec)
        .env("LLMLINT_MOCK_STATE", p.path().join("state"))
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(table_parts(&log).len() >= 3);
    assert_eq!(out.status.code(), Some(1), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "fail", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");
    let messages: Vec<&str> = rule["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages, vec!["bad cell"], "{report:#}");
}

#[test]
fn the_flag_overrides_the_config_bound() {
    // A config bound small enough to split; the flag lifts it back to one call.
    let p = changed_repo("max_prompt_chars: 20000\n", 200);
    let (_, by_config) = run(&p, "config", &[]);
    assert!(by_config.len() > 1, "the config bound should split");
    assert!(by_config.iter().all(|c| c.chars <= 20000));
    let (_, by_flag) = run(&p, "flag", &["--max-prompt-chars", "1000000"]);
    assert_eq!(by_flag.len(), 1, "the flag should win over the config");

    // And the other way: a generous config, a tight flag.
    let p = changed_repo("max_prompt_chars: 1000000\n", 200);
    let (_, tight) = run(&p, "tight", &["--max-prompt-chars", "20000"]);
    assert!(tight.len() > 1);
    assert!(tight.iter().all(|c| c.chars <= 20000));
}

#[test]
fn plan_only_shows_the_split_without_calling_the_harness() {
    let p = changed_repo("", 200);
    let spawns = p.path().join("spawns");
    let out = p
        .lint()
        .args(["--diff", "--plan-only", "--max-prompt-chars", "20000"])
        .env("LLMLINT_MOCK_SPAWNLOG", &spawns)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("judge calls to keep each prompt within max_prompt_chars 20000"),
        "{text}"
    );
    assert!(text.contains("call 1: [a_rule"), "{text}");
    assert_eq!(
        harness_spawns(&spawns),
        0,
        "--plan-only must not run the harness"
    );
    // The control: the same run without `--plan-only` does spawn it.
    p.lint()
        .args(["--diff", "--max-prompt-chars", "20000"])
        .env("LLMLINT_MOCK_SPAWNLOG", &spawns)
        .output()
        .unwrap();
    assert!(harness_spawns(&spawns) > 0);
}

#[test]
fn the_env_var_sets_the_bound_and_the_flag_beats_it() {
    let p = changed_repo("", 200);
    let log = |name: &str, env: &[(&str, &str)], extra: &[&str]| {
        let dir = p.path().join(name);
        let mut c = p.lint();
        c.args(["--diff"])
            .args(extra)
            .env("LLMLINT_MOCK_PROMPTLOG", &dir);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap();
        calls(&dir)
    };
    let by_env = log("env", &[("LLMLINT_MAX_PROMPT_CHARS", "20000")], &[]);
    assert!(by_env.len() > 1, "the env bound should split");
    assert!(by_env.iter().all(|c| c.chars <= 20000));
    let by_flag = log(
        "flag",
        &[("LLMLINT_MAX_PROMPT_CHARS", "20000")],
        &["--max-prompt-chars", "1000000"],
    );
    assert_eq!(by_flag.len(), 1, "the flag should win over the env var");
}

#[test]
fn a_rework_over_the_bound_is_skipped_and_cleanup_still_decides() {
    // `b_rule` covers only f1 and f4, but its judge pins a violation to f0: a
    // wrong-file verdict that normally earns one corrective re-ask.
    let p = changed_repo("", 2);
    let spec = p.write_verdicts(
        r#"{"a_rule": true, "b_rule": {"holds": false, "violations": [{"file": "src/f0.rs", "line": 2, "message": "wrong file"}]}}"#,
    );
    let run = |name: &str, extra: &[&str]| {
        let dir = p.path().join(name);
        let out = p
            .lint()
            .args(["--diff", "--format", "json"])
            .args(extra)
            .env("LLMLINT_MOCK_VERDICTS", &spec)
            .env("LLMLINT_MOCK_PROMPTLOG", &dir)
            .output()
            .unwrap();
        (out, calls(&dir))
    };
    let (unbounded, with_rework) = run("unbounded", &[]);
    assert_eq!(with_rework.len(), 2, "the control re-asks once");
    assert_eq!(unbounded.status.code(), Some(0));
    // A bound the first prompt fits exactly, which the longer rework would not.
    let first = with_rework.iter().map(|c| c.chars).min().unwrap();
    let (bounded, calls) = run("bounded", &["--max-prompt-chars", &first.to_string()]);
    assert_eq!(calls.len(), 1, "the over-bound rework must not be sent");
    assert!(calls[0].chars <= first);
    // The deterministic cleanup still drops the wrong-file violation: b passes.
    assert_eq!(bounded.status.code(), Some(0));
    assert_eq!(verdicts(&bounded), verdicts(&unbounded));
}

#[test]
fn a_conditional_rule_split_across_calls_casts_one_merged_vote() {
    let rules = "  - { name: c_rule, description: \"true when ok; false otherwise.\", \
         relevance: \"the change adds a function\" }\n";
    let initial: Vec<(&str, &str)> = FILES.iter().map(|f| (*f, "fn base() {}\n")).collect();
    let p = committed_repo(rules, &initial);
    for f in FILES {
        let body: String = (0..200)
            .map(|i| format!("fn changed_line_number_{i:04}() {{}}\n"))
            .collect();
        p.write(f, &format!("fn base() {{}}\n{body}"));
    }
    // The first call finds it not relevant; every later call finds it holds.
    let verdicts = p.write_verdicts(
        r#"{"c_rule": [{"relevant": false, "rationale": "n/a here"}, {"relevant": true, "holds": true, "rationale": "fine here"}]}"#,
    );
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json", "--max-parallel", "1"])
        .args(["--max-prompt-chars", "20000"])
        .env("LLMLINT_MOCK_VERDICTS", &verdicts)
        .env("LLMLINT_MOCK_STATE", p.path().join("state"))
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let parts = calls(&log).len();
    assert!(parts >= 3, "{parts} calls");
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rule = &report["rules"][0];
    // Relevant because some part found it so; one judge, so one vote.
    assert_eq!(rule["outcome"], "pass", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");
    // The rationale joins the parts that agree with the merged verdict.
    let expected = vec!["fine here"; parts - 1].join(" ");
    assert_eq!(rule["rationale"], expected.as_str(), "{report:#}");
}

#[test]
fn split_parts_stay_within_their_own_judge_s_vote() {
    let rules =
        "  - { name: j_rule, judges: 3, description: \"true when ok; false otherwise.\" }\n";
    let initial: Vec<(&str, &str)> = FILES.iter().map(|f| (*f, "fn base() {}\n")).collect();
    let p = committed_repo(rules, &initial);
    for f in FILES {
        let body: String = (0..200)
            .map(|i| format!("fn changed_line_number_{i:04}() {{}}\n"))
            .collect();
        p.write(f, &format!("fn base() {{}}\n{body}"));
    }
    // Run serially, the calls arrive judge by judge, each judge's parts in turn.
    // Judge 1 holds in every part; judge 2 fails only in its first part; judge 3
    // fails only in its last. Grouped by judge that is one pass and two fails.
    let ok = r#"{"holds": true}"#;
    let fail = |m: &str| format!(r#"{{"holds": false, "violations": [{{"message": "{m}"}}]}}"#);
    let probe = p.path().join("probe");
    let plan = p
        .lint()
        .args(["--diff", "--plan-only", "--max-prompt-chars", "20000"])
        .output()
        .unwrap();
    let plan = String::from_utf8_lossy(&plan.stdout).into_owned();
    let parts: usize = plan
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("split into ")
                .and_then(|r| r.split(' ').next())
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or_else(|| panic!("no split in the plan:\n{plan}"));
    assert!(parts >= 2, "{plan}");
    let mut seq: Vec<String> = Vec::new();
    for judge in 1..=3 {
        for part in 1..=parts {
            seq.push(match (judge, part) {
                (2, 1) => fail("judge two"),
                (3, n) if n == parts => fail("judge three"),
                _ => ok.to_string(),
            });
        }
    }
    let spec = p.write_verdicts(&format!(r#"{{"j_rule": [{}]}}"#, seq.join(", ")));
    let out = p
        .lint()
        .args(["--diff", "--format", "json", "--max-parallel", "1"])
        .args(["--max-prompt-chars", "20000"])
        .env("LLMLINT_MOCK_VERDICTS", &spec)
        .env("LLMLINT_MOCK_STATE", p.path().join("state"))
        .env("LLMLINT_MOCK_PROMPTLOG", &probe)
        .output()
        .unwrap();
    assert_eq!(calls(&probe).len(), 3 * parts);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rule = &report["rules"][0];
    assert_eq!(rule["votes_total"], 3, "{report:#}");
    assert_eq!(rule["votes_hold"], 1, "{report:#}");
    assert_eq!(rule["outcome"], "fail", "{report:#}");
    let messages: Vec<&str> = rule["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages, vec!["judge two", "judge three"], "{report:#}");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn the_nearest_config_s_bound_wins_and_an_ancestor_s_is_inherited() {
    // The ancestor sets the bound; the config the run starts from declares the
    // rules and files but no bound of its own.
    let p = Project::new();
    p.write("llmlint.yml", "max_prompt_chars: 20000\n");
    let leaf = format!("files:\n  include: [\"src/**\"]\nrules:\n{RULES}");
    p.write("sub/llmlint.yml", &leaf);
    for f in FILES {
        p.write(&format!("sub/{f}"), "fn base() {}\n");
    }
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    for f in FILES {
        let body: String = (0..200)
            .map(|i| format!("fn changed_line_number_{i:04}() {{}}\n"))
            .collect();
        p.write(&format!("sub/{f}"), &format!("fn base() {{}}\n{body}"));
    }
    let run = |name: &str| {
        let dir = p.path().join(name);
        p.lint()
            .current_dir(p.path().join("sub"))
            .arg("--diff")
            .env("LLMLINT_MOCK_PROMPTLOG", &dir)
            .output()
            .unwrap();
        calls(&dir)
    };
    let inherited = run("inherited");
    assert!(inherited.len() > 1, "the ancestor's bound should split");
    assert!(inherited.iter().all(|c| c.chars <= 20000));
    p.write(
        "sub/llmlint.yml",
        &format!("max_prompt_chars: 1000000\n{leaf}"),
    );
    assert_eq!(run("nearest").len(), 1, "the nearer config's bound wins");
}

#[test]
fn a_change_whose_every_file_is_over_the_bound_makes_no_judge_call() {
    let p = changed_repo("", 200);
    let spawns = p.path().join("spawns");
    let (out, calls) = run(&p, "log", &["--max-prompt-chars", "2000"]);
    assert!(
        calls.is_empty(),
        "{} prompts reached the harness",
        calls.len()
    );
    assert_eq!(out.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), FILES.len(), "{report:#}");
    for f in FILES {
        assert!(
            errors
                .iter()
                .any(|e| e.as_str().unwrap().contains(&format!("file {f}:"))),
            "{f}: {report:#}"
        );
    }
    // The plan says the same, with no call to make.
    let plan = p
        .lint()
        .args(["--diff", "--plan-only", "--max-prompt-chars", "2000"])
        .env("LLMLINT_MOCK_SPAWNLOG", &spawns)
        .output()
        .unwrap();
    let plan = String::from_utf8_lossy(&plan.stdout);
    assert!(plan.starts_with("Plan: 0 judge call(s)"), "{plan}");
    assert_eq!(harness_spawns(&spawns), 0);
}

#[test]
fn a_conditional_rule_not_relevant_in_any_part_stays_not_relevant() {
    let rules = "  - { name: c_rule, description: \"true when ok; false otherwise.\", \
         relevance: \"the change adds a function\" }\n";
    let initial: Vec<(&str, &str)> = FILES.iter().map(|f| (*f, "fn base() {}\n")).collect();
    let p = committed_repo(rules, &initial);
    for f in FILES {
        let body: String = (0..200)
            .map(|i| format!("fn changed_line_number_{i:04}() {{}}\n"))
            .collect();
        p.write(f, &format!("fn base() {{}}\n{body}"));
    }
    let spec = p.write_verdicts(r#"{"c_rule": {"relevant": false, "rationale": "n/a here"}}"#);
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json", "--max-prompt-chars", "20000"])
        .env("LLMLINT_MOCK_VERDICTS", &spec)
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let parts = calls(&log).len();
    assert!(parts >= 3, "{parts} calls");
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "not_relevant", "{report:#}");
    assert_eq!(rule["votes_total"], 0, "{report:#}");
    let expected = vec!["n/a here"; parts].join(" ");
    assert_eq!(rule["rationale"], expected.as_str(), "{report:#}");
}

#[test]
fn a_zero_bound_is_rejected() {
    let p = changed_repo("max_prompt_chars: 0\n", 1);
    p.lint()
        .arg("--diff")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("max_prompt_chars"));
    p.lint()
        .args(["--diff", "--max-prompt-chars", "0"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--max-prompt-chars"));
    let clean = changed_repo("", 1);
    clean
        .lint()
        .arg("--diff")
        .env("LLMLINT_MAX_PROMPT_CHARS", "0")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("LLMLINT_MAX_PROMPT_CHARS"));
}

#[test]
fn a_hunk_over_the_bound_is_cut_at_lines_keeping_true_line_numbers_and_ignores() {
    // Every other row rewritten: one hunk far over the bound, its single-line
    // change runs separated by context. Rows 1000-1099 sit in an ignore block
    // for the rule, so their runs are trimmed from whichever part carries them —
    // which only lands on the right lines if each cut part is re-headered with
    // the file's true line numbers.
    const N: usize = 2_000;
    let row = |i: usize, cell: &str| format!("| row {i:04} | {cell} cell value |\n");
    let rows = |rewritten: bool| -> String {
        (0..N)
            .map(|i| {
                let mut out = String::new();
                if i == 1_000 {
                    out.push_str(&format!(
                        "<!-- llmlint: {}[t_rule] generated rows -->\n",
                        "ignore-block"
                    ));
                }
                out.push_str(&row(
                    i,
                    if rewritten && i % 2 == 1 {
                        "regenerated"
                    } else {
                        "original"
                    },
                ));
                if i == 1_099 {
                    out.push_str(&format!("<!-- llmlint: {}[t_rule] -->\n", "ignore-end"));
                }
                out
            })
            .collect()
    };
    let p = committed_repo(
        TABLE_RULE,
        &[("src/block.md", &rows(false)), ("src/small.md", "a\n")],
    );
    p.write("src/block.md", &rows(true));
    p.write("src/small.md", "b\n");
    let whole = git_out(p.path(), &["diff", "HEAD", "--", "src/block.md"]);
    assert_eq!(whole.matches("\n@@").count(), 1, "one hunk:\n{whole}");
    let bound = 30_000;
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json", "--max-parallel", "1"])
        .args(["--max-prompt-chars", &bound.to_string()])
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");

    // The new file's line for each row, to check every part's numbering against.
    let line_of: BTreeMap<String, usize> = rows(true)
        .lines()
        .enumerate()
        .map(|(n, l)| (l.to_string(), n + 1))
        .collect();
    let records: Vec<Value> = fs::read_dir(&log)
        .unwrap()
        .map(|e| serde_json::from_str(&fs::read_to_string(e.unwrap().path()).unwrap()).unwrap())
        .collect();
    let mut seen: BTreeMap<usize, usize> = BTreeMap::new();
    let mut omitted: Vec<usize> = Vec::new();
    let mut parts = 0;
    for record in &records {
        let call = parse_call(record);
        assert!(call.chars <= bound, "a {}-char prompt", call.chars);
        if call.files != vec!["src/block.md".to_string()] {
            continue;
        }
        parts += 1;
        let system = record["system"].as_str().unwrap();
        let diff = system.split("```diff\n").nth(1).unwrap();
        let diff = diff.split("```").next().unwrap();
        let mut new_line = 0;
        for l in diff.lines() {
            if let Some(header) = l.strip_prefix("@@ ") {
                new_line = header
                    .split('+')
                    .nth(1)
                    .and_then(|r| r.split(',').next())
                    .unwrap()
                    .parse()
                    .unwrap();
            } else if let Some(text) = l.strip_prefix('+') {
                if text.starts_with("++") {
                    continue;
                }
                // Each added row is numbered with its true line in the file.
                assert_eq!(line_of[text], new_line, "{text}");
                *seen.entry(new_line).or_default() += 1;
                new_line += 1;
            } else if let Some(marker) = l.strip_prefix("[llmlint: 1 changed line ") {
                // A trimmed run stands in for its one added line, at that line.
                let at: usize = marker.split(' ').next().unwrap().parse().unwrap();
                assert_eq!(at, new_line, "{l}");
                omitted.push(at);
                new_line += 1;
            } else if l.starts_with(' ') {
                new_line += 1;
            }
        }
    }
    assert!(parts >= 3, "{parts} parts");
    // Every rewritten row outside the ignore block was shown exactly once; every
    // one inside it was trimmed, each at its own line.
    let rewritten = |lo: usize, hi: usize| -> Vec<usize> {
        (lo..hi)
            .filter(|i| i % 2 == 1)
            .map(|i| line_of[row(i, "regenerated").trim_end()])
            .collect()
    };
    let mut shown = rewritten(0, 1_000);
    shown.extend(rewritten(1_100, N));
    assert_eq!(seen.keys().copied().collect::<Vec<_>>(), shown);
    assert!(seen.values().all(|n| *n == 1));
    omitted.sort_unstable();
    assert_eq!(omitted, rewritten(1_000, 1_100));

    // The JSON plan names each diff part and leaves the whole small file's
    // call without one.
    let batch = &report["plan"]["agents"][0]["judges"][0]["batches"][0];
    let planned = batch["parts"].as_array().unwrap();
    assert_eq!(planned.len(), parts + 1, "{batch:#}");
    for (i, part) in planned[..parts].iter().enumerate() {
        assert_eq!(part["files"], serde_json::json!(["src/block.md"]));
        assert_eq!(
            part["diff_part"],
            serde_json::json!({"index": i + 1, "of": parts}),
            "{batch:#}"
        );
    }
    let small = &planned[parts];
    assert_eq!(small["files"], serde_json::json!(["src/small.md"]));
    assert!(small.get("diff_part").is_none(), "{batch:#}");

    // The persisted record, read back through `llmlint history`, is schema
    // version 2 and keeps every part's metadata.
    let latest = || -> Value {
        let out = p
            .bare()
            .args(["history", "latest", "--format", "json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let record = latest();
    assert_eq!(record["schema_version"], 2, "{record:#}");
    assert_eq!(
        record["plan"]["agents"][0]["judges"][0]["batches"][0]["parts"],
        batch["parts"]
    );

    // A run that fits one call writes a version-1 record: no stamp, no part.
    // (Cleared first: two runs in one second need not sort by run order.)
    fs::remove_dir_all(p.history_dir()).unwrap();
    let whole = p
        .lint()
        .args(["--diff", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(whole.status.code(), Some(0));
    let record = latest();
    assert!(record.get("schema_version").is_none(), "{record:#}");
    assert!(!record.to_string().contains("diff_part"), "{record:#}");
}

#[test]
fn a_part_is_filled_from_an_oversized_next_hunk_and_keeps_no_newline_markers() {
    // A small hunk, then a hunk far over the bound running to the end of a file
    // with no final newline: the first part carries the small hunk whole and
    // fills the rest with the big hunk's first lines, and each `\ No newline`
    // marker stays under the line it qualifies.
    const N: usize = 1_500;
    let rows = |rewritten: bool| -> String {
        let body: Vec<String> = (0..N)
            .map(|i| {
                let changed = rewritten && (i == 2 || (i >= 200 && i % 2 == 1) || i == N - 1);
                let cell = if changed { "regenerated" } else { "original" };
                format!("| row {i:04} | {cell} cell value |")
            })
            .collect();
        body.join("\n")
    };
    let p = committed_repo(TABLE_RULE, &[("src/tail.md", &rows(false))]);
    p.write("src/tail.md", &rows(true));
    let whole = git_out(p.path(), &["diff", "HEAD", "--", "src/tail.md"]);
    assert_eq!(whole.matches("\n@@").count(), 2, "two hunks:\n{whole}");
    assert_eq!(whole.matches("\\ No newline at end of file").count(), 2);
    let bound = 20_000;
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--diff", "--format", "json"])
        .args(["--max-prompt-chars", &bound.to_string()])
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");

    let parts = table_parts(&log);
    assert!(parts.len() >= 3, "{} parts", parts.len());
    let mut added: Vec<String> = Vec::new();
    let mut markers = 0;
    for part in &parts {
        assert!(part.chars <= bound, "a {}-char prompt", part.chars);
        let mut prev = "";
        for l in part.hunks.lines() {
            if l.starts_with('\\') {
                // A marker never opens a part or a hunk.
                assert!(
                    prev.starts_with('+') || prev.starts_with('-'),
                    "{l} after {prev}"
                );
                markers += 1;
            } else if let Some(row) = l.strip_prefix('+') {
                added.push(row.to_string());
            }
            prev = l;
        }
    }
    assert_eq!(markers, 2);
    // Every added row, once and in file order.
    let expected: Vec<String> = whole
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| l[1..].to_string())
        .collect();
    assert_eq!(added, expected);
    // The first part is the small hunk whole, then the big hunk's first lines
    // under a header recomputed for them.
    let first = &parts[0].hunks;
    let headers: Vec<&str> = first.lines().filter(|l| l.starts_with("@@")).collect();
    assert_eq!(headers.len(), 2, "{first}");
    assert!(
        whole.contains(&format!("{}\n", headers[0])),
        "{}",
        headers[0]
    );
    assert!(
        !whole.contains(&format!("{}\n", headers[1])),
        "{}",
        headers[1]
    );
    // The last part ends at the file's last line and its marker.
    let last = &parts.last().unwrap().hunks;
    assert!(
        last.ends_with(&format!(
            "+| row {:04} | regenerated cell value |\n\\ No newline at end of file\n",
            N - 1
        )),
        "{last}"
    );
}

/// One judge call over a part of a file's content (no `--diff`): its part
/// number, how many parts there were, its size, the file's line range it
/// carried, and those lines with their line-number prefixes removed.
struct LinesPart {
    index: usize,
    of: usize,
    chars: usize,
    start_line: usize,
    end_line: usize,
    lines: Vec<String>,
    files: Vec<String>,
}

/// Every logged call that carried a part of a file's content, in part order —
/// read from the section llmlint appends to such a prompt whatever the
/// template — and the records of the calls that carried none.
fn lines_parts(log: &Path) -> (Vec<LinesPart>, Vec<Value>) {
    let mut parts = Vec::new();
    let mut whole = Vec::new();
    for e in fs::read_dir(log).unwrap() {
        let record: Value =
            serde_json::from_str(&fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
        let system = record["system"].as_str().unwrap();
        let Some(section) = system.split("\n## Part of ").nth(1) else {
            whole.push(record);
            continue;
        };
        let file = section.split(" under review\n").next().unwrap().to_string();
        let note = section.split("This is part ").nth(1).unwrap();
        let (index, rest) = note.split_once(" of ").unwrap();
        let (of, rest) = rest.split_once(": lines ").unwrap();
        let (start_line, rest) = rest.split_once('-').unwrap();
        let end_line = rest.split(' ').next().unwrap();
        let block = section
            .split("\n\n```\n")
            .nth(1)
            .unwrap()
            .split("\n```\n")
            .next()
            .unwrap();
        let lines = block
            .lines()
            .map(|l| l.trim_start().split_once("  ").unwrap().1.to_string())
            .collect();
        let chars = system.chars().count() + record["prompt"].as_str().unwrap().chars().count();
        parts.push(LinesPart {
            index: index.parse().unwrap(),
            of: of.parse().unwrap(),
            chars,
            start_line: start_line.parse().unwrap(),
            end_line: end_line.parse().unwrap(),
            lines,
            files: vec![file],
        });
    }
    parts.sort_by_key(|p| p.index);
    (parts, whole)
}

/// A committed table over three times the bound, beside a small file: without
/// `--diff` the judge would read the table whole, so it is judged in parts.
fn big_table() -> Project {
    let p = committed_repo(
        TABLE_RULE,
        &[("src/table.md", &table(true)), ("src/small.md", "tiny\n")],
    );
    let content = fs::read_to_string(p.path().join("src/table.md")).unwrap();
    assert!(
        content.len() > 3 * TABLE_BOUND,
        "the file is over the bound"
    );
    p
}

/// Run `lint --format json` over `p` without `--diff`, every prompt logged
/// under `log`, returning the exit code and the report.
fn run_whole(p: &Project, log: &str, verdicts: &str, extra: &[&str]) -> (Option<i32>, Value) {
    let spec = p.write_verdicts(verdicts);
    let out = p
        .lint()
        .args([
            "--format",
            "json",
            "--max-prompt-chars",
            &TABLE_BOUND.to_string(),
        ])
        .args(extra)
        .env("LLMLINT_MOCK_VERDICTS", &spec)
        .env("LLMLINT_MOCK_STATE", p.path().join(format!("{log}-state")))
        .env("LLMLINT_MOCK_PROMPTLOG", p.path().join(log))
        .output()
        .unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// Without `--diff` the judge reads a target file whole, so a file whose content
/// would take it over `max_prompt_chars` is judged in parts of its lines: more
/// than one call, none over the bound, each inlining a consecutive run of the
/// file's real lines numbered as the file numbers them, packed so no two
/// adjacent parts would fit one call — and the file passes when every part does.
/// A file that fits is judged as before, by name, with nothing inlined.
#[test]
fn an_oversized_file_is_judged_in_parts_of_its_lines_without_diff() {
    let p = big_table();
    let (code, report) = run_whole(&p, "log", r#"{"t_rule": true}"#, &[]);
    assert_eq!(code, Some(0), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    let (parts, whole) = lines_parts(&p.path().join("log"));
    assert!(parts.len() > 1, "{} parts", parts.len());
    let content = fs::read_to_string(p.path().join("src/table.md")).unwrap();
    let file_lines: Vec<&str> = content.lines().collect();
    let mut next = 1;
    for (i, part) in parts.iter().enumerate() {
        assert_eq!((part.index, part.of), (i + 1, parts.len()));
        assert_eq!(part.files, vec!["src/table.md".to_string()]);
        assert!(part.chars <= TABLE_BOUND, "a {}-char prompt", part.chars);
        assert_eq!(part.start_line, next, "parts are consecutive");
        assert_eq!(
            part.lines,
            file_lines[part.start_line - 1..part.end_line].to_vec(),
            "part {} shows the file's own lines {}-{}",
            part.index,
            part.start_line,
            part.end_line
        );
        // Packed: one more line would have taken this part over the bound.
        if i + 1 < parts.len() {
            let more = format!("\n{}  {}", part.end_line + 1, file_lines[part.end_line]);
            assert!(
                part.chars + more.chars().count() > TABLE_BOUND,
                "part {} could have taken line {}",
                part.index,
                part.end_line + 1
            );
        }
        next = part.end_line + 1;
    }
    assert_eq!(next, file_lines.len() + 1, "every line is judged once");
    // The small file is one call by name, its content not inlined.
    assert_eq!(whole.len(), 1);
    let small = parse_call(&whole[0]);
    assert_eq!(small.files, vec!["src/small.md".to_string()]);
    assert!(small.chars <= TABLE_BOUND);
    // The parts are one judge's vote: every part passing passes the file.
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "pass", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");

    // The plan names each call's line range, and the same input splits the
    // same way.
    let plan = p
        .lint()
        .args([
            "--plan-only",
            "--max-prompt-chars",
            &TABLE_BOUND.to_string(),
        ])
        .output()
        .unwrap();
    let plan = String::from_utf8_lossy(&plan.stdout);
    let n = parts.len();
    let last = &parts[n - 1];
    assert!(
        plan.contains(&format!(
            "src/table.md (lines {}-{}, part {n} of {n})",
            last.start_line, last.end_line
        )),
        "{plan}"
    );
    run_whole(&p, "again", r#"{"t_rule": true}"#, &[]);
    let ranges = |parts: &[LinesPart]| -> Vec<(usize, usize)> {
        parts.iter().map(|p| (p.start_line, p.end_line)).collect()
    };
    assert_eq!(
        ranges(&lines_parts(&p.path().join("again")).0),
        ranges(&parts)
    );

    // The JSON plan names each call's line range after the small file's whole
    // call, and the persisted record, read back through `llmlint history`, is
    // schema version 2 and keeps them.
    let batch = &report["plan"]["agents"][0]["judges"][0]["batches"][0];
    let planned = batch["parts"].as_array().unwrap();
    assert_eq!(planned.len(), n + 1, "{batch:#}");
    assert_eq!(planned[0]["files"], serde_json::json!(["src/small.md"]));
    assert!(planned[0].get("content_part").is_none(), "{batch:#}");
    for (part, logged) in planned[1..].iter().zip(&parts) {
        assert_eq!(part["files"], serde_json::json!(["src/table.md"]));
        assert_eq!(
            part["content_part"],
            serde_json::json!({
                "index": logged.index,
                "of": n,
                "start_line": logged.start_line,
                "end_line": logged.end_line,
            }),
            "{batch:#}"
        );
        assert!(part.get("diff_part").is_none(), "{batch:#}");
    }
    fs::remove_dir_all(p.history_dir()).unwrap();
    run_whole(&p, "recorded", r#"{"t_rule": true}"#, &[]);
    let out = p
        .bare()
        .args(["history", "latest", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let record: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["schema_version"], 2, "{record:#}");
    assert_eq!(
        record["plan"]["agents"][0]["judges"][0]["batches"][0]["parts"],
        batch["parts"]
    );
}

/// A custom `prompt_template` that only names its files still has an oversized
/// file judged in parts: llmlint appends each part's lines and scope to
/// whatever the template renders, so every prompt stays within the bound, the
/// parts cover every line once, and a rule failing in exactly one part fails the
/// file as one vote.
#[test]
fn a_custom_template_still_judges_an_oversized_file_in_bounded_parts() {
    let p = big_table();
    p.write(
        "names.md",
        "Judge:\n{% for fr in file_rules %}- {{ fr.file }}\n{% endfor %}",
    );
    let template = ["--prompt-template", "names.md", "--max-parallel", "1"];
    let content = fs::read_to_string(p.path().join("src/table.md")).unwrap();
    let file_lines: Vec<&str> = content.lines().collect();

    // Every part passing: several bounded prompts covering every line once.
    let (code, report) = run_whole(&p, "pass", r#"{"t_rule": true}"#, &template);
    assert_eq!(code, Some(0), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    let (parts, whole) = lines_parts(&p.path().join("pass"));
    assert!(parts.len() >= 3, "{} parts", parts.len());
    let mut next = 1;
    for (i, part) in parts.iter().enumerate() {
        assert_eq!((part.index, part.of), (i + 1, parts.len()));
        assert_eq!(part.files, vec!["src/table.md".to_string()]);
        assert!(part.chars <= TABLE_BOUND, "a {}-char prompt", part.chars);
        assert_eq!(part.start_line, next, "parts are consecutive");
        assert_eq!(
            part.lines,
            file_lines[part.start_line - 1..part.end_line].to_vec()
        );
        next = part.end_line + 1;
    }
    assert_eq!(next, file_lines.len() + 1, "every line is judged once");
    // The small file is one call naming only it, nothing appended.
    assert_eq!(whole.len(), 1);
    let small = whole[0]["system"].as_str().unwrap();
    assert_eq!(small, "Judge:\n- src/small.md\n");
    assert_eq!(report["rules"][0]["outcome"], "pass", "{report:#}");
    assert_eq!(report["rules"][0]["votes_total"], 1, "{report:#}");

    // Failing in the table's second part alone (serially the small file's
    // call comes first) fails the file with that part's violation.
    let line = parts[1].start_line + 1;
    let verdicts = format!(
        r#"{{"t_rule": [true, true, {{"holds": false, "violations": [{{"file": "src/table.md", "line": {line}, "message": "bad cell"}}]}}, true]}}"#
    );
    let (code, report) = run_whole(&p, "fail", &verdicts, &template);
    assert_eq!(code, Some(1), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    assert_eq!(lines_parts(&p.path().join("fail")).0.len(), parts.len());
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "fail", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");
    let violations = rule["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report:#}");
    assert_eq!(violations[0]["message"], "bad cell");
    assert_eq!(violations[0]["line"], line);
}

/// A rule failing in exactly one part of an oversized file fails the file, with
/// that part's violation, as one judge's vote.
#[test]
fn a_rule_failing_in_one_part_of_an_oversized_file_fails_it_without_diff() {
    let p = big_table();
    run_whole(&p, "probe", r#"{"t_rule": true}"#, &[]);
    let (parts, _) = lines_parts(&p.path().join("probe"));
    assert!(parts.len() >= 3, "{} parts", parts.len());
    // Serially the small file's call comes first, then the parts in order: the
    // rule fails in the table's second part alone, citing a line it carried.
    let line = parts[1].start_line + 1;
    let verdicts = format!(
        r#"{{"t_rule": [true, true, {{"holds": false, "violations": [{{"file": "src/table.md", "line": {line}, "message": "bad cell"}}]}}, true]}}"#
    );
    let (code, report) = run_whole(&p, "log", &verdicts, &["--max-parallel", "1"]);
    assert_eq!(code, Some(1), "{report:#}");
    assert_eq!(report["errors"], serde_json::json!([]), "{report:#}");
    let (logged, _) = lines_parts(&p.path().join("log"));
    assert_eq!(logged.len(), parts.len());
    let rule = &report["rules"][0];
    assert_eq!(rule["outcome"], "fail", "{report:#}");
    assert_eq!(rule["votes_total"], 1, "{report:#}");
    let violations = rule["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report:#}");
    assert_eq!(violations[0]["message"], "bad cell");
    assert_eq!(violations[0]["line"], line);
}

/// Without `--diff` a file holding one line too long to fit any part beside
/// the prompt's overhead cannot be split at a line boundary, so it is refused by
/// name and never sent, and a small file beside it is still judged.
#[test]
fn a_content_line_too_long_for_any_part_is_refused_by_name_without_diff() {
    let long = format!("short\n{}\nshort\n", "x".repeat(TABLE_BOUND + 1));
    let p = committed_repo(
        TABLE_RULE,
        &[("src/long.md", &long), ("src/small.md", "tiny\n")],
    );
    let (code, report) = run_whole(&p, "log", r#"{"t_rule": true}"#, &[]);
    assert_eq!(code, Some(2), "{report:#}");
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{report:#}");
    let error = errors[0].as_str().unwrap();
    assert!(error.contains("file src/long.md:"), "{error}");
    assert!(error.contains("not sent"), "{error}");
    let (parts, whole) = lines_parts(&p.path().join("log"));
    assert!(parts.is_empty(), "no part of the file was sent");
    assert_eq!(whole.len(), 1);
    assert_eq!(
        parse_call(&whole[0]).files,
        vec!["src/small.md".to_string()]
    );
}

/// Without `--diff` a file's prompt is over the bound only on its fixed
/// overhead (the rules and guidance), which no split of its lines can shrink,
/// so it is refused by name and never sent.
#[test]
fn a_file_over_the_bound_on_overhead_alone_is_refused_without_diff() {
    let p = committed_repo(TABLE_RULE, &[("src/table.md", &table(true))]);
    let log = p.path().join("log");
    let out = p
        .lint()
        .args(["--format", "json", "--max-prompt-chars", "500"])
        .env("LLMLINT_MOCK_PROMPTLOG", &log)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(2), "{report:#}");
    assert!(calls(&log).is_empty(), "nothing reached the harness");
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{report:#}");
    let error = errors[0].as_str().unwrap();
    assert!(error.contains("file src/table.md:"), "{error}");
    assert!(error.contains("not sent"), "{error}");
}
