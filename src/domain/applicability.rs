//! Per-file rule applicability: which rules a judge should evaluate against each
//! target file, the most token-efficient way to say so in the prompt, and the
//! validation that rejects (and repairs) a verdict that strays outside a rule's
//! files.
//!
//! With nested/cascading configs and per-rule/agent `files`, one judge call can
//! cover a union of files where different rules apply to different files. This
//! module is the pure core that (1) tells the judge, per file, exactly which
//! rules apply — picking the shorter of an apply-list or a skip-list so the
//! context stays cheap — and (2) after the judge answers, finds violations
//! pinned to a file outside the offending rule's scope (the "wrong rule in wrong
//! file" case) so the caller can ask for a rework and, failing that, drop them.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::domain::verdict::RuleVerdict;

/// Whether a file's rule line lists the rules that *apply* (`Include`) or the
/// rules to *skip* (`Exclude`) — chosen per file by whichever spelling is shorter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Include,
    Exclude,
}

/// One file's applicability line for the prompt: the file, whether `rules` is the
/// apply-list or the skip-list, and the (possibly empty) rule names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileRules {
    pub file: String,
    pub mode: Mode,
    pub rules: Vec<String>,
}

/// A wrong-file report: the judge flagged `rule` in `file`, but the rule's scope
/// does not cover that file. Drives the corrective rework prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeProblem {
    pub rule: String,
    pub file: String,
}

