//! Jev (TypeSafe's System One model) as a judge: turn one file and the rules
//! that apply to it into typed questions, and turn the answers back into
//! llmlint verdicts. Pure — the HTTP call lives in [`crate::io::jev`].
//!
//! **Why per file.** Jev evaluates every question against one shared `state`,
//! in parallel and in isolation, so adding a question barely costs anything
//! while a bigger `state` costs accuracy (unrelated content is a distractor) and
//! is capped (32k tokens for the state plus the longest question). So the unit
//! is one file — or one chunk of an oversized file — carrying every applicable
//! rule as questions, the opposite of the harness's file-union batching.
//!
//! **Questions per rule** (ids are for our code; Jev never sees them):
//! - `holds:<rule>` — a Noul: does the file (or, under `--diff`, the change)
//!   satisfy the rule's invariant? Worded positively and literally, since Jev
//!   reads the question as written.
//! - `relevant:<rule>` — a Noul, only for a rule with a `relevance` condition.
//! - `line:<rule>:<k>` — a Choice over the candidate line ids (every line of the
//!   chunk, or only the lines the diff adds), at most [`MAX_CHOICE_OPTIONS`] per
//!   window, asked speculatively so a failing rule can cite its line without a
//!   second round trip.
//!
//! The state tags each line `L<n>|` so a Choice can point at one; the id is the
//! real 1-based line number, so no mapping survives the request.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::domain::config::JevUnit;
use crate::domain::jevunits;
use crate::domain::verdict::{JudgeScore, RuleVerdict, Violation};

/// The model llmlint pins by default: thresholds are tuned per version, and an
/// alias (`jev-latest`) can move under a tuned threshold.
pub const DEFAULT_MODEL: &str = "jev-1.13.0";

/// A rule holds when Jev's probability that it holds reaches this. The cascade
/// (a later milestone) replaces the single cut with an escalation band.
pub const HOLDS_THRESHOLD: f64 = 0.5;

/// The probability below which a rule fails, per unit. A `doc_claims` rule is
/// asked "does the code *contradict* this claim?", a conservative framing whose
/// probabilities run lower than a whole-file "does it hold?": on the eval
/// corpus's dev split, 0.35 kept every catchable violation while admitting the
/// fewest false alarms (re-derive it there, on dev only, when the question
/// changes).
pub fn holds_threshold(unit: JevUnit) -> f64 {
    match unit {
        JevUnit::File => HOLDS_THRESHOLD,
        JevUnit::DocClaims => 0.35,
    }
}

/// A conditional rule is relevant when Jev's probability reaches this.
pub const RELEVANT_THRESHOLD: f64 = 0.5;

/// Jev accepts at most this many options per Choice question.
pub const MAX_CHOICE_OPTIONS: usize = 255;

/// Estimated-token budget for one request's `state`. Jev's limit is 32k for the
/// state plus the longest question; the headroom covers the question text and
/// the estimate's slack (≈ 4 bytes per token).
pub const STATE_TOKEN_BUDGET: usize = 24_000;

/// Input price per million tokens for the models llmlint knows (output is
/// free). `None` for an unknown model, so a report never invents a cost.
pub fn price_per_mtok(model: &str) -> Option<f64> {
    model.starts_with("jev-1.13").then_some(0.042)
}

/// One rule as Jev sees it.
#[derive(Debug, Clone)]
pub struct JevRule {
    pub name: String,
    pub description: String,
    /// The `relevance` condition, when the rule has one.
    pub relevance: Option<String>,
    /// How the rule is decomposed into questions.
    pub unit: JevUnit,
}

/// One file to judge.
#[derive(Debug, Clone)]
pub struct FileUnit<'a> {
    /// Forward-slash path, as the report shows it.
    pub path: &'a str,
    pub text: &'a str,
    /// The file's unified diff under `--diff`; `None` judges the whole file.
    pub diff: Option<&'a str>,
    /// New-file lines the diff adds (`None` without `--diff`). Localization
    /// options are limited to these, since only changed lines are under review.
    pub added_lines: Option<&'a [usize]>,
}

/// One Jev request: a state and its questions, plus what our code needs to read
/// the answers back.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub state: Value,
    pub questions: Map<String, Value>,
    /// Line-choice question ids per rule, in window order.
    line_questions: BTreeMap<String, Vec<String>>,
    /// The only candidate line, when there is exactly one: nothing to ask, so a
    /// failing rule cites it directly (a Choice needs at least two options).
    only_line: Option<usize>,
    /// The rules this request asks about (a unit-specific request asks only
    /// the rules of that unit).
    asked: Vec<String>,
    /// Per rule, its claim questions with the line each claim starts on
    /// (`doc_claims` requests only).
    claim_questions: BTreeMap<String, Vec<(String, usize)>>,
}

fn line_id(n: usize) -> String {
    format!("L{n}")
}

fn parse_line_id(id: &str) -> Option<u64> {
    id.strip_prefix('L')?.parse().ok()
}

