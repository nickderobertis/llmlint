//! Aggregate independent judges' verdicts for a rule into one outcome.

use std::collections::HashSet;

use crate::domain::verdict::{JudgeOpinion, Outcome, RuleOutcome, RuleVerdict, Violation};

/// Tally the verdicts for a single rule. Relevance is decided first: the rule is
/// **not relevant** unless a strict majority of all judges deem it relevant (an
/// always-evaluated rule has every judge implicitly relevant). A relevant rule
/// then **passes** only with a strict majority of `holds = true` among the judges
/// that found it relevant (a tie fails — conservative for a linter); abstaining
/// "not relevant" judges don't vote on the verdict. On a fail, violations from
/// the dissenting relevant judges are unioned and de-duplicated.
pub fn tally(name: &str, verdicts: &[RuleVerdict]) -> RuleOutcome {
    let total = verdicts.len() as u32;
    let votes_relevant = verdicts.iter().filter(|v| v.is_relevant()).count() as u32;
    let votes_hold = verdicts
        .iter()
        .filter(|v| v.is_relevant() && v.holds)
        .count() as u32;

    let relevant = votes_relevant * 2 > total;
    let passes = relevant && votes_hold * 2 > votes_relevant;
    let outcome = if !relevant {
        Outcome::NotRelevant
    } else if passes {
        Outcome::Pass
    } else {
        Outcome::Fail
    };

    let violations = if outcome == Outcome::Fail {
        dedup(
            verdicts
                .iter()
                .filter(|v| v.is_relevant() && !v.holds)
                .flat_map(|v| v.violations.iter().cloned()),
        )
    } else {
        Vec::new()
    };

    // Keep the per-judge breakdown only when more than one judge ran: a single
    // judge is already fully described by the rule's own `rationale`.
    let judges = if total > 1 {
        verdicts
            .iter()
            .map(|v| JudgeOpinion {
                relevant: v.is_relevant(),
                holds: v.holds,
                rationale: clean_rationale(v),
            })
            .collect()
    } else {
        Vec::new()
    };

    RuleOutcome {
        name: name.to_string(),
        rationale: pick_rationale(verdicts, outcome),
        outcome,
        // The held fraction is over the judges that actually voted on the verdict
        // (the relevant ones); for an always-evaluated rule that is every judge.
        votes_total: votes_relevant,
        votes_hold,
        judges,
        violations,
    }
}

/// A judge's rationale, trimmed, or `None` when it is missing or blank.
fn clean_rationale(v: &RuleVerdict) -> Option<String> {
    v.rationale
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Choose one rationale to represent the winning verdict: prefer a judge that
/// landed on the same conclusion (a not-relevant rule shows why it doesn't
/// apply; a pass/fail shows why it held/failed), falling back to any non-empty
/// rationale if the majority gave none.
fn pick_rationale(verdicts: &[RuleVerdict], outcome: Outcome) -> Option<String> {
    let agrees = |v: &&RuleVerdict| match outcome {
        Outcome::NotRelevant => !v.is_relevant(),
        Outcome::Pass => v.is_relevant() && v.holds,
        Outcome::Fail => v.is_relevant() && !v.holds,
        // Skipped/Ignored rules never run a judge, so no verdict can agree.
        Outcome::Skipped | Outcome::Ignored => false,
    };
    verdicts
        .iter()
        .filter(agrees)
        .find_map(clean_rationale)
        .or_else(|| verdicts.iter().find_map(clean_rationale))
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
            relevant: None,
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

    /// A judge that ruled the rule not relevant, with a rationale explaining why.
    fn v_irrelevant(why: &str) -> RuleVerdict {
        RuleVerdict {
            relevant: Some(false),
            rationale: Some(why.into()),
            ..Default::default()
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
    fn multi_judge_keeps_each_judges_opinion_single_judge_does_not() {
        // Three judges -> a per-judge breakdown in verdict order, each carrying
        // its own holds + rationale.
        let o = tally(
            "r",
            &[v_why(false, "j1"), v_why(true, "j2"), v_why(false, "j3")],
        );
        assert_eq!(o.outcome, Outcome::Fail);
        assert_eq!(o.judges.len(), 3);
        assert_eq!(
            o.judges.iter().map(|j| j.holds).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(o.judges[1].rationale.as_deref(), Some("j2"));

        // A single judge needs no breakdown — its rationale is the rule's.
        let o = tally("r", &[v_why(false, "solo")]);
        assert!(o.judges.is_empty());
        assert_eq!(o.rationale.as_deref(), Some("solo"));
    }

    #[test]
    fn single_irrelevant_judge_is_not_relevant_with_its_rationale() {
        let o = tally("r", &[v_irrelevant("change touches no SQL")]);
        assert_eq!(o.outcome, Outcome::NotRelevant);
        assert_eq!(o.votes_hold, 0);
        assert!(o.violations.is_empty());
        assert_eq!(o.rationale.as_deref(), Some("change touches no SQL"));
        // A single judge needs no per-judge breakdown.
        assert!(o.judges.is_empty());
    }

    #[test]
    fn relevance_majority_decides_before_the_verdict() {
        // 2 of 3 say not relevant -> not relevant, even though the lone relevant
        // judge would have failed it.
        let o = tally(
            "r",
            &[
                v_irrelevant("n/a here"),
                v_irrelevant("n/a too"),
                v(false, "would-be violation"),
            ],
        );
        assert_eq!(o.outcome, Outcome::NotRelevant);
        assert!(o.violations.is_empty());
        // The representative rationale comes from a not-relevant judge.
        assert_eq!(o.rationale.as_deref(), Some("n/a here"));
        // The breakdown still records each judge, including relevance.
        assert_eq!(o.judges.len(), 3);
        assert!(!o.judges[0].relevant);
        assert!(o.judges[2].relevant);
    }

    #[test]
    fn relevant_majority_then_tallies_holds_over_the_relevant_judges() {
        // 2 relevant + 1 abstaining: with both relevant judges holding, the rule
        // passes, and the held fraction is over the relevant judges (2/2).
        let o = tally("r", &[v(true, ""), v(true, ""), v_irrelevant("n/a")]);
        assert_eq!(o.outcome, Outcome::Pass);
        assert_eq!((o.votes_hold, o.votes_total), (2, 2));
        // A split among the relevant judges is a tie over them -> fail (the
        // abstainer doesn't vote on the verdict, and contributes no violations).
        let o = tally("r", &[v(true, ""), v(false, "bad"), v_irrelevant("n/a")]);
        assert_eq!(o.outcome, Outcome::Fail);
        assert_eq!((o.votes_hold, o.votes_total), (1, 2));
        assert_eq!(o.violations.len(), 1);
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
