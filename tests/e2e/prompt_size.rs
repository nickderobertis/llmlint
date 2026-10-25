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