/// Split `text` into chunks of whole lines whose estimated tokens fit
/// [`STATE_TOKEN_BUDGET`], as `(first_line, lines)` with 1-based numbering. A
/// single line longer than the budget still forms its own chunk (Jev reports
/// the overflow as a 422, which surfaces as a clear error).
fn chunks(text: &str) -> Vec<(usize, Vec<&str>)> {
    let budget_bytes = STATE_TOKEN_BUDGET * 4;
    let mut out: Vec<(usize, Vec<&str>)> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_bytes = 0;
    let mut start = 1;
    for (i, line) in text.lines().enumerate() {
        let cost = line.len() + 8; // + the `L<n>| ` tag and newline
        if !cur.is_empty() && cur_bytes + cost > budget_bytes {
            out.push((start, std::mem::take(&mut cur)));
            cur_bytes = 0;
            start = i + 1;
        }
        cur.push(line);
        cur_bytes += cost;
    }
    if !cur.is_empty() || out.is_empty() {
        out.push((start, cur));
    }
    out
}

/// Build the requests for one file: whole-file rules share one request per
/// chunk; each unit-specific rule adds requests for its units.
pub fn build_requests(unit: &FileUnit, rules: &[JevRule]) -> Vec<Request> {
    let whole: Vec<JevRule> = rules
        .iter()
        .filter(|r| r.unit == JevUnit::File)
        .cloned()
        .collect();
    let docs: Vec<JevRule> = rules
        .iter()
        .filter(|r| r.unit == JevUnit::DocClaims)
        .cloned()
        .collect();
    let mut out = Vec::new();
    if !whole.is_empty() {
        out.extend(file_requests(unit, &whole));
    }
    if !docs.is_empty() {
        out.extend(doc_claim_requests(unit, &docs));
    }
    out
}

/// One request per documented item (per the extractor in
/// [`jevunits::doc_items`]): the state is only that item's code, and each
/// sentence of its doc is one Noul — is this claim true of the code? Under
/// `--diff`, only items whose doc or code the diff touches are asked.
fn doc_claim_requests(unit: &FileUnit, rules: &[JevRule]) -> Vec<Request> {
    let lines: Vec<&str> = unit.text.lines().collect();
    jevunits::doc_items(unit.path, unit.text)
        .into_iter()
        .filter(|item| !item.claims.is_empty())
        .filter(|item| match unit.added_lines {
            None => true,
            Some(added) => {
                let first = item.claims[0].line.min(item.code_start);
                added.iter().any(|n| (first..=item.code_end).contains(n))
            }
        })
        .map(|item| {
            let code: String = (item.code_start..=item.code_end)
                .filter_map(|n| lines.get(n - 1).map(|l| format!("{}| {l}\n", line_id(n))))
                .collect();
            let tagged = |a: usize, b: usize| -> String {
                (a..=b)
                    .filter_map(|n| lines.get(n - 1).map(|l| format!("{}| {l}\n", line_id(n))))
                    .collect()
            };
            let mut state = json!({ "path": unit.path, "code": code });
            let with_helpers = !item.helpers.is_empty();
            if with_helpers {
                let helpers: String = item
                    .helpers
                    .iter()
                    .map(|&(a, b)| tagged(a, b))
                    .collect::<Vec<_>>()
                    .join("\n");
                state["helpers"] = json!(helpers);
            }
            let mut questions = Map::new();
            let mut claim_questions = BTreeMap::new();
            for rule in rules {
                let mut ids = Vec::new();
                for (k, claim) in item.claims.iter().enumerate() {
                    let id = format!("claim:{}:{k}", rule.name);
                    questions.insert(id.clone(), claim_question(&claim.text, with_helpers));
                    ids.push((id, claim.line));
                }
                if let Some(cond) = &rule.relevance {
                    questions.insert(
                        format!("relevant:{}", rule.name),
                        json!({
                            "type": "noul",
                            "instructions": { "condition": cond, "question": "Is `condition` true of `code`?" },
                        }),
                    );
                }
                claim_questions.insert(rule.name.clone(), ids);
            }
            Request {
                state,
                questions,
                line_questions: BTreeMap::new(),
                only_line: None,
                asked: rules.iter().map(|r| r.name.clone()).collect(),
                claim_questions,
            }
        })
        .collect()
}

/// One claim as a Noul. It asks whether the code *contradicts* the claim rather
/// than whether the claim is *true*: a claim the code cannot confirm either way
/// (a performance note, a caller contract) is not a contradiction, while a
/// literal "is it true?" reads it as false and raises a false alarm. The
/// probability that the rule holds is `1 - p(contradicts)`.
fn claim_question(claim: &str, with_helpers: bool) -> Value {
    let code = if with_helpers {
        "the code in `code` (together with the functions it calls, in `helpers`)"
    } else {
        "the code in `code`"
    };
    json!({
        "type": "noul",
        "instructions": { "claim": claim, "question": format!("Does {code} contradict `claim`?") },
        "criteria": {
            "true": "The code does something `claim` says it does not, or fails to do something `claim` says it does.",
            "false": "Nothing in the code goes against `claim`, including when the code cannot show whether `claim` holds.",
        },
    })
}

