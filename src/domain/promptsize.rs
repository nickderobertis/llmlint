//! Bound every judge prompt to `max_prompt_chars`.
//!
//! The planner batches rules over the union of their files without knowing how
//! large the rendered prompt will be — under `--diff` each changed file's diff is
//! inlined, so a big change can outgrow the harness's turn limit and come back as
//! an opaque "no JSON value could be extracted" failure (issue #156). This pass
//! runs after planning: a run whose prompt fits is kept as-is; one that does not
//! is split by its (sorted) files into consecutive parts, each the longest run of
//! files whose prompt still fits, and each part judges only the rules that apply
//! to its files. Every rule therefore still judges each of its files exactly once
//! per judge index. A file whose prompt is over the bound on its own is never
//! sent; it is returned as an [`OversizedFile`] for the caller to report.
//!
//! Pure: the prompt size is measured by a caller-supplied function (rendering the
//! real template), so the split is decided against the prompt actually sent.

use std::path::PathBuf;

use crate::domain::plan::{BatchPart, JudgeRun, OversizedFile, Plan};
use crate::domain::to_slash;

/// A file over the bound on its own, with the run it was planned into, for the
/// caller's errored result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oversized {
    pub agent: String,
    pub judge_index: u32,
    pub file: OversizedFile,
}

