//! Aggregate independent judges' verdicts for a rule into one outcome.

use std::collections::HashSet;

use crate::domain::verdict::{Outcome, RuleOutcome, RuleVerdict, Violation};

/// Tally the verdicts for a single rule. A rule **passes** only with a strict
/// majority of `holds = true` (a tie fails — conservative for a linter). On a
/// fail, violations from the failing judges are unioned and de-duplicated.
pub fn tally(name: &str, verdicts: &[RuleVerdict]) -> RuleOutcome {
    let total = verdicts.len() as u32;
    let votes_hold = verdicts.iter().filter(|v| v.holds).count() as u32;
    let passes = votes_hold * 2 > total;

    let violations = if passes {
        Vec::new()
    } else {
        dedup(
            verdicts
                .iter()
                .filter(|v| !v.holds)
                .flat_map(|v| v.violations.iter().cloned()),
        )
    };

    RuleOutcome {
        name: name.to_string(),
        rationale: pick_rationale(verdicts, passes),
        outcome: if passes { Outcome::Pass } else { Outcome::Fail },
        votes_total: total,
        votes_hold,
        violations,
    }
}

/// Choose one rationale to represent the winning verdict: prefer a judge that
/// agreed with the outcome (so a pass shows why it held and a fail why it
/// failed), falling back to any non-empty rationale if the majority gave none.
fn pick_rationale(verdicts: &[RuleVerdict], passes: bool) -> Option<String> {
    let non_empty = |v: &RuleVerdict| {
        v.rationale
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    verdicts
        .iter()
        .filter(|v| v.holds == passes)
        .find_map(non_empty)
        .or_else(|| verdicts.iter().find_map(non_empty))
}

fn dedup(items: impl Iterator<Item = Violation>) -> Vec<Violation> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in items {
        if seen.insert(v.dedup_key()) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(holds: bool, msg: &str) -> RuleVerdict {
        RuleVerdict {
            holds,
            violations: if holds {
                vec![]
            } else {
                vec![Violation {
                    message: Some(msg.into()),
                    ..Default::default()
                }]
            },
            rationale: None,
        }
    }

    fn v_why(holds: bool, why: &str) -> RuleVerdict {
        RuleVerdict {
            rationale: Some(why.into()),
            ..v(holds, "bad")
        }
    }

    #[test]
    fn single_pass() {
        let o = tally("r", &[v(true, "")]);
        assert_eq!(o.outcome, Outcome::Pass);
        assert_eq!(o.votes_total, 1);
        assert_eq!(o.votes_hold, 1);
        assert!(o.violations.is_empty());
    }

    #[test]
    fn single_fail_keeps_violation() {
        let o = tally("r", &[v(false, "bad")]);
        assert_eq!(o.outcome, Outcome::Fail);
        assert_eq!(o.violations.len(), 1);
        assert_eq!(o.violations[0].message.as_deref(), Some("bad"));
    }

    #[test]
    fn majority_pass_three_judges() {
        let o = tally("r", &[v(true, ""), v(false, "x"), v(true, "")]);
        assert_eq!(o.outcome, Outcome::Pass);
        assert_eq!(o.votes_hold, 2);
        // Passing rule reports no violations even though a minority dissented.
        assert!(o.violations.is_empty());
    }

    #[test]
    fn majority_fail_three_judges_unions_violations() {
        let o = tally("r", &[v(false, "a"), v(true, ""), v(false, "b")]);
        assert_eq!(o.outcome, Outcome::Fail);
        assert_eq!(o.votes_hold, 1);
        assert_eq!(o.violations.len(), 2);
    }

    #[test]
    fn tie_fails() {
        let o = tally("r", &[v(true, ""), v(false, "x")]);
        assert_eq!(o.outcome, Outcome::Fail);
    }

    #[test]
    fn duplicate_violations_are_deduped() {
        let o = tally("r", &[v(false, "same"), v(false, "same")]);
        assert_eq!(o.violations.len(), 1);
    }

    #[test]
    fn rationale_comes_from_a_judge_that_agreed_with_the_outcome() {
        // Pass: take a holding judge's rationale, not the lone dissenter's.
        let o = tally(
            "r",
            &[
                v_why(true, "complies"),
                v_why(false, "looks off"),
                v_why(true, "ok"),
            ],
        );
        assert_eq!(o.outcome, Outcome::Pass);
        assert_eq!(o.rationale.as_deref(), Some("complies"));

        // Fail: take a dissenting judge's rationale.
        let o = tally(
            "r",
            &[v_why(false, "inline sql at db.rs:42"), v_why(true, "fine")],
        );
        assert_eq!(o.outcome, Outcome::Fail);
        assert_eq!(o.rationale.as_deref(), Some("inline sql at db.rs:42"));
    }

    #[test]
    fn rationale_falls_back_when_the_majority_gave_none_and_is_absent_when_disabled() {
        // Majority passed but only the dissenter explained itself: fall back.
        let o = tally("r", &[v(true, ""), v(true, ""), v_why(false, "stray")]);
        assert_eq!(o.outcome, Outcome::Pass);
        assert_eq!(o.rationale.as_deref(), Some("stray"));
        // No rationales at all (disabled) -> none on the outcome.
        let o = tally("r", &[v(true, ""), v(true, "")]);
        assert_eq!(o.rationale, None);
    }
}
