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
        // Sites: clean sites scored >= 0.70 on dev while violations spread up
        // to ~0.64, so the cut sits between them.
        JevUnit::Sites => 0.65,
        // YAML entries (config meta-rules): violations and clean entries overlap
        // above ~0.15 on dev, so the cut keeps Jev's fails always right and
        // leaves the uncertain middle to the cascade band.
        JevUnit::YamlEntries => 0.15,
        JevUnit::MultiFile => HOLDS_THRESHOLD,
    }
}

/// A conditional rule is relevant when Jev's probability reaches this.
pub const RELEVANT_THRESHOLD: f64 = 0.5;

/// Jev accepts at most this many options per Choice question.
pub const MAX_CHOICE_OPTIONS: usize = 255;

/// Estimated-token budget for one request's `state`. Jev's limit is 32k for the
/// state plus the longest question; the headroom covers the question text and
/// the estimate's slack.
pub const STATE_TOKEN_BUDGET: usize = 24_000;

/// Bytes per token assumed when estimating. Source code tokenizes densely (short
/// identifiers, punctuation, line tags): an 80KB Rust file estimated at 4
/// bytes/token was sent whole and exceeded Jev's limit, so estimate at 3.
pub const BYTES_PER_TOKEN: usize = 3;

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
    /// The site pattern, for `JevUnit::Sites` (validated at config load).
    pub pattern: Option<String>,
    /// The top-level list key, for `JevUnit::YamlEntries`.
    pub key: Option<String>,
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
    /// Per rule, its violation questions — each asks whether something at a
    /// line violates the rule (a doc claim contradicted, a site breaking it) —
    /// with that line. The rule holds as far as its most-violated question.
    /// (`doc_claims` and `sites` requests.)
    violation_questions: BTreeMap<String, Vec<(String, usize)>>,
    /// For a violation question, the Choice question that localizes it within
    /// its unit (a YAML entry): the violation cites the chosen line.
    violation_lines: BTreeMap<String, String>,
    /// For a violation question about another file than the request's own (a
    /// `multi_file` item), that file.
    violation_files: BTreeMap<String, String>,
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
    let budget_bytes = STATE_TOKEN_BUDGET * BYTES_PER_TOKEN;
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
    // `multi_file` rules are asked once over all their files, by the caller.
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
    for rule in rules.iter().filter(|r| r.unit == JevUnit::Sites) {
        out.extend(site_requests(unit, rule));
    }
    // YAML-entry rules sharing a key share each entry's request.
    let mut by_key: BTreeMap<&str, Vec<&JevRule>> = BTreeMap::new();
    for rule in rules.iter().filter(|r| r.unit == JevUnit::YamlEntries) {
        if let Some(key) = rule.key.as_deref() {
            by_key.entry(key).or_default().push(rule);
        }
    }
    for (key, rules) in by_key {
        out.extend(yaml_entry_requests(unit, key, &rules));
    }
    out
}