/// One request per chunk of the file, each asking every (whole-file) rule.
fn file_requests(unit: &FileUnit, rules: &[JevRule]) -> Vec<Request> {
    let parts = chunks(unit.text);
    let total_lines = unit.text.lines().count();
    let multi = parts.len() > 1;
    parts
        .into_iter()
        .map(|(first, lines)| {
            let last = first + lines.len().saturating_sub(1);
            let content: String = lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{}| {l}\n", line_id(first + i)))
                .collect();
            let mut state = Map::new();
            state.insert("path".into(), json!(unit.path));
            if multi {
                state.insert(
                    "excerpt".into(),
                    json!(format!("lines {first}-{last} of {total_lines}")),
                );
            }
            state.insert("content".into(), json!(content));
            if let Some(diff) = unit.diff {
                state.insert("diff".into(), json!(diff));
            }

            // Candidate lines to localize to: this chunk's lines, narrowed to the
            // diff's added lines when judging a change.
            let candidates: Vec<usize> = match unit.added_lines {
                Some(added) => added
                    .iter()
                    .copied()
                    .filter(|n| (first..=last).contains(n))
                    .collect(),
                None => (first..=last).collect(),
            };

            let mut questions = Map::new();
            let mut line_questions = BTreeMap::new();
            for rule in rules {
                questions.insert(format!("holds:{}", rule.name), holds_question(rule, unit));
                if let Some(cond) = &rule.relevance {
                    questions.insert(
                        format!("relevant:{}", rule.name),
                        relevance_question(cond, unit),
                    );
                }
                let mut ids = Vec::new();
                if candidates.len() >= 2 {
                    for (k, window) in candidates.chunks(MAX_CHOICE_OPTIONS).enumerate() {
                        let id = format!("line:{}:{k}", rule.name);
                        questions.insert(id.clone(), line_question(rule, unit, window));
                        ids.push(id);
                    }
                }
                line_questions.insert(rule.name.clone(), ids);
            }
            Request {
                state: Value::Object(state),
                questions,
                line_questions,
                only_line: match candidates.as_slice() {
                    [n] => Some(*n),
                    _ => None,
                },
                asked: rules.iter().map(|r| r.name.clone()).collect(),
                claim_questions: BTreeMap::new(),
            }
        })
        .collect()
}

fn holds_question(rule: &JevRule, unit: &FileUnit) -> Value {
    let (question, yes, no) = if unit.diff.is_some() {
        (
            "Do the lines that `diff` adds or changes in `content` satisfy `rule`?",
            "No line that `diff` adds or changes violates `rule`.",
            "A line that `diff` adds or changes violates `rule`.",
        )
    } else {
        (
            "Does `content` satisfy `rule`?",
            "Nothing in `content` violates `rule`.",
            "At least one line of `content` violates `rule`.",
        )
    };
    json!({
        "type": "noul",
        "instructions": { "rule": rule.description, "question": question },
        "criteria": { "true": yes, "false": no },
    })
}

fn relevance_question(condition: &str, unit: &FileUnit) -> Value {
    let question = if unit.diff.is_some() {
        "Is `condition` true of the change shown in `diff`?"
    } else {
        "Is `condition` true of `content`?"
    };
    json!({
        "type": "noul",
        "instructions": { "condition": condition, "question": question },
    })
}

fn line_question(rule: &JevRule, unit: &FileUnit, window: &[usize]) -> Value {
    let question = if unit.diff.is_some() {
        "Which line that `diff` adds most directly violates `rule`? Lines are tagged `L<n>|` in `content`."
    } else {
        "Which line of `content` most directly violates `rule`? Lines are tagged `L<n>|`."
    };
    let criteria: Map<String, Value> = window.iter().map(|n| (line_id(*n), Value::Null)).collect();
    json!({
        "type": "choice",
        "instructions": { "rule": rule.description, "question": question },
        "criteria": criteria,
    })
}

/// What Jev said about one rule on one request.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleAnswer {
    pub p_holds: f64,
    /// `None` for an always-evaluated rule.
    pub p_relevant: Option<f64>,
    /// The most likely violating line and its probability, when localized.
    pub line: Option<(u64, f64)>,
}

fn noul(answers: &Map<String, Value>, id: &str) -> Result<f64, String> {
    let a = answers
        .get(id)
        .ok_or_else(|| format!("Jev returned no answer for question {id}"))?;
    let p = a
        .get("noul")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("Jev answer for {id} has no numeric `noul`: {a}"))?;
    if !(0.0..=1.0).contains(&p) {
        return Err(format!("Jev answer for {id} is outside 0..1: {p}"));
    }
    Ok(p)
}

/// The top option of a Choice answer as `(line, probability)`.
fn top_line(answers: &Map<String, Value>, id: &str) -> Result<(u64, f64), String> {
    let a = answers
        .get(id)
        .ok_or_else(|| format!("Jev returned no answer for question {id}"))?;
    let choice = a
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Jev answer for {id} has no `choice`: {a}"))?;
    let line = parse_line_id(choice)
        .ok_or_else(|| format!("Jev answer for {id} chose an unknown option {choice:?}"))?;
    let p = a
        .get("probabilities")
        .and_then(|m| m.get(choice))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Ok((line, p))
}

