//! Verdict types: what a judge returns per rule, and the aggregated outcome.

use serde::{Deserialize, Serialize};

/// A single violation of a rule. Every field is optional: a judge localizes a
/// violation to a `file`/`line` when it can, but some violations (e.g. a
/// cross-cutting architectural drift) cannot be tied to an exact source line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Violation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Violation {
    /// A stable key for de-duplicating identical violations reported by
    /// independent judges.
    pub fn dedup_key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.file.as_deref().unwrap_or(""),
            self.line.map(|l| l.to_string()).unwrap_or_default(),
            self.end_line.map(|l| l.to_string()).unwrap_or_default(),
            self.message.as_deref().unwrap_or(""),
        )
    }
}

/// One judge's decision for one rule, as parsed from oneharness's validated
/// `structured` output. Lenient on extra fields the model may add (e.g. the
/// echoed `name`, which we already know from the map key).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleVerdict {
    /// Whether the rule applies to the change. `None` when the judge was not
    /// asked to decide relevance (an always-evaluated rule); `Some(false)` when
    /// the judge ruled the rule not applicable (and so gave no `holds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevant: Option<bool>,
    /// The verdict. Defaults to `false` (a conservative fail) when omitted, which
    /// only happens for a `relevant=false` verdict — where it is ignored anyway.
    #[serde(default)]
    pub holds: bool,
    #[serde(default)]
    pub violations: Vec<Violation>,
    /// The judge's terse justification, when rationales are enabled for the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

impl RuleVerdict {
    /// Whether this verdict counts as relevant: true unless the judge explicitly
    /// ruled the rule not applicable. An always-evaluated rule (`relevant`
    /// absent) is always relevant.
    pub fn is_relevant(&self) -> bool {
        self.relevant != Some(false)
    }

    /// Combine one judge's verdicts for a rule whose files were split across
    /// several calls (to fit `max_prompt_chars`) into that judge's one vote: the
    /// rule is relevant when any part found it so, holds only when every relevant
    /// part holds, and carries every relevant part's violations. The rationale
    /// joins those of the parts that agree with the combined verdict. A single
    /// part is returned unchanged; `None` for no parts.
    pub fn merge_parts(mut parts: Vec<RuleVerdict>) -> Option<RuleVerdict> {
        if parts.len() <= 1 {
            return parts.pop();
        }
        let relevant = if parts.iter().all(|p| p.relevant.is_none()) {
            None
        } else {
            Some(parts.iter().any(RuleVerdict::is_relevant))
        };
        let judged: Vec<&RuleVerdict> = parts.iter().filter(|p| p.is_relevant()).collect();
        let holds = !judged.is_empty() && judged.iter().all(|p| p.holds);
        let violations = judged
            .iter()
            .flat_map(|p| p.violations.iter().cloned())
            .collect();
        let agrees = |p: &&RuleVerdict| {
            if relevant == Some(false) {
                !p.is_relevant()
            } else {
                p.is_relevant() && p.holds == holds
            }
        };
        let rationales: Vec<&str> = parts
            .iter()
            .filter(agrees)
            .filter_map(|p| p.rationale.as_deref().map(str::trim))
            .filter(|r| !r.is_empty())
            .collect();
        Some(RuleVerdict {
            relevant,
            holds,
            violations,
            rationale: (!rationales.is_empty()).then(|| rationales.join(" ")),
        })
    }
}

/// Final state of a rule after aggregating all of its judges' verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
    /// Every file the rule would cover is suppressed by a whole-file `ignore-file`
    /// directive, so no judge was run. A deliberate, reasoned exemption — reported
    /// distinctly from an incidental `Skipped` (no files matched). Not a violation
    /// — exits clean.
    Ignored,
    /// The rule did not apply to the change (statically `relevance: false`, or a
    /// majority of judges ruled it not relevant). Not a violation — exits clean.
    NotRelevant,
}