/// Build the request for a `multi_file` rule: every one of its files in one
/// state (`files`, path -> tagged text). With a pattern, each distinct capture
/// across the files is one violation Noul (cited at its first occurrence);
/// without one, a single violation Noul over the files together. `Err` when
/// the files exceed the state budget — too large for one Jev state, so the
/// caller reports it rather than guessing. `Ok(None)` when a pattern names no
/// item in the files (nothing to ask).
pub fn multi_file_request(
    files: &[(String, String)],
    rule: &JevRule,
) -> Result<Option<Request>, String> {
    let total: usize = files.iter().map(|(_, t)| t.len() / BYTES_PER_TOKEN).sum();
    if total > STATE_TOKEN_BUDGET {
        return Err(format!(
            "rule {}'s files are ~{total} tokens together, over the ~{STATE_TOKEN_BUDGET}-token `multi_file` budget; judge it with the harness (`judge: harness` or `cascade`) or narrow its `files`",
            rule.name
        ));
    }
    let mut state_files = Map::new();
    for (path, text) in files {
        let tagged: String = text
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{}| {l}\n", line_id(i + 1)))
            .collect();
        state_files.insert(path.clone(), json!(tagged));
    }
    let (invariant, false_when) = split_false_when(&rule.description);
    let breaks = |what: &str| match &false_when {
        Some(clause) => format!("{what} breaks `rule`. {clause}"),
        None => format!("{what} breaks `rule`."),
    };
    let mut questions = Map::new();
    let mut ids = Vec::new();
    let mut violation_files = BTreeMap::new();
    match rule
        .pattern
        .as_deref()
        .and_then(|p| regex::Regex::new(p).ok())
    {
        Some(re) => {
            let mut seen = std::collections::BTreeSet::new();
            for (path, text) in files {
                // Matched against the whole text, so a pattern may span lines
                // (a multi-line `__all__ = [...]`); an item's line is where its
                // capture starts.
                for cap in re.captures_iter(text) {
                    let Some(m) = cap.get(1) else { continue };
                    let item = m.as_str().to_string();
                    if !seen.insert(item.clone()) {
                        continue;
                    }
                    let i = text[..m.start()].matches('\n').count();
                    {
                        let id = format!("item:{}:{}", rule.name, ids.len());
                        questions.insert(
                            id.clone(),
                            json!({
                                "type": "noul",
                                "instructions": {
                                    "rule": invariant,
                                    "item": item,
                                    "question": "Judging by the files in `files`, does `item` violate `rule`?",
                                },
                                "criteria": {
                                    "true": breaks("`item`"),
                                    "false": "`item` complies with `rule`, or `rule` does not concern it.",
                                },
                            }),
                        );
                        violation_files.insert(id.clone(), path.clone());
                        ids.push((id, i + 1));
                    }
                }
            }
            if ids.is_empty() {
                // Nothing the pattern names exists in these files: nothing to ask.
                return Ok(None);
            }
        }
        None => {
            let id = format!("files:{}", rule.name);
            questions.insert(
                id.clone(),
                json!({
                    "type": "noul",
                    "instructions": { "rule": invariant, "question": "Do the files in `files`, taken together, violate `rule`?" },
                    "criteria": {
                        "true": breaks("The files"),
                        "false": "The files comply with `rule`.",
                    },
                }),
            );
            ids.push((id, 0));
        }
    }
    if let Some(cond) = &rule.relevance {
        questions.insert(
            format!("relevant:{}", rule.name),
            json!({
                "type": "noul",
                "instructions": { "condition": cond, "question": "Is `condition` true of the files in `files`?" },
            }),
        );
    }
    Ok(Some(Request {
        state: json!({ "files": state_files }),
        questions,
        line_questions: BTreeMap::new(),
        only_line: None,
        asked: vec![rule.name.clone()],
        violation_questions: BTreeMap::from([(rule.name.clone(), ids)]),
        violation_lines: BTreeMap::new(),
        violation_files,
    }))
}

/// A rule's text split into the invariant and its "False when …" clause (the
/// literal definition of a violation), when it has one.
fn split_false_when(description: &str) -> (String, Option<String>) {
    match description.to_lowercase().find("false when") {
        Some(i) => (
            description[..i].trim().to_string(),
            Some(description[i..].trim().to_string()),
        ),
        None => (description.to_string(), None),
    }
}

