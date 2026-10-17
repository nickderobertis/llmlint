//! A judge reply answers exactly the rules its call selected. Each journey drives
//! `llmlint --diff` over two selected rules — the workload at which llmlint
//! 0.4.10 exited 0 on every one of these replies — with the stand-in harness
//! scripted (`LLMLINT_MOCK_REPLIES`) to return a schema-valid reply that answers
//! fewer, or other, rules than the call asked about.

// llmlint: ignore-file[e2e_not_mocked] the stand-in oneharness is this suite's double for the oneharness subprocess, the one genuinely external boundary (AGENTS.md); a reply that answers fewer or other rules than its call selected can only be scripted at that seam, and the journeys still drive the real freshly built llmlint binary over real git repos

use std::fs;

use super::*;

const TWO_RULES: &str = "  - { name: a_rule, description: \"true when ok; false otherwise.\" }\n  \
     - { name: b_rule, description: \"true when ok; false otherwise.\" }\n";

/// A committed repo with two rules over `src/**` and one uncommitted change, so
/// `--diff` selects both rules over one changed file.
fn changed_repo(rules: &str) -> Project {
    let p = committed_repo(rules, &[("src/lib.rs", "fn a() {}\n")]);
    p.write("src/lib.rs", "fn a() {}\nfn b() {}\n");
    p
}

/// Run `llmlint --diff` with the mock answering each `run` call with the next
/// element of `replies` (the last repeating).
fn run_with_replies(p: &Project, replies: &str) -> std::process::Output {
    let path = p.path().join("replies.json");
    fs::write(&path, replies).unwrap();
    p.lint()
        .arg("--diff")
        .env("LLMLINT_MOCK_REPLIES", &path)
        .output()
        .unwrap()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn an_empty_reply_is_an_error_naming_every_selected_rule() {
    let p = changed_repo(TWO_RULES);
    let out = run_with_replies(&p, "[{}]");
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    for rule in ["a_rule", "b_rule"] {
        assert!(
            text.contains(&format!(
                "rule {rule}: the judge returned no verdict for it"
            )),
            "{text}"
        );
    }
    assert!(text.contains("2 errored"), "{text}");
}

#[test]
fn a_reply_holding_one_of_two_rules_is_an_error_naming_the_other() {
    let p = changed_repo(TWO_RULES);
    let out = run_with_replies(&p, r#"[{"a_rule": {"holds": true}}]"#);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(
        text.contains("rule b_rule: the judge returned no verdict for it"),
        "{text}"
    );
    assert!(
        !text.contains("rule a_rule: the judge returned no verdict"),
        "{text}"
    );
}

#[test]
fn a_reply_naming_only_an_unselected_rule_counts_nothing_and_names_all_three() {
    let p = changed_repo(TWO_RULES);
    let out = run_with_replies(&p, r#"[{"ghost_rule": {"holds": true}}]"#);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    for rule in ["a_rule", "b_rule"] {
        assert!(
            text.contains(&format!(
                "rule {rule}: the judge returned no verdict for it"
            )),
            "{text}"
        );
    }
    assert!(
        text.contains(
            "returned a verdict for rule ghost_rule, which this judge call did not select"
        ),
        "{text}"
    );
    // The stray verdict is dropped, never tallied as a pass.
    assert!(text.contains("0 rules: 0 passed"), "{text}");
}

#[test]
fn with_three_judges_only_the_first_answering_is_an_error_for_the_others() {
    let rules =
        "  - { name: a_rule, judges: 3, description: \"true when ok; false otherwise.\" }\n  \
         - { name: b_rule, judges: 3, description: \"true when ok; false otherwise.\" }\n";
    let p = changed_repo(rules);
    let out = run_with_replies(
        &p,
        r#"[{"a_rule": {"holds": true}, "b_rule": {"holds": true}}, {}]"#,
    );
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    // Two of the three calls answered nothing: each names both rules.
    for rule in ["a_rule", "b_rule"] {
        let n = text
            .matches(&format!(
                "rule {rule}: the judge returned no verdict for it"
            ))
            .count();
        assert_eq!(
            n, 2,
            "{rule} should be unanswered by two judge calls:\n{text}"
        );
    }
}

#[test]
fn a_reply_answering_both_rules_still_exits_on_its_verdicts() {
    let p = changed_repo(TWO_RULES);
    let pass = run_with_replies(
        &p,
        r#"[{"a_rule": {"holds": true}, "b_rule": {"holds": true}}]"#,
    );
    assert_eq!(pass.status.code(), Some(0), "{}", stdout(&pass));
    assert!(stdout(&pass).contains("2 rules: 2 passed, 0 failed"));

    let p = changed_repo(TWO_RULES);
    let fail = run_with_replies(
        &p,
        r#"[{"a_rule": {"holds": true}, "b_rule": {"holds": false, "violations": []}}]"#,
    );
    let text = stdout(&fail);
    assert_eq!(fail.status.code(), Some(1), "{text}");
    assert!(text.contains("FAIL b_rule"), "{text}");
    assert!(!text.contains("ERROR"), "{text}");
}