/// Read every rule's answer from one request's `answers` map.
pub fn read_answers(
    req: &Request,
    rules: &[JevRule],
    answers: &Map<String, Value>,
) -> Result<BTreeMap<String, RuleAnswer>, String> {
    let mut out = BTreeMap::new();
    for rule in rules.iter().filter(|r| req.asked.contains(&r.name)) {
        let p_relevant = match rule.relevance {
            Some(_) => Some(noul(answers, &format!("relevant:{}", rule.name))?),
            None => None,
        };
        // A claims request: the rule holds as far as its least-true claim, which
        // is also where a violation points.
        if let Some(claims) = req.claim_questions.get(&rule.name) {
            let mut worst: Option<(f64, usize)> = None;
            for (id, line) in claims {
                // The question asks for a contradiction; the rule holds as far
                // as the claim is not contradicted.
                let p = 1.0 - noul(answers, id)?;
                if worst.is_none_or(|(w, _)| p < w) {
                    worst = Some((p, *line));
                }
            }
            if let Some((p_holds, line)) = worst {
                out.insert(
                    rule.name.clone(),
                    RuleAnswer {
                        p_holds,
                        p_relevant,
                        line: Some((line as u64, 1.0)),
                    },
                );
            }
            continue;
        }
        let p_holds = noul(answers, &format!("holds:{}", rule.name))?;
        // Across windows, keep the option the model is most sure of.
        let mut line: Option<(u64, f64)> = None;
        for id in req.line_questions.get(&rule.name).into_iter().flatten() {
            let cand = top_line(answers, id)?;
            if line.is_none_or(|(_, best)| cand.1 > best) {
                line = Some(cand);
            }
        }
        if line.is_none() {
            line = req.only_line.map(|n| (n as u64, 1.0));
        }
        out.insert(
            rule.name.clone(),
            RuleAnswer {
                p_holds,
                p_relevant,
                line,
            },
        );
    }
    Ok(out)
}

/// Fold one rule's answers across every request of every file into a verdict.
/// `per_file` pairs each file (slash path) with its chunks' answers.
///
/// A conditional rule is relevant when any chunk is; not-relevant chunks never
/// count toward the verdict. The rule holds when every relevant chunk reaches
/// its unit's [`holds_threshold`]; each failing file contributes one violation at the line
/// of its least-holding chunk (when localized), whose message carries the
/// probability — Jev writes no rationale, so the number *is* the explanation.
pub fn verdict(rule: &JevRule, per_file: &[(String, Vec<RuleAnswer>)]) -> RuleVerdict {
    let relevant = |a: &RuleAnswer| a.p_relevant.is_none_or(|p| p >= RELEVANT_THRESHOLD);
    let any_relevant = per_file.iter().flat_map(|(_, v)| v).any(relevant);
    let mut violations = Vec::new();
    for (file, answers) in per_file {
        let worst = answers
            .iter()
            .filter(|a| relevant(a))
            .min_by(|a, b| a.p_holds.total_cmp(&b.p_holds));
        if let Some(a) = worst.filter(|a| a.p_holds < holds_threshold(rule.unit)) {
            violations.push(Violation {
                file: Some(file.clone()),
                line: a.line.map(|(n, _)| n),
                end_line: None,
                message: Some(format!(
                    "jev: p(holds)={:.2}{}",
                    a.p_holds,
                    a.line
                        .map(|(_, p)| format!(", p(line)={p:.2}"))
                        .unwrap_or_default()
                )),
            });
        }
    }
    // The raw numbers for every file, passing ones included: each file's
    // least-holding relevant chunk (else its least-holding chunk).
    let scores = per_file
        .iter()
        .filter_map(|(file, answers)| {
            let pick = answers
                .iter()
                .filter(|a| relevant(a))
                .min_by(|a, b| a.p_holds.total_cmp(&b.p_holds))
                .or_else(|| {
                    answers
                        .iter()
                        .min_by(|a, b| a.p_holds.total_cmp(&b.p_holds))
                })?;
            Some(JudgeScore {
                file: file.clone(),
                p_holds: pick.p_holds,
                p_relevant: pick.p_relevant,
                line: pick.line.map(|(n, _)| n),
            })
        })
        .collect();
    RuleVerdict {
        relevant: rule.relevance.as_ref().map(|_| any_relevant),
        holds: !any_relevant || violations.is_empty(),
        violations: if any_relevant { violations } else { Vec::new() },
        rationale: None,
        scores,
    }
}

/// The cascade's default escalation band, from the eval sweep (`evals/`): on
/// the labeled corpus every Jev mistake scored inside it, while nothing below
/// 0.10 or at/above 0.95 was ever wrong.
pub const DEFAULT_ESCALATE_LOW: f64 = 0.15;
pub const DEFAULT_ESCALATE_HIGH: f64 = 0.90;

/// Where a cascade trusts Jev: below `low` Jev's fail stands, at or above
/// `high` its pass stands, and in between the harness decides. It must straddle
/// [`HOLDS_THRESHOLD`] so every confident fail is one Jev actually reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub low: f64,
    pub high: f64,
}

impl Band {
    /// The band from optional settings, defaulting each side; an error message
    /// when it is outside 0..1 or does not straddle the pass/fail threshold.
    pub fn from_settings(low: Option<f64>, high: Option<f64>) -> Result<Band, String> {
        let band = Band {
            low: low.unwrap_or(DEFAULT_ESCALATE_LOW),
            high: high.unwrap_or(DEFAULT_ESCALATE_HIGH),
        };
        if !(0.0..=HOLDS_THRESHOLD).contains(&band.low)
            || !(HOLDS_THRESHOLD..=1.0).contains(&band.high)
        {
            return Err(format!(
                "jev.escalate_low ({}) must be in 0..={HOLDS_THRESHOLD} and jev.escalate_high ({}) in {HOLDS_THRESHOLD}..=1",
                band.low, band.high
            ));
        }
        Ok(band)
    }
}

/// A cascade rule's Jev verdict split by confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Triage {
    /// Jev's verdict over the files it decided confidently: violations only
    /// from files below the band (and still standing after ignore cleanup).
    pub decided: RuleVerdict,
    /// Files whose probability fell inside the band, for the harness to judge.
    pub escalate: Vec<String>,
}