/// One request per YAML list entry under `key`: the state is only that entry,
/// every rule sharing the key is one violation Noul on it, and a Choice over the
/// entry's lines localizes each. Under `--diff`, only entries the diff touches.
fn yaml_entry_requests(unit: &FileUnit, key: &str, rules: &[&JevRule]) -> Vec<Request> {
    let lines: Vec<&str> = unit.text.lines().collect();
    jevunits::yaml_entries(unit.text, key)
        .into_iter()
        .filter(|(a, b)| match unit.added_lines {
            None => true,
            Some(added) => added.iter().any(|n| (*a..=*b).contains(n)),
        })
        .map(|(a, b)| {
            let entry: String = (a..=b)
                .filter_map(|n| lines.get(n - 1).map(|l| format!("{}| {l}\n", line_id(n))))
                .collect();
            let mut questions = Map::new();
            let mut violation_questions = BTreeMap::new();
            let mut violation_lines = BTreeMap::new();
            for rule in rules {
                let (invariant, false_when) = split_false_when(&rule.description);
                let breaks = match &false_when {
                    Some(clause) => format!("The entry in `entry` breaks `rule`. {clause}"),
                    None => "The entry in `entry` breaks `rule`.".to_string(),
                };
                let id = format!("entry:{}:{a}", rule.name);
                questions.insert(
                    id.clone(),
                    json!({
                        "type": "noul",
                        "instructions": { "rule": invariant, "question": "Does the entry in `entry` violate `rule`?" },
                        "criteria": {
                            "true": breaks,
                            "false": "The entry in `entry` complies with `rule`, or `rule` does not concern it.",
                        },
                    }),
                );
                if b > a {
                    let line_id_q = format!("entryline:{}:{a}", rule.name);
                    let criteria: Map<String, Value> =
                        (a..=b).take(MAX_CHOICE_OPTIONS).map(|n| (line_id(n), Value::Null)).collect();
                    questions.insert(
                        line_id_q.clone(),
                        json!({
                            "type": "choice",
                            "instructions": { "rule": invariant, "question": "Which line of `entry` most directly violates `rule`? Lines are tagged `L<n>|`." },
                            "criteria": criteria,
                        }),
                    );
                    violation_lines.insert(id.clone(), line_id_q);
                }
                if let Some(cond) = &rule.relevance {
                    questions.insert(
                        format!("relevant:{}", rule.name),
                        json!({
                            "type": "noul",
                            "instructions": { "condition": cond, "question": "Is `condition` true of `entry`?" },
                        }),
                    );
                }
                violation_questions.insert(rule.name.clone(), vec![(id, a)]);
            }
            Request {
                state: json!({ "path": unit.path, "entry": entry }),
                questions,
                line_questions: BTreeMap::new(),
                only_line: None,
                asked: rules.iter().map(|r| r.name.clone()).collect(),
                violation_questions,
                violation_lines,
                violation_files: BTreeMap::new(),
            }
        })
        .collect()
}

