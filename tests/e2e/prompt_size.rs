//! Prompt-size-bounded batching: `max_prompt_chars` / `--max-prompt-chars`.
//! Each journey drives `llmlint --diff` with the stand-in harness logging every
//! prompt it receives (`LLMLINT_MOCK_PROMPTLOG`), and reads back what each call
//! carried from the rendered prompt itself — its target-file list and each rule's
//! `Scope:` line — so "every rule judged every file once" is checked against what
//! the judge was actually shown.

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
fn a_single_file_over_the_bound_is_an_error_and_is_never_sent() {
    let p = changed_repo("", 2);
    let big: String = (0..400)
        .map(|i| format!("fn a_much_larger_change_{i:04}() {{}}\n"))
        .collect();
    p.write("src/f3.rs", &format!("fn base() {{}}\n{big}"));
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
fn a_zero_bound_is_rejected() {
    let p = changed_repo("max_prompt_chars: 0\n", 1);
    p.lint()
        .arg("--diff")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "max_prompt_chars is 0 (must be >= 1)",
        ));
    p.lint()
        .args(["--diff", "--max-prompt-chars", "0"])
        .assert()
        .code(2);
}
