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
}

/// Final state of a rule after aggregating all of its judges' verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
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