/// Split a Jev verdict (with its per-file scores) by the band. A file where a
/// conditional rule is not relevant is decided (not relevant), never escalated.
pub fn triage(v: &RuleVerdict, band: Band) -> Triage {
    let mut escalate = Vec::new();
    let mut failing = std::collections::BTreeSet::new();
    let mut any_decided_relevant = false;
    for s in &v.scores {
        if s.p_relevant.is_some_and(|p| p < RELEVANT_THRESHOLD) {
            continue;
        }
        // A confident fail must be one Jev actually reported: below the band
        // but above the unit's own threshold, it is escalated, not passed.
        let reported = v
            .violations
            .iter()
            .any(|x| x.file.as_deref() == Some(s.file.as_str()));
        if s.p_holds < band.low && reported {
            failing.insert(s.file.as_str());
            any_decided_relevant = true;
        } else if s.p_holds < band.high {
            escalate.push(s.file.clone());
        } else {
            any_decided_relevant = true;
        }
    }
    let violations: Vec<Violation> = v
        .violations
        .iter()
        .filter(|x| x.file.as_deref().is_some_and(|f| failing.contains(f)))
        .cloned()
        .collect();
    Triage {
        decided: RuleVerdict {
            relevant: v.relevant.map(|_| any_decided_relevant),
            holds: violations.is_empty(),
            violations,
            rationale: None,
            scores: v.scores.clone(),
        },
        escalate,
    }
}

