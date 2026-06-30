//! Backstop validation for the `require_line_attribution` rule option.
//!
//! A rule with `require_line_attribution: true` declares that every one of its
//! violations must cite a concrete `file` and `line`. The primary enforcement is
//! the generated schema ([`crate::domain::schema`]) marking each violation's
//! `file`/`line` **required**, so oneharness re-prompts the judge — in one
//! batched turn over the whole verdict object — until every violation is
//! localized. This module is the deterministic backstop on the *final* tallied
//! outcomes: if a failing rule still surfaces a violation without a file+line, it
//! is reported as a hard error instead of a silently-imprecise result.
//!
//! It runs after voting (on [`RuleOutcome`]s), not per judge, so a dissenting
//! judge's unlocalized violation that loses the majority vote never trips it —
//! only violations that actually make it into a failing rule's report count.

use std::collections::BTreeSet;

use crate::domain::verdict::{Outcome, RuleOutcome};

/// One batched error per failing `require_line_attribution` rule whose report
/// still carries a violation without a concrete file+line. `require_attribution`
/// is the set of rule names that opted in. Each message lists *all* of that
/// rule's unlocalized violations together (one error per rule, not per
/// violation), so a re-run sees the whole batch at once.
pub fn unlocalized_errors(
    outcomes: &[RuleOutcome],
    require_attribution: &BTreeSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for o in outcomes {
        if o.outcome != Outcome::Fail || !require_attribution.contains(&o.name) {
            continue;
        }
        let missing: Vec<&str> = o
            .violations
            .iter()
            .filter(|v| v.file.is_none() || v.line.is_none())
            .map(|v| v.message.as_deref().unwrap_or("violation"))
            .collect();
        if missing.is_empty() {
            continue;
        }
        let listed = missing
            .iter()
            .map(|m| format!("{m:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(format!(
            "rule {:?} requires a file and line for every violation, but the judge \
             reported {} without one: {listed} — re-run so each is localized to a file:line",
            o.name,
            count(missing.len(), "violation"),
        ));
    }
    errors
}

/// `1 violation` / `2 violations` — a count with a correctly-pluralized noun.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::verdict::Violation;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn fail(name: &str, violations: Vec<Violation>) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Fail,
            votes_total: 1,
            votes_hold: 0,
            judges: Vec::new(),
            violations,
        }
    }

    fn located(file: &str, line: u64, msg: &str) -> Violation {
        Violation {
            file: Some(file.into()),
            line: Some(line),
            end_line: None,
            message: Some(msg.into()),
        }
    }

    fn unlocated(msg: &str) -> Violation {
        Violation {
            message: Some(msg.into()),
            ..Default::default()
        }
    }

    #[test]
    fn fully_localized_failure_passes() {
        let outcomes = vec![fail("located", vec![located("src/a.rs", 7, "bad")])];
        assert!(unlocalized_errors(&outcomes, &names(&["located"])).is_empty());
    }

    #[test]
    fn an_unlocalized_violation_is_an_error_that_batches_the_messages() {
        let outcomes = vec![fail(
            "located",
            vec![
                located("src/a.rs", 7, "pinned"),
                unlocated("drifted"),
                unlocated("also drifted"),
            ],
        )];
        let errs = unlocalized_errors(&outcomes, &names(&["located"]));
        assert_eq!(errs.len(), 1);
        // One error per rule, naming the rule and listing every unlocalized
        // violation's message (the located one is not named).
        assert!(errs[0].contains("rule \"located\""));
        assert!(errs[0].contains("2 violations"));
        assert!(errs[0].contains("\"drifted\""));
        assert!(errs[0].contains("\"also drifted\""));
        assert!(!errs[0].contains("\"pinned\""));
    }

    #[test]
    fn a_line_without_a_file_still_counts_as_missing() {
        let v = Violation {
            file: None,
            line: Some(3),
            ..Default::default()
        };
        let outcomes = vec![fail("located", vec![v])];
        let errs = unlocalized_errors(&outcomes, &names(&["located"]));
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("1 violation"));
        assert!(!errs[0].contains("1 violations"));
    }

    #[test]
    fn rules_that_did_not_opt_in_are_ignored() {
        let outcomes = vec![fail("free", vec![unlocated("anywhere")])];
        // `free` is not in the require-attribution set, so its unlocalized
        // violation is fine.
        assert!(unlocalized_errors(&outcomes, &names(&["other"])).is_empty());
    }

    #[test]
    fn only_failing_outcomes_are_checked() {
        // A passing/not-relevant/skipped rule has no surfaced violations to
        // attribute, so the check never fires for it even when it opted in.
        let pass = RuleOutcome {
            name: "located".into(),
            rationale: None,
            outcome: Outcome::Pass,
            votes_total: 1,
            votes_hold: 1,
            judges: Vec::new(),
            violations: Vec::new(),
        };
        assert!(unlocalized_errors(&[pass], &names(&["located"])).is_empty());
    }
}
