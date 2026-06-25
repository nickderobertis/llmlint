//! Assemble rule outcomes into a report: human-readable text or stable JSON,
//! plus the process exit code.

use serde_json::{json, Value};

use crate::domain::verdict::{Outcome, RuleOutcome, Violation};

/// The full result of a lint run.
#[derive(Debug, Clone)]
pub struct Report {
    pub outcomes: Vec<RuleOutcome>,
    /// Judge runs that could not produce a usable verdict (oneharness/schema
    /// failures). Their presence makes the run exit `2` (could not complete).
    pub run_errors: Vec<String>,
}

impl Report {
    pub fn new(mut outcomes: Vec<RuleOutcome>, run_errors: Vec<String>) -> Self {
        outcomes.sort_by(|a, b| a.name.cmp(&b.name));
        Report {
            outcomes,
            run_errors,
        }
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut pass = 0;
        let mut fail = 0;
        let mut skip = 0;
        for o in &self.outcomes {
            match o.outcome {
                Outcome::Pass => pass += 1,
                Outcome::Fail => fail += 1,
                Outcome::Skipped => skip += 1,
            }
        }
        (pass, fail, skip)
    }

    /// Exit code: `2` if any judge run errored (incomplete), else `1` if any
    /// rule failed, else `0`.
    pub fn exit_code(&self) -> i32 {
        if !self.run_errors.is_empty() {
            2
        } else if self.outcomes.iter().any(|o| o.outcome == Outcome::Fail) {
            1
        } else {
            0
        }
    }

