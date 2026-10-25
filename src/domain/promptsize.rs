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
//! per judge index. A file whose prompt is over the bound on its own is judged
//! in parts instead: its diff is split at hunk or line boundaries
//! ([`crate::domain::diffsplit`]) into the fewest consecutive parts that each
//! fit, every part a run of its own over that one file carrying the
//! [`DiffPart`] it shows, so each rule's parts are folded into one judge's vote
//! like any other split. Without `--diff` the judge reads a file with its own
//! tools, so its content is not in the prompt but lands in the judge's context
//! beside it: a file whose content and one-file prompt together are over the
//! bound is taken out of its run and judged in parts of its content instead,
//! split at line boundaries ([`crate::domain::linesplit`]), each part inlining
//! its lines in a run of its own carrying the [`ContentPart`] it shows — so the
//! parts fold the same way. Only a file that cannot fit even one diff or content
//! line beside the prompt's fixed overhead (or has neither to split, its prompt
//! being over the bound on overhead alone) is never sent; it is returned as an
//! [`OversizedFile`] for the caller to report.
//!
//! Pure: the prompt size is measured by a caller-supplied function (rendering the
//! real template), so the split is decided against the prompt actually sent.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::diffsplit::{self, SplitDiff};
use crate::domain::linesplit::{self, SplitContent};
use crate::domain::plan::{BatchPart, ContentPart, DiffPart, JudgeRun, OversizedFile, Plan};
use crate::domain::template::{LinesPart, PartOf};
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
/// is over `max`, and record each split (and each file that could not be sent)
/// on the plan's explanation. `diffs` holds each changed file's diff, which a
/// file over the bound on its own is split by; `contents` holds the text of each
/// file judged by its whole content (no diff), which such a file is split by
/// when its content would take the judge over the bound. Returns the files that
/// could not be sent.
pub fn bound(
    plan: &mut Plan,
    max: usize,
    diffs: &BTreeMap<PathBuf, String>,
    contents: &BTreeMap<PathBuf, String>,
    mut measure: impl FnMut(&JudgeRun) -> usize,
) -> Vec<Oversized> {
    let mut runs = Vec::with_capacity(plan.runs.len());
    let mut oversized = Vec::new();
    for run in std::mem::take(&mut plan.runs) {
        let whole = measure(&run);
        // Each file whose content would take the judge over the bound is pulled
        // out of the run into parts of its lines, or refused when a line cannot
        // fit. A template that does not inline a part's lines (a custom one
        // without `file_rules[i].content`) packs it into one part: it stays
        // named in its run, judged as before.
        let mut pulled: Vec<PathBuf> = Vec::new();
        let mut content_parts = Vec::new();
        let mut too_big = Vec::new();
        for (alone, chars) in over_on_content(&run, whole, max, diffs, contents, &mut measure) {
            let file = &alone.files[0];
            match split_content(&alone, &contents[file], max, &mut measure) {
                Some(more) if more.len() == 1 => continue,
                Some(more) => content_parts.extend(more),
                None => too_big.push(OversizedFile {
                    file: to_slash(file),
                    prompt_chars: chars,
                    rules: alone.rules.iter().map(|r| r.name.clone()).collect(),
                }),
            }
            pulled.push(file.clone());
        }
        if whole <= max && pulled.is_empty() {
            runs.push(run);
            continue;
        }
        let mut parts = Vec::new();
        let rest: Vec<PathBuf> = run
            .files
            .iter()
            .filter(|f| !pulled.contains(f))
            .cloned()
            .collect();
        if !rest.is_empty() {
            let (rest, chars) = if pulled.is_empty() {
                (run.clone(), whole)
            } else {
                let rest = restrict(&run, &rest);
                let chars = measure(&rest);
                (rest, chars)
            };
            if chars <= max {
                parts.push((rest, chars));
            } else {
                let (more, refused) = split(&rest, max, diffs, &mut measure);
                parts.extend(more);
                too_big.extend(refused);
            }
        }
        parts.extend(content_parts);
        if let Some(batch) = batch_plan(plan, &run) {
            batch.parts = if parts.len() > 1 {
                parts
                    .iter()
                    .map(|(part, chars)| BatchPart {
                        rules: part.rules.iter().map(|r| r.name.clone()).collect(),
                        files: part.files.iter().map(|f| to_slash(f)).collect(),
                        prompt_chars: *chars,
                        diff_part: part.diff_part.as_ref().map(|d| d.part),
                        content_part: part.content_part.as_ref().map(|c| c.part),
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

/// The one-file runs, with the size each would put in the judge's context, of
/// the files in `run` judged by their whole content (no diff) whose one-file
/// prompt plus that content is over `max`. `whole` is `run`'s own prompt size: a
/// file's one-file prompt is never longer than the run's, so a file whose content
/// fits beside the whole run's prompt is not measured on its own.
fn over_on_content(
    run: &JudgeRun,
    whole: usize,
    max: usize,
    diffs: &BTreeMap<PathBuf, String>,
    contents: &BTreeMap<PathBuf, String>,
    measure: &mut impl FnMut(&JudgeRun) -> usize,
) -> Vec<(JudgeRun, usize)> {
    let mut out = Vec::new();
    for file in &run.files {
        if diffs.contains_key(file) {
            continue;
        }
        let Some(content) = contents.get(file) else {
            continue;
        };
        let chars = content.chars().count();
        if whole + chars <= max {
            continue;
        }
        let alone = restrict(run, std::slice::from_ref(file));
        let total = measure(&alone) + chars;
        if total > max {
            out.push((alone, total));
        }
    }
    out
}

/// Split the one-file run `alone`, whose content would take the judge over
/// `max`, into runs that each inline one run of the file's lines and fit, with
/// each one's measured size; `None` when the content cannot be split to fit.
fn split_content(
    alone: &JudgeRun,
    content: &str,
    max: usize,
    measure: &mut impl FnMut(&JudgeRun) -> usize,
) -> Option<Vec<(JudgeRun, usize)>> {
    let with = |part: LinesPart, text: &str| JudgeRun {
        content_part: Some(ContentPart {
            part,
            text: text.to_string(),
        }),
        ..alone.clone()
    };
    let parsed = SplitContent::parse(content);
    // As for a diff: pack against the widest part numbers there could be (no
    // more parts than lines), so the final prompt is never longer than measured.
    let widest = content.lines().count().max(1);
    let packed = linesplit::pack(&parsed, max, |start_line, end_line, text| {
        measure(&with(
            LinesPart {
                index: widest,
                of: widest,
                start_line,
                end_line,
            },
            text,
        ))
    })?;
    if packed.is_empty() {
        return None;
    }
    let of = packed.len();
    Some(
        packed
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let part = LinesPart {
                    index: i + 1,
                    of,
                    start_line: p.start_line,
                    end_line: p.end_line,
                };
                let run = with(part, &p.text);
                let chars = measure(&run);
                (run, chars)
            })
            .collect(),
    )
}

/// Split one over-bound run into parts that each fit, with each part's measured
/// size, plus the files that could not be sent. Each part is the longest run of
/// consecutive files that fits, found by doubling then bisecting, so a split
/// costs a logarithmic number of renders per part rather than one per file; a
/// file over the bound on its own contributes the parts of its diff instead.
fn split(
    run: &JudgeRun,
    max: usize,
    diffs: &BTreeMap<PathBuf, String>,
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
            if let Some(diff_parts) = diffs
                .get(&files[start])
                .and_then(|diff| split_file(&alone, diff, max, measure))
            {
                parts.extend(diff_parts);
                start += 1;
                continue;
            }
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

/// Split the one-file run `alone`, whose prompt is over `max`, by its file's
/// `diff` into runs that each carry one part of it and fit, with each one's
/// measured size; `None` when the diff cannot be split to fit.
fn split_file(
    alone: &JudgeRun,
    diff: &str,
    max: usize,
    measure: &mut impl FnMut(&JudgeRun) -> usize,
) -> Option<Vec<(JudgeRun, usize)>> {
    let with = |part: PartOf, diff: &str| JudgeRun {
        diff_part: Some(DiffPart {
            part,
            diff: diff.to_string(),
        }),
        ..alone.clone()
    };
    let parsed = SplitDiff::parse(diff);
    // The part numbers are only known once packed, so pack against the widest
    // they could be (no more parts than diff lines): the final prompt is never
    // longer than the one measured.
    let widest = diff.lines().count().max(1);
    let widest = PartOf {
        index: widest,
        of: widest,
    };
    let packed = diffsplit::pack(&parsed, max, |text| measure(&with(widest, text)))?;
    if packed.is_empty() {
        return None;
    }
    let of = packed.len();
    Some(
        packed
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let run = with(PartOf { index: i + 1, of }, &p.diff);
                let chars = measure(&run);
                (run, chars)
            })
            .collect(),
    )
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
            diff_part: None,
            content_part: None,
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
        let over = bound(
            &mut p,
            100,
            &BTreeMap::new(),
            &BTreeMap::new(),
            measure(&sizes),
        );
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
        let over = bound(
            &mut p,
            100,
            &BTreeMap::new(),
            &BTreeMap::new(),
            measure(&sizes),
        );
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
    fn a_file_over_the_bound_with_no_diff_to_split_is_never_sent() {
        let (mut p, sizes) = plan(&[("big.rs", 500), ("small.rs", 20)]);
        let over = bound(
            &mut p,
            100,
            &BTreeMap::new(),
            &BTreeMap::new(),
            measure(&sizes),
        );
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
        let over = bound(
            &mut p,
            100,
            &BTreeMap::new(),
            &BTreeMap::new(),
            measure(&sizes),
        );
        assert_eq!(over.len(), 1);
        assert!(p.runs.is_empty());
        assert_eq!(p.explanation.total_runs(), 0);
    }

    const PREAMBLE: &str = "--- a/big.rs\n+++ b/big.rs\n";

    /// `big.rs`'s diff: `n` one-line replacement hunks.
    fn big_diff(n: usize) -> String {
        let mut d = PREAMBLE.to_string();
        for i in 0..n {
            d.push_str(&format!("@@ -{i},1 +{i},1 @@\n-o{i:02}\n+n{i:02}\n"));
        }
        d
    }

    /// A prompt is 10 chars of boilerplate plus each file's diff (or size, for a
    /// file with none), or the one part of a diff a run carries.
    fn measure_diffs<'a>(
        sizes: &'a std::collections::BTreeMap<String, usize>,
        diffs: &'a BTreeMap<PathBuf, String>,
    ) -> impl FnMut(&JudgeRun) -> usize + 'a {
        |run| match &run.diff_part {
            Some(part) => 10 + part.diff.len(),
            None => {
                10 + run
                    .files
                    .iter()
                    .map(|f| {
                        diffs
                            .get(f)
                            .map_or_else(|| sizes[&to_slash(f)], String::len)
                    })
                    .sum::<usize>()
            }
        }
    }

    #[test]
    fn a_file_over_the_bound_alone_is_judged_in_parts_of_its_diff() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let diffs = BTreeMap::from([(PathBuf::from("big.rs"), big_diff(20))]);
        let max = 120;
        let over = bound(
            &mut p,
            max,
            &diffs,
            &BTreeMap::new(),
            measure_diffs(&sizes, &diffs),
        );
        assert!(over.is_empty(), "{over:?}");
        let (parted, whole): (Vec<&JudgeRun>, Vec<&JudgeRun>) =
            p.runs.iter().partition(|r| r.diff_part.is_some());
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].files, vec![PathBuf::from("small.rs")]);
        // The diff's hunks, three to a part with the preamble, in order and once.
        assert_eq!(parted.len(), 7);
        let mut hunks = String::new();
        for (i, run) in parted.iter().enumerate() {
            let part = run.diff_part.as_ref().unwrap();
            assert_eq!(
                part.part,
                PartOf {
                    index: i + 1,
                    of: 7
                }
            );
            assert!(10 + part.diff.len() <= max);
            assert_eq!(run.files, vec![PathBuf::from("big.rs")]);
            // Both rules judge big.rs in every part.
            let names: Vec<&str> = run.rules.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(names, vec!["a", "b"]);
            hunks.push_str(part.diff.strip_prefix(PREAMBLE).unwrap());
        }
        assert_eq!(hunks, big_diff(20)[PREAMBLE.len()..]);
        // The plan names each call's diff part.
        let batch = &p.explanation.agents[0].judges[0].batches[0];
        assert_eq!(batch.parts.len(), 8);
        assert!(batch.oversized_files.is_empty());
        assert_eq!(batch.parts[0].diff_part, Some(PartOf { index: 1, of: 7 }));
        assert_eq!(batch.parts[7].diff_part, None);
        assert!(p
            .explanation
            .to_human()
            .contains("big.rs (diff part 7 of 7)"));
        // And the same input always splits the same way.
        let (mut again, _) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        bound(
            &mut again,
            max,
            &diffs,
            &BTreeMap::new(),
            measure_diffs(&sizes, &diffs),
        );
        let diffs_of = |p: &Plan| -> Vec<Option<DiffPart>> {
            p.runs.iter().map(|r| r.diff_part.clone()).collect()
        };
        assert_eq!(diffs_of(&again), diffs_of(&p));
    }

    #[test]
    fn a_diff_line_too_big_for_any_part_still_refuses_the_file() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let diff = format!("{PREAMBLE}@@ -1,1 +1,2 @@\n-a\n+{}\n+b\n", "x".repeat(200));
        let diffs = BTreeMap::from([(PathBuf::from("big.rs"), diff.clone())]);
        let over = bound(
            &mut p,
            120,
            &diffs,
            &BTreeMap::new(),
            measure_diffs(&sizes, &diffs),
        );
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].file.file, "big.rs");
        assert_eq!(over[0].file.prompt_chars, 10 + diff.len());
        assert_eq!(p.runs.len(), 1);
        assert!(p.runs.iter().all(|r| r.diff_part.is_none()));
        assert_eq!(p.runs[0].files, vec![PathBuf::from("small.rs")]);
    }

    /// `big.rs`'s content: `n` lines `line NN`.
    fn big_content(n: usize) -> String {
        (1..=n).map(|i| format!("line {i:02}\n")).collect()
    }

    /// A prompt is 10 chars of boilerplate plus each named file's size, or the
    /// lines one part of a file's content inlines.
    fn measure_contents(
        sizes: &std::collections::BTreeMap<String, usize>,
    ) -> impl FnMut(&JudgeRun) -> usize + '_ {
        |run| match &run.content_part {
            Some(part) => 10 + part.text.chars().count(),
            None => 10 + run.files.iter().map(|f| sizes[&to_slash(f)]).sum::<usize>(),
        }
    }

    #[test]
    fn a_file_whose_content_is_over_the_bound_is_judged_in_parts_of_its_lines() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let content = big_content(40);
        let contents = BTreeMap::from([(PathBuf::from("big.rs"), content.clone())]);
        let max = 120;
        let over = bound(
            &mut p,
            max,
            &BTreeMap::new(),
            &contents,
            measure_contents(&sizes),
        );
        assert!(over.is_empty(), "{over:?}");
        let (parted, whole): (Vec<&JudgeRun>, Vec<&JudgeRun>) =
            p.runs.iter().partition(|r| r.content_part.is_some());
        // small.rs is still judged whole, with no content inlined.
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].files, vec![PathBuf::from("small.rs")]);
        assert!(parted.len() > 1);
        let of = parted.len();
        let mut next = 1;
        for (i, run) in parted.iter().enumerate() {
            let part = run.content_part.as_ref().unwrap();
            assert_eq!((part.part.index, part.part.of), (i + 1, of));
            assert_eq!(part.part.start_line, next, "parts are consecutive");
            assert!(measure_contents(&sizes)(run) <= max);
            assert_eq!(run.files, vec![PathBuf::from("big.rs")]);
            let names: Vec<&str> = run.rules.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(
                names,
                vec!["a", "b"],
                "both rules judge big.rs in every part"
            );
            // Adjacent parts could not be merged: one more line is over the bound.
            if i + 1 < of {
                assert!(10 + part.text.chars().count() + "\n40  line 40".len() > max);
            }
            next = part.part.end_line + 1;
        }
        assert_eq!(next, 41, "every line is judged once");
        let batch = &p.explanation.agents[0].judges[0].batches[0];
        assert_eq!(batch.parts.len(), of + 1);
        assert_eq!(
            batch.parts[0].content_part, None,
            "small.rs's call comes first"
        );
        assert_eq!(
            batch.parts[1]
                .content_part
                .map(|c| (c.index, c.of, c.start_line)),
            Some((1, of, 1))
        );
        let human = p.explanation.to_human();
        assert!(human.contains("big.rs (lines 1-"), "{human}");
        assert!(human.contains(&format!("part {of} of {of})")), "{human}");
        // The same input always splits the same way.
        let (mut again, _) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        bound(
            &mut again,
            max,
            &BTreeMap::new(),
            &contents,
            measure_contents(&sizes),
        );
        assert_eq!(again.runs.len(), p.runs.len());
        for (a, b) in again.runs.iter().zip(&p.runs) {
            assert_eq!(a.content_part, b.content_part);
        }
    }

    #[test]
    fn a_file_whose_content_fits_beside_its_run_leaves_the_run_whole() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let contents = BTreeMap::from([(PathBuf::from("big.rs"), big_content(5))]);
        let over = bound(
            &mut p,
            120,
            &BTreeMap::new(),
            &contents,
            measure_contents(&sizes),
        );
        assert!(over.is_empty());
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].content_part, None);
        assert_eq!(p.explanation.max_prompt_chars, None);
    }

    #[test]
    fn a_content_line_too_big_for_any_part_still_refuses_the_file() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let content = format!("short\n{}\nshort\n", "x".repeat(200));
        let contents = BTreeMap::from([(PathBuf::from("big.rs"), content.clone())]);
        let over = bound(
            &mut p,
            120,
            &BTreeMap::new(),
            &contents,
            measure_contents(&sizes),
        );
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].file.file, "big.rs");
        assert_eq!(over[0].file.prompt_chars, 10 + content.len());
        assert_eq!(over[0].file.rules, vec!["a", "b"]);
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].files, vec![PathBuf::from("small.rs")]);
        assert!(p.runs.iter().all(|r| r.content_part.is_none()));
    }

    #[test]
    fn a_template_that_does_not_inline_content_leaves_the_file_in_its_run() {
        let (mut p, sizes) = plan(&[("big.rs", 0), ("small.rs", 20)]);
        let contents = BTreeMap::from([(PathBuf::from("big.rs"), big_content(40))]);
        // Every run measures as if its content part were not rendered.
        let over = bound(&mut p, 120, &BTreeMap::new(), &contents, measure(&sizes));
        assert!(over.is_empty());
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].content_part, None);
        assert_eq!(
            p.runs[0].files,
            vec![PathBuf::from("big.rs"), PathBuf::from("small.rs")]
        );
        assert_eq!(p.explanation.max_prompt_chars, None);
    }
}