/// Split every run of `plan` whose prompt, as `measure` sizes it in characters,
/// is over `max`, and record each split (and each file over the bound on its
/// own) on the plan's explanation. Returns the files that could not be sent.
pub fn bound(
    plan: &mut Plan,
    max: usize,
    mut measure: impl FnMut(&JudgeRun) -> usize,
) -> Vec<Oversized> {
    let mut runs = Vec::with_capacity(plan.runs.len());
    let mut oversized = Vec::new();
    for run in std::mem::take(&mut plan.runs) {
        if measure(&run) <= max {
            runs.push(run);
            continue;
        }
        let (parts, too_big) = split(&run, max, &mut measure);
        if let Some(batch) = batch_plan(plan, &run) {
            batch.parts = if parts.len() > 1 {
                parts
                    .iter()
                    .map(|(part, chars)| BatchPart {
                        rules: part.rules.iter().map(|r| r.name.clone()).collect(),
                        files: part.files.iter().map(|f| to_slash(f)).collect(),
                        prompt_chars: *chars,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            batch.oversized_files = too_big.clone();
        }
        plan.explanation.max_prompt_chars = Some(max);
        runs.extend(parts.into_iter().map(|(part, _)| part));
        oversized.extend(too_big.into_iter().map(|file| Oversized {
            agent: run.agent.clone(),
            judge_index: run.judge_index,
            file,
        }));
    }
    plan.runs = runs;
    oversized
}

/// The explanation entry a run was planned under.
fn batch_plan<'a>(
    plan: &'a mut Plan,
    run: &JudgeRun,
) -> Option<&'a mut crate::domain::plan::BatchPlan> {
    plan.explanation
        .agents
        .iter_mut()
        .find(|a| a.agent == run.agent)?
        .judges
        .iter_mut()
        .find(|j| j.judge_index == run.judge_index)?
        .batches
        .iter_mut()
        .find(|b| b.id == run.batch_id)
}

/// Split one over-bound run into parts that each fit, with each part's measured
/// size, plus the files over the bound on their own. Each part is the longest
/// run of consecutive files that fits, found by doubling then bisecting, so a
/// split costs a logarithmic number of renders per part rather than one per file.
fn split(
    run: &JudgeRun,
    max: usize,
    measure: &mut impl FnMut(&JudgeRun) -> usize,
) -> (Vec<(JudgeRun, usize)>, Vec<OversizedFile>) {
    let files = &run.files;
    let mut parts = Vec::new();
    let mut oversized = Vec::new();
    let mut start = 0;
    while start < files.len() {
        let alone = restrict(run, &files[start..=start]);
        let alone_chars = measure(&alone);
        if alone_chars > max {
            oversized.push(OversizedFile {
                file: to_slash(&files[start]),
                prompt_chars: alone_chars,
                rules: alone.rules.iter().map(|r| r.name.clone()).collect(),
            });
            start += 1;
            continue;
        }
        // `fits` is the longest known-fitting end (exclusive); `over` the
        // shortest known end that does not fit (or one past the last file).
        let mut fits = (start + 1, alone_chars);
        let mut over = files.len() + 1;
        let mut step = 1;
        while fits.0 + step <= files.len() {
            let end = fits.0 + step;
            let chars = measure(&restrict(run, &files[start..end]));
            if chars <= max {
                fits = (end, chars);
                step *= 2;
            } else {
                over = end;
                break;
            }
        }
        while over - fits.0 > 1 {
            let mid = fits.0 + (over - fits.0) / 2;
            if mid > files.len() {
                break;
            }
            let chars = measure(&restrict(run, &files[start..mid]));
            if chars <= max {
                fits = (mid, chars);
            } else {
                over = mid;
            }
        }
        parts.push((restrict(run, &files[start..fits.0]), fits.1));
        start = fits.0;
    }
    (parts, oversized)
}

/// The run narrowed to `files`: each rule keeps only its files among them, and a
/// rule left with none is dropped from the part.
fn restrict(run: &JudgeRun, files: &[PathBuf]) -> JudgeRun {
    let keep: Vec<String> = files.iter().map(|f| to_slash(f)).collect();
    let rules = run
        .rules
        .iter()
        .filter_map(|rule| {
            let mine: Vec<String> = rule
                .files
                .iter()
                .filter(|f| keep.contains(f))
                .cloned()
                .collect();
            (!mine.is_empty()).then(|| {
                let mut r = rule.clone();
                r.files = mine;
                r
            })
        })
        .collect();
    JudgeRun {
        files: files.to_vec(),
        rules,
        ..run.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::plan::{AgentPlan, BatchPlan, JudgePlan};
    use crate::domain::template::RuleSpec;

    fn spec(name: &str, files: &[&str]) -> RuleSpec {
        RuleSpec {
            name: name.into(),
            description: "d".into(),
            rationale: false,
            relevance: None,
            require_line_attribution: false,
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// A one-run plan over `files` (each `(path, size)`), with rule `a` on every
    /// file and rule `b` on the first only.
    fn plan(files: &[(&str, usize)]) -> (Plan, std::collections::BTreeMap<String, usize>) {
        let names: Vec<&str> = files.iter().map(|(f, _)| *f).collect();
        let run = JudgeRun {
            agent: "default".into(),
            harness: None,
            model: None,
            schema_max_retries: None,
            judge_index: 1,
            batch_id: 1,
            template: String::new(),
            files: names.iter().map(PathBuf::from).collect(),
            rules: vec![spec("a", &names), spec("b", &names[..1])],
        };
        let mut p = Plan::default();
        p.runs.push(run);
        p.explanation.agents.push(AgentPlan {
            agent: "default".into(),
            batch_size: 20,
            model: None,
            harness: None,
            judges: vec![JudgePlan {
                judge_index: 1,
                batches: vec![BatchPlan {
                    id: 1,
                    rules: vec!["a".into(), "b".into()],
                    files: names.iter().map(|s| s.to_string()).collect(),
                    excluded_files: vec![],
                    reused_files: vec![],
                    parts: vec![],
                    oversized_files: vec![],
                }],
            }],
        });
        let sizes = files.iter().map(|(f, n)| (f.to_string(), *n)).collect();
        (p, sizes)
    }

    /// A prompt is 10 chars of boilerplate plus each carried file's size.
    fn measure(
        sizes: &std::collections::BTreeMap<String, usize>,
    ) -> impl FnMut(&JudgeRun) -> usize + '_ {
        |run| 10 + run.files.iter().map(|f| sizes[&to_slash(f)]).sum::<usize>()
    }

    #[test]
    fn a_run_that_fits_is_left_whole() {
        let (mut p, sizes) = plan(&[("a.rs", 30), ("b.rs", 30)]);
        let over = bound(&mut p, 100, measure(&sizes));
        assert!(over.is_empty());
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.explanation.max_prompt_chars, None);
        assert_eq!(p.explanation.total_runs(), 1);
    }

    #[test]
    fn an_over_bound_run_splits_into_parts_that_each_fit_and_cover_every_file_once() {
        let files: Vec<(String, usize)> = (0..9).map(|i| (format!("f{i}.rs"), 30)).collect();
        let refs: Vec<(&str, usize)> = files.iter().map(|(f, n)| (f.as_str(), *n)).collect();
        let (mut p, sizes) = plan(&refs);
        let over = bound(&mut p, 100, measure(&sizes));
        assert!(over.is_empty());
        // Three 30-char files fit with the 10-char boilerplate; four do not.
        assert_eq!(p.runs.len(), 3);
        let mut seen: Vec<String> = Vec::new();
        for run in &p.runs {
            assert!(measure(&sizes)(run) <= 100);
            seen.extend(run.files.iter().map(|f| to_slash(f)));
            // Rule `a` judges exactly this part's files.
            let a = run.rules.iter().find(|r| r.name == "a").unwrap();
            assert_eq!(a.files.len(), run.files.len());
        }
        assert_eq!(
            seen,
            files.iter().map(|(f, _)| f.clone()).collect::<Vec<_>>()
        );
        // Rule `b` only applies to the first file, so only the first part has it.
        assert_eq!(
            p.runs
                .iter()
                .filter(|r| r.rules.iter().any(|s| s.name == "b"))
                .count(),
            1
        );
        let batch = &p.explanation.agents[0].judges[0].batches[0];
        assert_eq!(batch.parts.len(), 3);
        assert_eq!(batch.parts[0].prompt_chars, 100);
        assert_eq!(p.explanation.total_runs(), 3);
        assert_eq!(p.explanation.max_prompt_chars, Some(100));
        assert!(p
            .explanation
            .to_human()
            .contains("split into 3 judge calls"));
    }

    #[test]
    fn a_file_over_the_bound_alone_is_never_sent() {
        let (mut p, sizes) = plan(&[("big.rs", 500), ("small.rs", 20)]);
        let over = bound(&mut p, 100, measure(&sizes));
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].file.file, "big.rs");
        assert_eq!(over[0].file.prompt_chars, 510);
        assert_eq!(over[0].file.rules, vec!["a", "b"]);
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].files, vec![PathBuf::from("small.rs")]);
        // One part is no split: the batch shows its oversized file, not parts.
        let batch = &p.explanation.agents[0].judges[0].batches[0];
        assert!(batch.parts.is_empty());
        assert_eq!(batch.oversized_files.len(), 1);
        assert!(p.explanation.to_human().contains("not sent big.rs"));
    }

    #[test]
    fn a_batch_of_only_oversized_files_makes_no_call() {
        let (mut p, sizes) = plan(&[("big.rs", 500)]);
        let over = bound(&mut p, 100, measure(&sizes));
        assert_eq!(over.len(), 1);
        assert!(p.runs.is_empty());
        assert_eq!(p.explanation.total_runs(), 0);
    }
}