    /// Render the report for humans at the given verbosity. Level `0` (default)
    /// lists failing rules and their locations; `1`+ additionally itemizes every
    /// passing and skipped rule. Operational errors (a run that couldn't
    /// complete) are surfaced at every level, since they explain a `2` exit the
    /// summary only counts. A blank line separates any per-rule/error detail
    /// from the trailing summary. (The oneharness command/result debug view is
    /// emitted separately to stderr at `-v` by the `lint` command.)
    pub fn to_human(&self, verbosity: u8) -> String {
        let mut out = String::new();
        for o in &self.outcomes {
            match o.outcome {
                // Failures are shown even at the default level — they are the
                // actionable result of a lint run.
                Outcome::Fail => {
                    let votes = if o.votes_total > 1 {
                        format!(" ({}/{} judges held)", o.votes_hold, o.votes_total)
                    } else {
                        String::new()
                    };
                    out.push_str(&format!("FAIL {}{}\n", o.name, votes));
                    // A failure is the actionable result, so its rationale shows
                    // at every level (right after the header, before locations).
                    push_rationale(&mut out, o);
                    for v in &o.violations {
                        out.push_str(&format!("     {}\n", format_violation(v)));
                    }
                }
                // Passing and skipped rules are only itemized at `-v`; at the
                // default level the summary alone accounts for them.
                Outcome::Pass if verbosity >= 1 => {
                    out.push_str(&format!("PASS {}\n", o.name));
                    push_rationale(&mut out, o);
                }
                Outcome::Skipped if verbosity >= 1 => {
                    out.push_str(&format!("SKIP {} (no files matched)\n", o.name))
                }
                Outcome::Pass | Outcome::Skipped => {}
            }
        }
        for e in &self.run_errors {
            out.push_str(&format!("ERROR {e}\n"));
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let (pass, fail, skip) = self.counts();
        out.push_str(&format!(
            "{} rules: {} passed, {} failed, {} skipped",
            self.outcomes.len(),
            pass,
            fail,
            skip
        ));
        if !self.run_errors.is_empty() {
            out.push_str(&format!(", {} errored", self.run_errors.len()));
        }
        out.push('\n');
        out
    }

    pub fn to_json(&self) -> Value {
        let (pass, fail, skip) = self.counts();
        json!({
            "summary": {
                "total": self.outcomes.len(),
                "passed": pass,
                "failed": fail,
                "skipped": skip,
                "errored": self.run_errors.len(),
            },
            "rules": self.outcomes,
            "errors": self.run_errors,
        })
    }
}

/// Append the rule's rationale line, if it has one, indented under its header.
fn push_rationale(out: &mut String, o: &RuleOutcome) {
    if let Some(r) = &o.rationale {
        let r = r.trim();
        if !r.is_empty() {
            out.push_str(&format!("     rationale: {r}\n"));
        }
    }
}

fn format_violation(v: &Violation) -> String {
    let mut loc = String::new();
    if let Some(file) = &v.file {
        loc.push_str(file);
        if let Some(line) = v.line {
            loc.push_str(&format!(":{line}"));
            if let Some(end) = v.end_line {
                loc.push_str(&format!("-{end}"));
            }
        }
    }
    let msg = v.message.as_deref().unwrap_or("violation");
    if loc.is_empty() {
        msg.to_string()
    } else {
        format!("{loc}: {msg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(name: &str, v: Vec<Violation>) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Fail,
            votes_total: 1,
            votes_hold: 0,
            violations: v,
        }
    }
    fn pass(name: &str) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Pass,
            votes_total: 1,
            votes_hold: 1,
            violations: vec![],
        }
    }
    fn with_rationale(mut o: RuleOutcome, why: &str) -> RuleOutcome {
        o.rationale = Some(why.into());
        o
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Report::new(vec![pass("a")], vec![]).exit_code(), 0);
        assert_eq!(Report::new(vec![fail("a", vec![])], vec![]).exit_code(), 1);
        assert_eq!(
            Report::new(vec![pass("a")], vec!["boom".into()]).exit_code(),
            2
        );
    }

    #[test]
    fn default_output_lists_failures_but_not_passes_or_skips() {
        let r = Report::new(
            vec![
                fail(
                    "no_inline_sql",
                    vec![
                        Violation {
                            file: Some("src/db.rs".into()),
                            line: Some(42),
                            end_line: Some(45),
                            message: Some("inline SQL".into()),
                        },
                        Violation {
                            message: Some("architectural drift".into()),
                            ..Default::default()
                        },
                    ],
                ),
                pass("layered"),
                RuleOutcome::skipped("nofiles"),
            ],
            vec![],
        );
        let text = r.to_human(0);
        // Failing rule and its locations are shown at the default level...
        assert!(text.contains("FAIL no_inline_sql"));
        assert!(text.contains("src/db.rs:42-45: inline SQL"));
        assert!(text.contains("architectural drift"));
        // ...but passing/skipped rules are only counted, not itemized.
        assert!(!text.contains("PASS layered"));
        assert!(!text.contains("SKIP nofiles"));
        assert!(text.contains("3 rules: 1 passed, 1 failed, 1 skipped"));
    }

    #[test]
    fn all_passing_default_output_is_just_the_summary() {
        let r = Report::new(vec![pass("a"), pass("b")], vec![]);
        // No failures, default verbosity: a single line, no leading blank line.
        assert_eq!(r.to_human(0), "2 rules: 2 passed, 0 failed, 0 skipped\n");
    }

    #[test]
    fn verbose_itemizes_passing_and_skipped_rules_too() {
        let r = Report::new(
            vec![
                fail(
                    "no_inline_sql",
                    vec![Violation {
                        file: Some("src/db.rs".into()),
                        line: Some(42),
                        end_line: Some(45),
                        message: Some("inline SQL".into()),
                    }],
                ),
                pass("layered"),
                RuleOutcome::skipped("nofiles"),
            ],
            vec![],
        );
        let text = r.to_human(1);
        assert!(text.contains("FAIL no_inline_sql"));
        assert!(text.contains("src/db.rs:42-45: inline SQL"));
        assert!(text.contains("PASS layered"));
        assert!(text.contains("SKIP nofiles (no files matched)"));
        assert!(text.contains("3 rules: 1 passed, 1 failed, 1 skipped"));
    }

    #[test]
    fn vote_split_shows_at_default_errors_at_every_level() {
        let r = Report::new(
            vec![
                RuleOutcome {
                    name: "voted".into(),
                    rationale: None,
                    outcome: Outcome::Fail,
                    votes_total: 3,
                    votes_hold: 1,
                    violations: vec![],
                },
                RuleOutcome::skipped("nofiles"),
            ],
            vec!["judge timed out".into()],
        );
        // Default: the failure (with vote split) and the operational error are
        // both shown; the skipped rule is not itemized.
        let quiet = r.to_human(0);
        assert!(quiet.contains("FAIL voted (1/3 judges held)"));
        assert!(quiet.contains("ERROR judge timed out"));
        assert!(quiet.contains("1 errored"));
        assert!(!quiet.contains("SKIP nofiles"));

        // Verbose itemizes the skipped rule as well.
        let text = r.to_human(1);
        assert!(text.contains("SKIP nofiles (no files matched)"));
    }

    #[test]
    fn rationale_shows_for_failures_at_default_and_for_all_rules_at_verbose() {
        let r = Report::new(
            vec![
                with_rationale(
                    fail(
                        "no_inline_sql",
                        vec![Violation {
                            message: Some("inline SQL".into()),
                            ..Default::default()
                        }],
                    ),
                    "raw SQL string built in db.rs",
                ),
                with_rationale(pass("layered"), "imports only flow downward"),
            ],
            vec![],
        );

        // Default: the failing rule's rationale is shown (before its violation);
        // the passing rule isn't itemized at all, so neither is its rationale.
        let quiet = r.to_human(0);
        assert!(quiet.contains("FAIL no_inline_sql"));
        assert!(quiet.contains("     rationale: raw SQL string built in db.rs"));
        let fail_idx = quiet.find("rationale:").unwrap();
        let viol_idx = quiet.find("inline SQL").unwrap();
        assert!(fail_idx < viol_idx, "rationale precedes the violation");
        assert!(!quiet.contains("imports only flow downward"));

        // Verbose: every evaluated rule shows its rationale.
        let loud = r.to_human(1);
        assert!(loud.contains("PASS layered"));
        assert!(loud.contains("     rationale: imports only flow downward"));
    }

    #[test]
    fn rationale_is_carried_in_json_when_present() {
        let r = Report::new(
            vec![with_rationale(pass("a"), "all good"), fail("b", vec![])],
            vec![],
        );
        let j = r.to_json();
        assert_eq!(j["rules"][0]["rationale"], "all good");
        // A rule with no rationale omits the key entirely.
        assert!(j["rules"][1].get("rationale").is_none());
    }

    #[test]
    fn json_output_is_stable_shape() {
        let r = Report::new(vec![pass("a"), fail("b", vec![])], vec![]);
        let j = r.to_json();
        assert_eq!(j["summary"]["total"], 2);
        assert_eq!(j["summary"]["passed"], 1);
        assert_eq!(j["summary"]["failed"], 1);
        // Outcomes are sorted by name.
        assert_eq!(j["rules"][0]["name"], "a");
        assert_eq!(j["rules"][0]["outcome"], "pass");
        assert_eq!(j["rules"][1]["outcome"], "fail");
    }
}