/// Combine a cascade rule's confident Jev verdict with one harness verdict on
/// its escalated files: it holds only when both halves hold, and it is
/// relevant when either half found it relevant.
pub fn merge(decided: &RuleVerdict, harness: &RuleVerdict) -> RuleVerdict {
    let harness_counts = harness.is_relevant() && !harness.holds;
    let mut violations = decided.violations.clone();
    if harness_counts {
        violations.extend(harness.violations.iter().cloned());
    }
    RuleVerdict {
        relevant: match (decided.relevant, harness.relevant) {
            (None, None) => None,
            (d, _) => Some(d == Some(true) || harness.is_relevant()),
        },
        holds: decided.violations.is_empty() && !harness_counts,
        violations,
        rationale: harness.rationale.clone(),
        scores: decided.scores.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, relevance: Option<&str>) -> JevRule {
        JevRule {
            name: name.into(),
            description: format!("{name} holds"),
            relevance: relevance.map(str::to_string),
            unit: JevUnit::File,
        }
    }

    fn file<'a>(text: &'a str, diff: Option<&'a str>, added: Option<&'a [usize]>) -> FileUnit<'a> {
        FileUnit {
            path: "src/a.rs",
            text,
            diff,
            added_lines: added,
        }
    }

    #[test]
    fn a_small_file_is_one_request_with_tagged_lines_and_every_question() {
        let rules = [rule("r", None), rule("c", Some("the file parses input"))];
        let reqs = build_requests(&file("fn a() {}\nfn b() {}\n", None, None), &rules);
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        assert_eq!(r.state["path"], "src/a.rs");
        assert_eq!(r.state["content"], "L1| fn a() {}\nL2| fn b() {}\n");
        assert!(r.state.get("diff").is_none() && r.state.get("excerpt").is_none());
        let ids: Vec<&String> = r.questions.keys().collect();
        assert_eq!(
            ids,
            ["holds:r", "line:r:0", "holds:c", "relevant:c", "line:c:0"]
        );
        assert_eq!(r.questions["holds:r"]["type"], "noul");
        assert_eq!(r.questions["holds:r"]["instructions"]["rule"], "r holds");
        assert_eq!(
            r.questions["relevant:c"]["instructions"]["condition"],
            "the file parses input"
        );
        let opts = r.questions["line:r:0"]["criteria"].as_object().unwrap();
        assert_eq!(opts.keys().collect::<Vec<_>>(), ["L1", "L2"]);
    }

    #[test]
    fn a_single_candidate_line_is_cited_without_a_question() {
        let rules = [rule("r", None)];
        let reqs = build_requests(&file("fn a() {}\n", None, None), &rules);
        assert_eq!(reqs[0].questions.keys().collect::<Vec<_>>(), ["holds:r"]);
        let got = read_answers(
            &reqs[0],
            &rules,
            &answers(json!({"holds:r": {"noul": 0.1}})),
        )
        .unwrap();
        assert_eq!(got["r"].line, Some((1, 1.0)));

        // A diff adding one line localizes to it; one adding none cites nothing.
        let one = build_requests(&file("a\nb\n", Some("d"), Some(&[2])), &rules);
        let got =
            read_answers(&one[0], &rules, &answers(json!({"holds:r": {"noul": 0.1}}))).unwrap();
        assert_eq!(got["r"].line, Some((2, 1.0)));
        let none = build_requests(&file("a\nb\n", Some("d"), Some(&[])), &rules);
        let got = read_answers(
            &none[0],
            &rules,
            &answers(json!({"holds:r": {"noul": 0.1}})),
        )
        .unwrap();
        assert_eq!(got["r"].line, None);
    }

    #[test]
    fn diff_mode_ships_the_diff_and_localizes_only_added_lines() {
        let text = "a\nb\nc\nd\n";
        let added = [2usize, 4];
        let reqs = build_requests(
            &file(text, Some("@@ diff @@"), Some(&added)),
            &[rule("r", None)],
        );
        let r = &reqs[0];
        assert_eq!(r.state["diff"], "@@ diff @@");
        let q = r.questions["holds:r"]["instructions"]["question"]
            .as_str()
            .unwrap();
        assert!(q.contains("`diff` adds or changes"), "{q}");
        let opts = r.questions["line:r:0"]["criteria"].as_object().unwrap();
        assert_eq!(opts.keys().collect::<Vec<_>>(), ["L2", "L4"]);
    }

    #[test]
    fn more_than_255_candidates_split_into_windows() {
        let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
        let reqs = build_requests(&file(&text, None, None), &[rule("r", None)]);
        let r = &reqs[0];
        assert_eq!(
            r.questions["line:r:0"]["criteria"]
                .as_object()
                .unwrap()
                .len(),
            255
        );
        assert_eq!(
            r.questions["line:r:1"]["criteria"]
                .as_object()
                .unwrap()
                .len(),
            45
        );
        assert!(r.questions["line:r:1"]["criteria"].get("L256").is_some());
    }

    #[test]
    fn an_oversized_file_splits_into_excerpt_chunks_that_keep_real_line_numbers() {
        let line = "x".repeat(1000);
        let text: String = (0..200).map(|_| format!("{line}\n")).collect();
        let reqs = build_requests(&file(&text, None, None), &[rule("r", None)]);
        assert!(reqs.len() >= 2, "{} chunk(s)", reqs.len());
        let second = &reqs[1];
        let excerpt = second.state["excerpt"].as_str().unwrap();
        assert!(excerpt.ends_with("of 200"), "{excerpt}");
        let first_tag = second.state["content"]
            .as_str()
            .unwrap()
            .split('|')
            .next()
            .unwrap();
        assert_ne!(first_tag, "L1");
        let total: usize = reqs
            .iter()
            .map(|r| r.state["content"].as_str().unwrap().lines().count())
            .sum();
        assert_eq!(total, 200);
    }

    #[test]
    fn an_empty_file_is_still_one_request() {
        assert_eq!(
            build_requests(&file("", None, None), &[rule("r", None)]).len(),
            1
        );
    }

    fn answers(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn answers_read_back_with_the_best_window_line() {
        let text: String = (0..300).map(|i| format!("l{i}\n")).collect();
        let rules = [rule("r", Some("cond"))];
        let req = &build_requests(&file(&text, None, None), &rules)[0];
        let got = read_answers(
            req,
            &rules,
            &answers(json!({
                "holds:r": {"type": "noul", "noul": 0.1},
                "relevant:r": {"type": "noul", "noul": 0.9},
                "line:r:0": {"type": "choice", "choice": "L7", "probabilities": {"L7": 0.4}},
                "line:r:1": {"type": "choice", "choice": "L290", "probabilities": {"L290": 0.8}},
            })),
        )
        .unwrap();
        assert_eq!(
            got["r"],
            RuleAnswer {
                p_holds: 0.1,
                p_relevant: Some(0.9),
                line: Some((290, 0.8)),
            }
        );
    }

    #[test]
    fn malformed_answers_are_errors_not_guesses() {
        let rules = [rule("r", None)];
        let req = &build_requests(&file("a\nb\n", None, None), &rules)[0];
        let bad = [
            (json!({}), "no answer for question holds:r"),
            (json!({"holds:r": {"noul": "yes"}}), "no numeric `noul`"),
            (json!({"holds:r": {"noul": 1.5}}), "outside 0..1"),
            (
                json!({"holds:r": {"noul": 0.2}, "line:r:0": {"choice": "banana"}}),
                "unknown option",
            ),
            (
                json!({"holds:r": {"noul": 0.2}, "line:r:0": {}}),
                "no `choice`",
            ),
        ];
        for (a, want) in bad {
            let err = read_answers(req, &rules, &answers(a)).unwrap_err();
            assert!(err.contains(want), "{err}");
        }
    }

    fn ans(p: f64, rel: Option<f64>, line: Option<u64>) -> RuleAnswer {
        RuleAnswer {
            p_holds: p,
            p_relevant: rel,
            line: line.map(|n| (n, 0.7)),
        }
    }

    #[test]
    fn a_rule_fails_per_file_at_its_least_holding_chunk() {
        let r = rule("r", None);
        let v = verdict(
            &r,
            &[
                (
                    "a.rs".into(),
                    vec![ans(0.9, None, Some(1)), ans(0.2, None, Some(40))],
                ),
                ("b.rs".into(), vec![ans(0.95, None, Some(3))]),
            ],
        );
        assert!(!v.holds);
        assert_eq!(v.relevant, None);
        assert_eq!(v.violations.len(), 1);
        assert_eq!(v.violations[0].file.as_deref(), Some("a.rs"));
        assert_eq!(v.violations[0].line, Some(40));
        assert_eq!(
            v.violations[0].message.as_deref(),
            Some("jev: p(holds)=0.20, p(line)=0.70")
        );
        // Scores cover every file, passing ones too, at the least-holding chunk.
        let scored: Vec<(&str, f64, Option<u64>)> = v
            .scores
            .iter()
            .map(|s| (s.file.as_str(), s.p_holds, s.line))
            .collect();
        assert_eq!(scored, [("a.rs", 0.2, Some(40)), ("b.rs", 0.95, Some(3))]);
    }

    #[test]
    fn a_rule_holds_when_every_chunk_clears_the_threshold() {
        let v = verdict(
            &rule("r", None),
            &[("a.rs".into(), vec![ans(0.5, None, None)])],
        );
        assert!(v.holds && v.violations.is_empty());
    }

    #[test]
    fn a_conditional_rule_ignores_chunks_where_it_is_not_relevant() {
        let r = rule("r", Some("cond"));
        let not_rel = verdict(&r, &[("a.rs".into(), vec![ans(0.1, Some(0.2), Some(3))])]);
        assert_eq!((not_rel.relevant, not_rel.holds), (Some(false), true));
        assert!(not_rel.violations.is_empty());
        let mixed = verdict(
            &r,
            &[
                ("a.rs".into(), vec![ans(0.1, Some(0.2), Some(3))]),
                ("b.rs".into(), vec![ans(0.3, Some(0.9), None)]),
            ],
        );
        assert_eq!((mixed.relevant, mixed.holds), (Some(true), false));
        assert_eq!(mixed.violations.len(), 1);
        assert_eq!(mixed.violations[0].file.as_deref(), Some("b.rs"));
        assert_eq!(mixed.violations[0].line, None);
        assert_eq!(
            mixed.violations[0].message.as_deref(),
            Some("jev: p(holds)=0.30")
        );
    }

    #[test]
    fn prices_are_known_only_for_pinned_models() {
        assert_eq!(price_per_mtok("jev-1.13.0"), Some(0.042));
        assert_eq!(price_per_mtok("jev-latest"), None);
    }
    fn scored(file: &str, p: f64, rel: Option<f64>) -> JudgeScore {
        JudgeScore {
            file: file.into(),
            p_holds: p,
            p_relevant: rel,
            line: Some(1),
        }
    }

    fn jev_verdict(scores: Vec<JudgeScore>, conditional: bool) -> RuleVerdict {
        let violations = scores
            .iter()
            .filter(|s| s.p_holds < HOLDS_THRESHOLD && s.p_relevant.is_none_or(|p| p >= 0.5))
            .map(|s| Violation {
                file: Some(s.file.clone()),
                line: s.line,
                ..Violation::default()
            })
            .collect::<Vec<_>>();
        RuleVerdict {
            relevant: conditional.then_some(true),
            holds: violations.is_empty(),
            violations,
            rationale: None,
            scores,
        }
    }

    const BAND: Band = Band {
        low: 0.15,
        high: 0.9,
    };

    #[test]
    fn triage_keeps_confident_answers_and_escalates_the_middle() {
        let v = jev_verdict(
            vec![
                scored("fail.rs", 0.05, None),
                scored("unsure_fail.rs", 0.3, None),
                scored("unsure_pass.rs", 0.7, None),
                scored("pass.rs", 0.95, None),
            ],
            false,
        );
        let t = triage(&v, BAND);
        assert_eq!(t.escalate, ["unsure_fail.rs", "unsure_pass.rs"]);
        assert!(!t.decided.holds);
        let files: Vec<_> = t
            .decided
            .violations
            .iter()
            .map(|v| v.file.clone().unwrap())
            .collect();
        assert_eq!(files, ["fail.rs"], "an unsure fail is the harness's call");
        assert_eq!(t.decided.scores.len(), 4);
    }

    #[test]
    fn triage_never_escalates_a_file_where_the_rule_is_not_relevant() {
        let v = jev_verdict(
            vec![
                scored("a.rs", 0.5, Some(0.1)),
                scored("b.rs", 0.97, Some(0.9)),
            ],
            true,
        );
        let t = triage(&v, BAND);
        assert!(t.escalate.is_empty());
        assert_eq!((t.decided.relevant, t.decided.holds), (Some(true), true));
        let none = triage(
            &jev_verdict(vec![scored("a.rs", 0.5, Some(0.1))], true),
            BAND,
        );
        assert_eq!(none.decided.relevant, Some(false));
    }

    #[test]
    fn merge_needs_both_halves_to_hold() {
        let decided = triage(&jev_verdict(vec![scored("ok.rs", 0.99, None)], false), BAND).decided;
        let harness_fail = RuleVerdict {
            holds: false,
            violations: vec![Violation {
                file: Some("unsure.rs".into()),
                line: Some(4),
                ..Violation::default()
            }],
            rationale: Some("bare ?".into()),
            ..RuleVerdict::default()
        };
        let m = merge(&decided, &harness_fail);
        assert!(!m.holds);
        assert_eq!(m.violations[0].file.as_deref(), Some("unsure.rs"));
        assert_eq!(m.rationale.as_deref(), Some("bare ?"));
        assert_eq!(m.scores.len(), 1);

        let harness_pass = RuleVerdict {
            holds: true,
            ..RuleVerdict::default()
        };
        assert!(merge(&decided, &harness_pass).holds);
        let jev_fail = triage(
            &jev_verdict(vec![scored("bad.rs", 0.02, None)], false),
            BAND,
        )
        .decided;
        let m = merge(&jev_fail, &harness_pass);
        assert!(!m.holds && m.violations.len() == 1);
    }

    #[test]
    fn merge_is_relevant_when_either_half_is() {
        let decided = triage(
            &jev_verdict(vec![scored("a.rs", 0.5, Some(0.1))], true),
            BAND,
        )
        .decided;
        let not_rel = RuleVerdict {
            relevant: Some(false),
            holds: true,
            ..RuleVerdict::default()
        };
        assert_eq!(merge(&decided, &not_rel).relevant, Some(false));
        let rel_fail = RuleVerdict {
            relevant: Some(true),
            holds: false,
            ..RuleVerdict::default()
        };
        let m = merge(&decided, &rel_fail);
        assert_eq!((m.relevant, m.holds), (Some(true), false));
        // An irrelevant harness half never contributes a failure.
        let irrelevant_fail = RuleVerdict {
            relevant: Some(false),
            holds: false,
            ..RuleVerdict::default()
        };
        assert!(merge(&decided, &irrelevant_fail).holds);
    }

    #[test]
    fn the_band_defaults_and_must_straddle_the_threshold() {
        assert_eq!(Band::from_settings(None, None).unwrap(), BAND);
        assert_eq!(
            Band::from_settings(Some(0.3), Some(0.7)).unwrap(),
            Band {
                low: 0.3,
                high: 0.7
            }
        );
        for (lo, hi) in [(0.6, 0.9), (0.1, 0.4), (-0.1, 0.9), (0.1, 1.2)] {
            let e = Band::from_settings(Some(lo), Some(hi)).unwrap_err();
            assert!(e.contains("jev.escalate_low"), "{e}");
        }
    }
    fn doc_rule(name: &str) -> JevRule {
        JevRule {
            name: name.into(),
            description: "docs match behavior".into(),
            relevance: None,
            unit: JevUnit::DocClaims,
        }
    }

    const DOCUMENTED: &str = "\
fn helper() -> u8 { 1 }

/// Sort cheapest first. Never panics.
pub fn sort(v: &mut [u8]) {
    helper();
    v.sort();
}

fn undocumented() {}
";

    #[test]
    fn doc_claims_ask_one_contradiction_per_claim_over_the_item_and_its_helpers() {
        let rules = [doc_rule("docs"), rule("whole", None)];
        let reqs = build_requests(&file(DOCUMENTED, None, None), &rules);
        assert_eq!(
            reqs.len(),
            2,
            "one whole-file request + one per documented item"
        );
        let item = &reqs[1];
        assert_eq!(
            item.state["code"],
            "L4| pub fn sort(v: &mut [u8]) {\nL5|     helper();\nL6|     v.sort();\nL7| }\n"
        );
        assert_eq!(item.state["helpers"], "L1| fn helper() -> u8 { 1 }\n");
        assert_eq!(
            item.questions.keys().collect::<Vec<_>>(),
            ["claim:docs:0", "claim:docs:1"]
        );
        let q = &item.questions["claim:docs:0"];
        assert_eq!(q["instructions"]["claim"], "Sort cheapest first.");
        assert!(q["instructions"]["question"]
            .as_str()
            .unwrap()
            .contains("contradict"));
        assert!(q["instructions"]["question"]
            .as_str()
            .unwrap()
            .contains("`helpers`"));
        // The whole-file request never asks the doc rule.
        assert!(reqs[0].questions.keys().all(|k| !k.contains("docs")));

        // The rule holds as far as its most-contradicted claim, cited at its line.
        let got = read_answers(
            item,
            &rules,
            &answers(json!({
                "claim:docs:0": {"noul": 0.9},
                "claim:docs:1": {"noul": 0.1},
            })),
        )
        .unwrap();
        assert_eq!(got.len(), 1, "only the rule this request asked");
        let a = &got["docs"];
        assert!((a.p_holds - 0.1).abs() < 1e-9);
        assert_eq!(a.line, Some((3, 1.0)));
    }

    #[test]
    fn doc_claims_under_diff_ask_only_items_the_change_touches() {
        let rules = [doc_rule("docs")];
        let untouched = build_requests(&file(DOCUMENTED, Some("d"), Some(&[9])), &rules);
        assert!(
            untouched.is_empty(),
            "the change only touched an undocumented fn"
        );
        let touched = build_requests(&file(DOCUMENTED, Some("d"), Some(&[6])), &rules);
        assert_eq!(touched.len(), 1);
        let doc_edit = build_requests(&file(DOCUMENTED, Some("d"), Some(&[3])), &rules);
        assert_eq!(doc_edit.len(), 1, "editing the doc itself counts");
    }

    #[test]
    fn doc_claims_fail_below_their_own_threshold() {
        let r = doc_rule("docs");
        let at = |p: f64| verdict(&r, &[("a.rs".into(), vec![ans(p, None, Some(3))])]);
        assert!(!at(0.30).holds, "below the doc_claims threshold");
        assert!(
            at(0.40).holds,
            "a whole-file rule would fail at 0.40; a claims rule does not"
        );
        assert!(
            !verdict(
                &rule("w", None),
                &[("a.rs".into(), vec![ans(0.40, None, Some(3))])]
            )
            .holds
        );
    }

    #[test]
    fn triage_escalates_a_below_band_file_that_reported_no_violation() {
        // Below the band (0.3 < 0.4) but above the claims threshold, so the
        // verdict carries no violation for it: escalate, never silently pass.
        let v = verdict(
            &doc_rule("docs"),
            &[("a.rs".into(), vec![ans(0.38, None, Some(3))])],
        );
        assert!(v.holds);
        let t = triage(
            &v,
            Band {
                low: 0.4,
                high: 0.9,
            },
        );
        assert_eq!(t.escalate, ["a.rs"]);
    }
}