/// One judge's opinion, kept per rule so a multi-judge breakdown can show how the
/// individual judges voted and why. Populated only when a rule runs more than one
/// judge (a single judge is fully described by the rule's own `rationale`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JudgeOpinion {
    /// False only when this judge ruled the rule not applicable; omitted from
    /// machine output otherwise (an always-relevant judge). `holds` is moot when
    /// this is false.
    #[serde(skip_serializing_if = "is_relevant")]
    pub relevant: bool,
    pub holds: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

fn is_relevant(relevant: &bool) -> bool {
    *relevant
}

/// The aggregated result for a single rule, ready for reporting. Fields serialize
/// in the same logical order the judge produced them: `name`, then the
/// `rationale`, then the verdict (`outcome` + votes + per-judge breakdown +
/// `violations`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleOutcome {
    pub name: String,
    /// A representative rationale for the winning verdict, when the rule had
    /// rationales enabled. Carried in machine output for auditability and shown
    /// in the human report (always for failures, and for every rule at `-v`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    pub outcome: Outcome,
    pub votes_total: u32,
    pub votes_hold: u32,
    /// Per-judge opinions, populated only for multi-judge rules so the report and
    /// machine output can show each judge's result (and rationale).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub judges: Vec<JudgeOpinion>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<Violation>,
}

impl RuleOutcome {
    pub fn skipped(name: impl Into<String>) -> Self {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Skipped,
            votes_total: 0,
            votes_hold: 0,
            judges: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// A rule whose every matching file is `ignore-file`d away: no judge was run,
    /// and the rationale records why. Neither pass nor fail — exits clean.
    pub fn ignored(name: impl Into<String>) -> Self {
        RuleOutcome {
            name: name.into(),
            rationale: Some("all matching files ignored (ignore-file)".to_string()),
            outcome: Outcome::Ignored,
            votes_total: 0,
            votes_hold: 0,
            judges: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// A rule declared statically not relevant (`relevance: false`): no judge was
    /// run, and the rationale records why it was skipped.
    pub fn not_relevant(name: impl Into<String>) -> Self {
        RuleOutcome {
            name: name.into(),
            rationale: Some("declared not relevant (relevance: false)".to_string()),
            outcome: Outcome::NotRelevant,
            votes_total: 0,
            votes_hold: 0,
            judges: Vec::new(),
            violations: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(relevant: Option<bool>, holds: bool, why: &str) -> RuleVerdict {
        RuleVerdict {
            relevant,
            holds,
            violations: if holds {
                vec![]
            } else {
                vec![Violation {
                    message: Some(why.into()),
                    ..Default::default()
                }]
            },
            rationale: Some(why.into()),
        }
    }

    #[test]
    fn one_part_is_returned_unchanged_and_none_is_none() {
        let p = part(None, false, "x");
        assert_eq!(RuleVerdict::merge_parts(vec![p.clone()]), Some(p));
        assert_eq!(RuleVerdict::merge_parts(vec![]), None);
    }

    #[test]
    fn parts_hold_only_when_every_part_holds() {
        let pass =
            RuleVerdict::merge_parts(vec![part(None, true, "a"), part(None, true, "b")]).unwrap();
        assert!(pass.holds);
        assert_eq!(pass.rationale.as_deref(), Some("a b"));
        let fail =
            RuleVerdict::merge_parts(vec![part(None, true, "a"), part(None, false, "b")]).unwrap();
        assert!(!fail.holds);
        assert_eq!(fail.violations.len(), 1);
        assert_eq!(fail.rationale.as_deref(), Some("b"));
    }

    #[test]
    fn a_rule_is_relevant_when_any_part_finds_it_so() {
        let m = RuleVerdict::merge_parts(vec![
            part(Some(false), false, "n/a"),
            part(Some(true), true, "ok"),
        ])
        .unwrap();
        assert_eq!(m.relevant, Some(true));
        assert!(m.holds);
        let none = RuleVerdict::merge_parts(vec![
            part(Some(false), false, "n/a"),
            part(Some(false), false, "n/a2"),
        ])
        .unwrap();
        assert_eq!(none.relevant, Some(false));
        assert_eq!(none.rationale.as_deref(), Some("n/a n/a2"));
    }
}
