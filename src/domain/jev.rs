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

use crate::domain::verdict::{JudgeScore, RuleVerdict, Violation};

/// The model llmlint pins by default: thresholds are tuned per version, and an
/// alias (`jev-latest`) can move under a tuned threshold.
pub const DEFAULT_MODEL: &str = "jev-1.13.0";

/// A rule holds when Jev's probability that it holds reaches this. The cascade
/// (a later milestone) replaces the single cut with an escalation band.
pub const HOLDS_THRESHOLD: f64 = 0.5;

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

/// Build the requests for one file: one per chunk, each asking every rule.
pub fn build_requests(unit: &FileUnit, rules: &[JevRule]) -> Vec<Request> {
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
    for rule in rules {
        let p_holds = noul(answers, &format!("holds:{}", rule.name))?;
        let p_relevant = match rule.relevance {
            Some(_) => Some(noul(answers, &format!("relevant:{}", rule.name))?),
            None => None,
        };
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
/// [`HOLDS_THRESHOLD`]; each failing file contributes one violation at the line
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
        if let Some(a) = worst.filter(|a| a.p_holds < HOLDS_THRESHOLD) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, relevance: Option<&str>) -> JevRule {
        JevRule {
            name: name.into(),
            description: format!("{name} holds"),
            relevance: relevance.map(str::to_string),
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
}