/// One request per group of sites (the function containing them): the state is
/// that function, and each site is one Noul — does the line at this site
/// violate the rule? Under `--diff`, only sites on added lines are asked.
fn site_requests(unit: &FileUnit, rule: &JevRule) -> Vec<Request> {
    // The pattern was validated at config load; a failure here asks nothing.
    let Some(re) = rule
        .pattern
        .as_deref()
        .and_then(|p| regex::Regex::new(p).ok())
    else {
        return Vec::new();
    };
    let lines: Vec<&str> = unit.text.lines().collect();
    let tag = |n: usize| lines.get(n - 1).map(|l| format!("{}| {l}", line_id(n)));
    jevunits::site_groups(unit.path, unit.text, &re)
        .into_iter()
        .filter_map(|mut g| {
            if let Some(added) = unit.added_lines {
                g.sites.retain(|n| added.contains(n));
            }
            if g.sites.is_empty() {
                return None;
            }
            let code: String = (g.start..=g.end).filter_map(|n| tag(n).map(|l| l + "\n")).collect();
            let helpers: String = g
                .helpers
                .iter()
                .map(|&(a, b)| (a..=b).filter_map(|n| tag(n).map(|l| l + "\n")).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            let use_helpers = !helpers.is_empty();
            // A rule's "False when …" clause, when it has one, is the literal
            // definition of a violation: it goes in the `true` criterion (the
            // site breaks the rule) rather than trailing the invariant.
            let (rule_text, breaks) = match rule.description.to_lowercase().find("false when") {
                Some(i) => (
                    rule.description[..i].trim().to_string(),
                    format!("The line in `site` breaks `rule`. {}", rule.description[i..].trim()),
                ),
                _ => (rule.description.clone(), "The line in `site` breaks `rule`.".to_string()),
            };
            let mut questions = Map::new();
            let mut ids = Vec::new();
            for &n in &g.sites {
                let id = format!("site:{}:{n}", rule.name);
                questions.insert(
                    id.clone(),
                    json!({
                        "type": "noul",
                        "instructions": {
                            "rule": rule_text,
                            "site": tag(n).unwrap_or_default(),
                            "question": if use_helpers {
                                "Does the line in `site` (shown in context in `code`; the functions it calls are in `helpers`) violate `rule`?"
                            } else {
                                "Does the line in `site` (shown in context in `code`) violate `rule`?"
                            },
                        },
                        "criteria": {
                            "true": breaks,
                            "false": "The line in `site` complies with `rule`, or `rule` does not concern it.",
                        },
                    }),
                );
                ids.push((id, n));
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
            let mut state = json!({ "path": unit.path, "code": code });
            if use_helpers {
                state["helpers"] = json!(helpers);
            }
            Some(Request {
                state,
                questions,
                line_questions: BTreeMap::new(),
                only_line: None,
                asked: vec![rule.name.clone()],
                violation_questions: BTreeMap::from([(rule.name.clone(), ids)]),
                violation_lines: BTreeMap::new(),
                violation_files: BTreeMap::new(),
            })
        })
        .collect()
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
            let mut violation_questions = BTreeMap::new();
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
                violation_questions.insert(rule.name.clone(), ids);
            }
            Request {
                state,
                questions,
                line_questions: BTreeMap::new(),
                only_line: None,
                asked: rules.iter().map(|r| r.name.clone()).collect(),
                violation_questions,
                violation_lines: BTreeMap::new(),
                violation_files: BTreeMap::new(),
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
                violation_questions: BTreeMap::new(),
                violation_lines: BTreeMap::new(),
                violation_files: BTreeMap::new(),
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
    /// The file the answer is about, when it is not the request's own file (a
    /// `multi_file` item found in one of several files).
    pub file: Option<String>,
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
        if let Some(claims) = req.violation_questions.get(&rule.name) {
            let mut worst: Option<(f64, usize, Option<String>)> = None;
            for (id, line) in claims {
                // The question asks for a violation (a contradicted claim, a
                // site breaking the rule); the rule holds as far as it doesn't.
                let p = 1.0 - noul(answers, id)?;
                if worst.as_ref().is_none_or(|(w, _, _)| p < *w) {
                    let line = match req.violation_lines.get(id) {
                        Some(choice) => top_line(answers, choice)?.0 as usize,
                        None => *line,
                    };
                    worst = Some((p, line, req.violation_files.get(id).cloned()));
                }
            }
            if let Some((p_holds, line, file)) = worst {
                out.insert(
                    rule.name.clone(),
                    RuleAnswer {
                        p_holds,
                        p_relevant,
                        line: (line > 0).then_some((line as u64, 1.0)),
                        file,
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
                file: None,
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
            pattern: None,
            key: None,
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
                file: None,
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
            file: None,
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
            pattern: None,
            key: None,
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
    fn site_rule(pattern: &str) -> JevRule {
        JevRule {
            name: "ctx".into(),
            description: "Errors carry context. False when an error is propagated bare with `?`."
                .into(),
            relevance: None,
            unit: JevUnit::Sites,
            pattern: Some(pattern.into()),
            key: None,
        }
    }

    const SITES: &str = "\
fn read(p: &str) -> Result<String> {
    fs::read_to_string(p).map_err(|e| ctx(p, e))
}

pub fn load(p: &str) -> Result<u8> {
    let t = read(p)?;
    let n = t.parse()?;
    Ok(n)
}
";

    #[test]
    fn sites_ask_one_violation_per_matched_line_over_its_function() {
        let rules = [site_rule(r"\?")];
        let reqs = build_requests(&file(SITES, None, None), &rules);
        assert_eq!(reqs.len(), 1, "both sites share their function's request");
        let r = &reqs[0];
        assert!(r.state["code"]
            .as_str()
            .unwrap()
            .starts_with("L5| pub fn load("));
        assert_eq!(r.state["helpers"], "L1| fn read(p: &str) -> Result<String> {\nL2|     fs::read_to_string(p).map_err(|e| ctx(p, e))\nL3| }\n");
        assert_eq!(
            r.questions.keys().collect::<Vec<_>>(),
            ["site:ctx:6", "site:ctx:7"]
        );
        let q = &r.questions["site:ctx:6"];
        assert_eq!(q["instructions"]["site"], "L6|     let t = read(p)?;");
        assert_eq!(q["instructions"]["rule"], "Errors carry context.");
        assert!(q["criteria"]["true"]
            .as_str()
            .unwrap()
            .contains("False when an error is propagated bare"));
        let got = read_answers(
            r,
            &rules,
            &answers(json!({"site:ctx:6": {"noul": 0.1}, "site:ctx:7": {"noul": 0.8}})),
        )
        .unwrap();
        assert!((got["ctx"].p_holds - 0.2).abs() < 1e-9);
        assert_eq!(got["ctx"].line, Some((7, 1.0)));
    }

    #[test]
    fn sites_under_diff_ask_only_added_lines_and_a_file_without_sites_asks_nothing() {
        let rules = [site_rule(r"\?")];
        let reqs = build_requests(&file(SITES, Some("d"), Some(&[7])), &rules);
        assert_eq!(reqs[0].questions.keys().collect::<Vec<_>>(), ["site:ctx:7"]);
        assert!(build_requests(&file(SITES, Some("d"), Some(&[2])), &rules).is_empty());
        assert!(build_requests(&file("fn a() {}\n", None, None), &rules).is_empty());
    }

    #[test]
    fn a_sites_rule_fails_below_its_own_threshold() {
        let r = site_rule(r"\?");
        let at = |p: f64| verdict(&r, &[("a.rs".into(), vec![ans(p, None, Some(6))])]);
        assert!(!at(0.6).holds);
        assert!(at(0.7).holds);
    }

    fn entry_rule(name: &str) -> JevRule {
        JevRule {
            name: name.into(),
            description: "Names describe the check. False when a name is a placeholder.".into(),
            relevance: None,
            unit: JevUnit::YamlEntries,
            pattern: None,
            key: Some("rules".into()),
        }
    }

    const CONFIG: &str = "\
version: 1
rules:
  - name: rule1
    description: |
      Every function is documented.
  - { name: fine, description: d }
";

    #[test]
    fn yaml_entries_ask_every_rule_per_entry_and_localize_within_it() {
        let rules = [entry_rule("names"), entry_rule("other")];
        let reqs = build_requests(&file(CONFIG, None, None), &rules);
        assert_eq!(reqs.len(), 2, "one request per entry, shared by both rules");
        let first = &reqs[0];
        assert_eq!(first.state["entry"], "L3|   - name: rule1\nL4|     description: |\nL5|       Every function is documented.\n");
        assert_eq!(
            first.questions.keys().collect::<Vec<_>>(),
            [
                "entry:names:3",
                "entryline:names:3",
                "entry:other:3",
                "entryline:other:3"
            ]
        );
        let q = &first.questions["entry:names:3"];
        assert_eq!(q["instructions"]["rule"], "Names describe the check.");
        assert!(q["criteria"]["true"]
            .as_str()
            .unwrap()
            .ends_with("False when a name is a placeholder."));
        let got = read_answers(
            first,
            &rules,
            &answers(json!({
                "entry:names:3": {"noul": 0.95},
                "entryline:names:3": {"choice": "L3", "probabilities": {"L3": 0.9}},
                "entry:other:3": {"noul": 0.02},
                "entryline:other:3": {"choice": "L5", "probabilities": {"L5": 0.6}},
            })),
        )
        .unwrap();
        assert_eq!(
            got["names"].line,
            Some((3, 1.0)),
            "the chosen line within the entry"
        );
        assert!((got["names"].p_holds - 0.05).abs() < 1e-9);
        // A one-line (flow) entry needs no line question.
        assert_eq!(
            reqs[1].questions.keys().collect::<Vec<_>>(),
            ["entry:names:6", "entry:other:6"]
        );
        assert!(
            !verdict(
                &rules[0],
                &[("c.yml".into(), vec![ans(0.1, None, Some(3))])]
            )
            .holds
        );
        assert!(
            verdict(
                &rules[0],
                &[("c.yml".into(), vec![ans(0.2, None, Some(3))])]
            )
            .holds,
            "threshold 0.15"
        );
    }

    #[test]
    fn yaml_entries_under_diff_ask_only_touched_entries() {
        let rules = [entry_rule("names")];
        assert_eq!(
            build_requests(&file(CONFIG, Some("d"), Some(&[6])), &rules).len(),
            1
        );
        assert!(build_requests(&file(CONFIG, Some("d"), Some(&[1])), &rules).is_empty());
    }
    fn multi_rule(pattern: Option<&str>) -> JevRule {
        JevRule {
            name: "flags".into(),
            description: "Every flag is documented. False when a flag is missing from the README."
                .into(),
            relevance: None,
            unit: JevUnit::MultiFile,
            pattern: pattern.map(str::to_string),
            key: None,
        }
    }

    fn files2() -> Vec<(String, String)> {
        vec![
            ("src/cli.rs".into(), "#[arg(long = \"timeout\")]\n#[arg(long = \"dry-run\")]\n#[arg(long = \"timeout\")]\n".into()),
            ("README.md".into(), "- `--timeout`\n".into()),
        ]
    }

    #[test]
    fn multi_file_puts_every_file_in_one_state_and_asks_each_distinct_item() {
        let rule = multi_rule(Some(r#"long = "([^"]+)""#));
        let req = multi_file_request(&files2(), &rule).unwrap().unwrap();
        assert_eq!(req.state["files"]["README.md"], "L1| - `--timeout`\n");
        assert!(req.state["files"]["src/cli.rs"]
            .as_str()
            .unwrap()
            .starts_with("L1| #[arg("));
        assert_eq!(
            req.questions.keys().collect::<Vec<_>>(),
            ["item:flags:0", "item:flags:1"],
            "`timeout` once"
        );
        assert_eq!(
            req.questions["item:flags:1"]["instructions"]["item"],
            "dry-run"
        );
        let got = read_answers(
            &req,
            std::slice::from_ref(&rule),
            &answers(json!({"item:flags:0": {"noul": 0.05}, "item:flags:1": {"noul": 0.9}})),
        )
        .unwrap();
        let a = &got["flags"];
        assert!((a.p_holds - 0.1).abs() < 1e-9);
        assert_eq!(
            (a.file.as_deref(), a.line),
            (Some("src/cli.rs"), Some((2, 1.0)))
        );
    }

    #[test]
    fn multi_file_without_a_pattern_asks_once_and_rejects_oversized_sets() {
        let rule = multi_rule(None);
        let req = multi_file_request(&files2(), &rule).unwrap().unwrap();
        assert_eq!(req.questions.keys().collect::<Vec<_>>(), ["files:flags"]);
        let got = read_answers(
            &req,
            std::slice::from_ref(&rule),
            &answers(json!({"files:flags": {"noul": 0.8}})),
        )
        .unwrap();
        assert_eq!((got["flags"].line, got["flags"].file.clone()), (None, None));
        let huge = vec![("a.txt".to_string(), "x".repeat(STATE_TOKEN_BUDGET * 5))];
        let e = multi_file_request(&huge, &rule).unwrap_err();
        assert!(
            e.contains("over the") && e.contains("judge: harness"),
            "{e}"
        );
    }

    #[test]
    fn multi_file_patterns_span_lines_and_no_items_asks_nothing() {
        let rule = JevRule {
            pattern: Some(r"__all__\s*=\s*\[([^\]]*)\]".into()),
            ..multi_rule(None)
        };
        let files = vec![(
            "src/api/__init__.py".to_string(),
            "import x\n\n__all__ = [\n    \"a\",\n    \"b\",\n]\n".to_string(),
        )];
        let req = multi_file_request(&files, &rule).unwrap().unwrap();
        let q = &req.questions["item:flags:0"];
        assert!(q["instructions"]["item"]
            .as_str()
            .unwrap()
            .contains("\"b\""));
        let got = read_answers(
            &req,
            std::slice::from_ref(&rule),
            &answers(json!({"item:flags:0": {"noul": 0.9}})),
        )
        .unwrap();
        assert_eq!(
            got["flags"].line,
            Some((3, 1.0)),
            "the line the capture starts on"
        );
        let none = vec![("a.py".to_string(), "nothing here\n".to_string())];
        assert_eq!(multi_file_request(&none, &rule).unwrap(), None);
    }

    #[test]
    fn an_80kb_source_file_is_split_so_each_state_fits() {
        // The shape that overflowed Jev in a real run: ~2000 lines of ~40-byte
        // code lines (80KB) sent as one chunk.
        let line = "    let resolved = cache.lookup(&key)?;\n";
        let text: String = line.repeat(2000);
        let reqs = build_requests(&file(&text, None, None), &[rule("r", None)]);
        assert!(reqs.len() >= 2, "{} chunk(s)", reqs.len());
        for r in &reqs {
            let bytes = r.state["content"].as_str().unwrap().len();
            assert!(
                bytes <= STATE_TOKEN_BUDGET * BYTES_PER_TOKEN,
                "{bytes} bytes in one state"
            );
        }
    }
}
