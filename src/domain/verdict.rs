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
    pub holds: bool,
    #[serde(default)]
    pub violations: Vec<Violation>,
    /// The judge's terse justification, when rationales are enabled for the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// Final state of a rule after aggregating all of its judges' verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
}

/// The aggregated result for a single rule, ready for reporting. Fields serialize
/// in the same logical order the judge produced them: `name`, then the
/// `rationale`, then the verdict (`outcome` + votes + `violations`).
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
            violations: Vec::new(),
        }
    }
}
