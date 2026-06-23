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
        outcome: if passes { Outcome::Pass } else { Outcome::Fail },
        votes_total: total,
        votes_hold,
        violations,
    }
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
}