/// Normalize a path the way both sides of an applicability match must agree on:
/// trim, forward-slash separators, and drop a leading `./` so a judge's
/// `./src/a.rs` matches the resolved `src/a.rs`.
pub fn norm(p: &str) -> String {
    p.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

/// The comma-joined character length of a name list — the token proxy used to
/// pick the cheaper of the apply/skip spellings. Empty lists cost nothing.
fn joined_len(names: &[String]) -> usize {
    if names.is_empty() {
        0
    } else {
        names.iter().map(String::len).sum::<usize>() + 2 * (names.len() - 1)
    }
}

/// Compute the per-file applicability lines for a judge call. `rules` pairs each
/// rule name (in the batch's order) with the slash-relative files it applies to;
/// `files` is the call's full file union. For each file, the applicable rules are
/// those whose file list contains it; we render whichever of the apply-list or
/// the skip-list is shorter (ties — including "every rule applies", where the
/// skip-list is empty and cheapest — resolve to the shorter spelling).
pub fn per_file(rules: &[(String, Vec<String>)], files: &[String]) -> Vec<FileRules> {
    files
        .iter()
        .map(|f| {
            let target = norm(f);
            let mut applies = Vec::new();
            let mut excluded = Vec::new();
            for (name, rule_files) in rules {
                if rule_files.iter().any(|rf| norm(rf) == target) {
                    applies.push(name.clone());
                } else {
                    excluded.push(name.clone());
                }
            }
            if joined_len(&applies) <= joined_len(&excluded) {
                FileRules {
                    file: f.clone(),
                    mode: Mode::Include,
                    rules: applies,
                }
            } else {
                FileRules {
                    file: f.clone(),
                    mode: Mode::Exclude,
                    rules: excluded,
                }
            }
        })
        .collect()
}

/// Build the rule → normalized-files lookup used by scope validation and
/// cleaning, from the same `(name, files)` pairs `per_file` consumes.
pub fn scope_map(rules: &[(String, Vec<String>)]) -> BTreeMap<String, BTreeSet<String>> {
    rules
        .iter()
        .map(|(name, files)| (name.clone(), files.iter().map(|f| norm(f)).collect()))
        .collect()
}

/// Find every violation whose file lies outside its rule's scope, across a
/// judge's verdicts. `scope` maps each rule to its normalized files (see
/// [`scope_map`]). File-less (cross-cutting) violations can't be mislocated, so
/// they never count. Results are de-duplicated and ordered for a stable prompt.
pub fn scope_problems(
    scope: &BTreeMap<String, BTreeSet<String>>,
    verdicts: &BTreeMap<String, RuleVerdict>,
) -> Vec<ScopeProblem> {
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for (rule, verdict) in verdicts {
        let Some(files) = scope.get(rule) else {
            continue;
        };
        for v in &verdict.violations {
            if let Some(file) = &v.file {
                let nf = norm(file);
                if !files.contains(&nf) {
                    seen.insert((rule.clone(), nf));
                }
            }
        }
    }
    seen.into_iter()
        .map(|(rule, file)| ScopeProblem { rule, file })
        .collect()
}

/// Clean one rule's verdict against its scope and the inline-ignore suppressions:
/// drop any violation outside the rule's files, drop any violation an ignore
/// directive covers, and — when the only basis for a `holds=false` was dropped —
/// flip it to a pass so a wrong-file or ignored violation can never turn the
/// build red. `is_suppressed(file, line)` reports whether an inline ignore for
/// *this* rule covers a violation at that (normalized) file and line.
///
/// Returns `true` when a wrong-file (out-of-scope) violation was dropped, so the
/// caller can decide whether a rework is still warranted.
pub fn clean_verdict(
    verdict: &mut RuleVerdict,
    scope: &BTreeSet<String>,
    is_suppressed: impl Fn(&str, Option<u64>) -> bool,
) -> bool {
    let had_violations = !verdict.violations.is_empty();
    let mut dropped_out_of_scope = false;
    let mut dropped_any = false;
    verdict.violations.retain(|v| {
        let Some(file) = &v.file else {
            // A cross-cutting violation isn't pinned to a file: keep it.
            return true;
        };
        let nf = norm(file);
        let in_scope = scope.contains(&nf);
        let suppressed = is_suppressed(&nf, v.line);
        let keep = in_scope && !suppressed;
        if !keep {
            dropped_any = true;
            if !in_scope {
                dropped_out_of_scope = true;
            }
        }
        keep
    });
    // Flip a fail to a pass only when its entire basis was removed: the rule is
    // relevant, it had violations, all are gone, and at least one was dropped
    // (rather than the judge having reported `holds=false` with no violations).
    if !verdict.holds
        && verdict.is_relevant()
        && had_violations
        && verdict.violations.is_empty()
        && dropped_any
    {
        verdict.holds = true;
    }
    dropped_out_of_scope
}

/// Render one file's applicability as a prompt line, e.g.
/// `- src/a.rs — only these rules apply: rule_a, rule_b` or
/// `- docs/x.md — all rules apply except: rule_a`. Shared so the system prompt
/// (via the template) and the rework prompt describe scope the same way.
pub fn file_rule_line(fr: &FileRules) -> String {
    let names = fr.rules.join(", ");
    match fr.mode {
        Mode::Include if fr.rules.is_empty() => format!("- {} — no rules apply", fr.file),
        Mode::Include => format!("- {} — only these rules apply: {names}", fr.file),
        Mode::Exclude if fr.rules.is_empty() => format!("- {} — all rules apply", fr.file),
        Mode::Exclude => format!("- {} — all rules apply except: {names}", fr.file),
    }
}

/// Build the corrective user prompt for a rework round: name the wrong-file
/// violations the judge reported, restate which rules apply to each file, and ask
/// for a corrected verdict. `file_rules` is the same per-file applicability shown
/// in the system prompt.
pub fn rework_prompt(problems: &[ScopeProblem], file_rules: &[FileRules]) -> String {
    let mut out = String::new();
    out.push_str(
        "Your previous verdict reported rule violations in files that those rules do not \
         cover. A rule must be evaluated ONLY against the files it applies to.\n\n\
         Wrong-file violations to remove:\n",
    );
    for p in problems {
        out.push_str(&format!(
            "- rule `{}` reported a violation in `{}`, but `{}` does not apply to `{}`\n",
            p.rule, p.file, p.rule, p.file
        ));
    }
    out.push_str("\nThe rules that apply to each target file are:\n");
    for fr in file_rules {
        out.push_str(&file_rule_line(fr));
        out.push('\n');
    }
    out.push_str(
        "\nRe-review every file with the correct scope and respond again with the full \
         structured verdict object. Do not report a violation for a rule in a file it does \
         not apply to. Respond with only the JSON object.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::verdict::Violation;

    fn rules(pairs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        pairs
            .iter()
            .map(|(n, fs)| (n.to_string(), fs.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    fn viol(file: Option<&str>, line: Option<u64>) -> Violation {
        Violation {
            file: file.map(Into::into),
            line,
            end_line: None,
            message: Some("x".into()),
        }
    }

    #[test]
    fn per_file_picks_the_shorter_apply_or_skip_list() {
        // f1: only a applies (1 of 3) -> include [a]; f2: a,b,c all apply -> the
        // skip-list is empty and cheapest -> exclude [].
        let rs = rules(&[("a", &["f1", "f2"]), ("b", &["f2"]), ("c", &["f2"])]);
        let out = per_file(&rs, &["f1".into(), "f2".into()]);
        assert_eq!(out[0].mode, Mode::Include);
        assert_eq!(out[0].rules, vec!["a"]);
        assert_eq!(out[1].mode, Mode::Exclude);
        assert!(out[1].rules.is_empty());
    }

    #[test]
    fn per_file_uses_exclude_when_most_rules_apply() {
        // f: bbbbb and ccccc apply, a does not. apply-list "bbbbb, ccccc" (12) is
        // longer than skip-list "a" (1) -> exclude [a].
        let rs = rules(&[("a", &["other"]), ("bbbbb", &["f"]), ("ccccc", &["f"])]);
        let out = per_file(&rs, &["f".into()]);
        assert_eq!(out[0].mode, Mode::Exclude);
        assert_eq!(out[0].rules, vec!["a"]);
    }

    #[test]
    fn per_file_normalizes_paths_for_matching() {
        // a applies to src/x.rs, b does not; a judge's `./src/x.rs` still matches.
        let rs = rules(&[("a", &["src/x.rs"]), ("b", &["other.rs"])]);
        let out = per_file(&rs, &["./src/x.rs".into()]);
        assert_eq!(out[0].mode, Mode::Include);
        assert_eq!(out[0].rules, vec!["a"]);
    }

    #[test]
    fn scope_problems_flag_out_of_scope_file_violations_only() {
        let scope = scope_map(&rules(&[("a", &["src/x.rs"]), ("b", &["docs/y.md"])]));
        let mut verdicts = BTreeMap::new();
        verdicts.insert(
            "a".to_string(),
            RuleVerdict {
                holds: false,
                violations: vec![
                    viol(Some("docs/y.md"), Some(3)),
                    viol(Some("src/x.rs"), None),
                ],
                ..Default::default()
            },
        );
        // A file-less violation is never out of scope.
        verdicts.insert(
            "b".to_string(),
            RuleVerdict {
                holds: false,
                violations: vec![viol(None, None)],
                ..Default::default()
            },
        );
        let problems = scope_problems(&scope, &verdicts);
        assert_eq!(
            problems,
            vec![ScopeProblem {
                rule: "a".into(),
                file: "docs/y.md".into()
            }]
        );
    }

    #[test]
    fn clean_drops_out_of_scope_and_flips_a_fail_with_no_remaining_basis() {
        let scope: BTreeSet<String> = ["src/x.rs".to_string()].into_iter().collect();
        let mut v = RuleVerdict {
            holds: false,
            violations: vec![viol(Some("elsewhere.rs"), Some(2))],
            ..Default::default()
        };
        let dropped = clean_verdict(&mut v, &scope, |_, _| false);
        assert!(dropped);
        assert!(v.violations.is_empty());
        assert!(
            v.holds,
            "a fail whose only basis was out of scope flips to pass"
        );
    }

    #[test]
    fn clean_keeps_a_fail_with_an_in_scope_violation() {
        let scope: BTreeSet<String> = ["src/x.rs".to_string()].into_iter().collect();
        let mut v = RuleVerdict {
            holds: false,
            violations: vec![
                viol(Some("src/x.rs"), Some(2)),
                viol(Some("oops.rs"), Some(9)),
            ],
            ..Default::default()
        };
        let dropped = clean_verdict(&mut v, &scope, |_, _| false);
        assert!(dropped);
        assert_eq!(v.violations.len(), 1);
        assert!(!v.holds, "an in-scope violation keeps the fail");
    }

    #[test]
    fn clean_drops_a_suppressed_violation_without_calling_it_out_of_scope() {
        let scope: BTreeSet<String> = ["src/x.rs".to_string()].into_iter().collect();
        let mut v = RuleVerdict {
            holds: false,
            violations: vec![viol(Some("src/x.rs"), Some(7))],
            ..Default::default()
        };
        // Suppress the in-scope violation at line 7.
        let dropped = clean_verdict(&mut v, &scope, |f, l| f == "src/x.rs" && l == Some(7));
        assert!(
            !dropped,
            "a suppressed in-scope drop is not an out-of-scope problem"
        );
        assert!(v.violations.is_empty());
        assert!(
            v.holds,
            "suppressing the only violation flips the fail to pass"
        );
    }

    #[test]
    fn clean_does_not_flip_a_pass_or_a_fail_with_a_fileless_violation() {
        let scope: BTreeSet<String> = ["src/x.rs".to_string()].into_iter().collect();
        // A cross-cutting (file-less) violation survives and keeps the fail.
        let mut v = RuleVerdict {
            holds: false,
            violations: vec![viol(None, None), viol(Some("oops.rs"), Some(1))],
            ..Default::default()
        };
        clean_verdict(&mut v, &scope, |_, _| false);
        assert_eq!(v.violations.len(), 1);
        assert!(!v.holds);
    }

    #[test]
    fn rework_prompt_names_the_problems_and_the_scope() {
        // Two rules so src/x.rs has a non-degenerate apply-list (a applies, b not).
        let fr = per_file(
            &rules(&[("a", &["src/x.rs"]), ("b", &["other.rs"])]),
            &["src/x.rs".into()],
        );
        let problems = vec![ScopeProblem {
            rule: "a".into(),
            file: "docs/y.md".into(),
        }];
        let p = rework_prompt(&problems, &fr);
        assert!(p.contains("`a` reported a violation in `docs/y.md`"));
        assert!(
            p.contains("src/x.rs — only these rules apply: a"),
            "prompt:\n{p}"
        );
        assert!(p.contains("only the JSON object"));
    }
}
