//! Plan the judge runs: group rules by agent and target files, expand the
//! multi-judge scheme, and split into batches — each batch is one `oneharness`
//! invocation.
//!
//! Multi-judge majority vote (per the configured scheme): within an
//! (agent, files) group, `maxJudges = max(rule.judges)`. For judge index
//! `j ∈ 1..=maxJudges` the judge evaluates `{rules | judges >= j}`, split into
//! balanced batches no larger than `batch_size` (the fewest batches that respect
//! the cap, with sizes kept within one of each other — see `balanced_chunks`). So
//! a `judges: N` rule appears in judges `1..=N` → N independent verdicts →
//! majority; a `judges: 1` rule runs exactly once.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::config::Config;
use crate::domain::template::RuleSpec;

/// A rule with its agent and target files resolved (globbing done by `io`).
#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub name: String,
    pub description: String,
    pub judges: u32,
    pub agent: String,
    pub files: Vec<PathBuf>,
    /// Whether the judge must justify this rule's verdict with a `rationale`.
    pub rationale: bool,
    /// The relevance condition the judge must decide before evaluating, or `None`
    /// for an always-evaluated rule. (Statically never-relevant rules are filtered
    /// out before planning and never reach here.)
    pub relevance: Option<String>,
}

/// One judge invocation: a batch of rules to evaluate against a file set.
#[derive(Debug, Clone)]
pub struct JudgeRun {
    pub agent: String,
    /// Harness id to pass to oneharness, or `None` to let oneharness pick its
    /// own configured default (no `--harness` flag is sent).
    pub harness: Option<String>,
    pub model: Option<String>,
    pub schema_max_retries: Option<u32>,
    pub judge_index: u32,
    /// Master template + this agent's appended prompt text (not yet rendered).
    pub template: String,
    pub files: Vec<PathBuf>,
    pub rules: Vec<RuleSpec>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub runs: Vec<JudgeRun>,
    /// Rules with no matching files — nothing to lint, reported as skipped.
    pub skipped: Vec<String>,
}

/// Build the plan. Deterministic: groups and runs come out in a stable order.
pub fn build(
    config: &Config,
    master_template: &str,
    default_batch_size: usize,
    resolved: Vec<ResolvedRule>,
) -> Plan {
    let mut plan = Plan::default();

    // Group by agent (stable order via BTreeMap).
    let mut by_agent: BTreeMap<String, Vec<ResolvedRule>> = BTreeMap::new();
    for r in resolved {
        by_agent.entry(r.agent.clone()).or_default().push(r);
    }

    for (agent_name, rules) in by_agent {
        let agent = config.agent_or_default(&agent_name);
        // No agent harness => leave it unset so oneharness uses its own default.
        let harness = agent.harness.clone();
        let batch_size = agent.batch_size.unwrap_or(default_batch_size).max(1);
        let template = match &agent.prompt_template {
            Some(extra) => format!("{master_template}\n\n{extra}"),
            None => master_template.to_string(),
        };

        // Within an agent, group by the resolved file set so each run carries a
        // coherent file list.
        let mut by_files: BTreeMap<Vec<PathBuf>, Vec<ResolvedRule>> = BTreeMap::new();
        for r in rules {
            by_files.entry(r.files.clone()).or_default().push(r);
        }

        for (files, group) in by_files {
            if files.is_empty() {
                plan.skipped.extend(group.into_iter().map(|r| r.name));
                continue;
            }
            let max_judges = group.iter().map(|r| r.judges).max().unwrap_or(1);
            for j in 1..=max_judges {
                let subset: Vec<&ResolvedRule> = group.iter().filter(|r| r.judges >= j).collect();
                for chunk in balanced_chunks(&subset, batch_size) {
                    plan.runs.push(JudgeRun {
                        agent: agent_name.clone(),
                        harness: harness.clone(),
                        model: agent.model.clone(),
                        schema_max_retries: config.oneharness.schema_max_retries,
                        judge_index: j,
                        template: template.clone(),
                        files: files.clone(),
                        rules: chunk
                            .iter()
                            .map(|r| RuleSpec {
                                name: r.name.clone(),
                                description: r.description.clone(),
                                rationale: r.rationale,
                                relevance: r.relevance.clone(),
                            })
                            .collect(),
                    });
                }
            }
        }
    }

    plan
}

/// Split `items` into batches no larger than `batch_size`, balancing the load so
/// the batch count is minimal *and* the sizes are as even as possible. E.g. 21
/// items with `batch_size` 20 yields two batches of 11 and 10 — not 20 and 1.
///
/// The number of batches is `ceil(len / batch_size)` (the fewest that respect the
/// cap); the remainder is then spread one-per-batch across the leading batches, so
/// sizes differ by at most one and order is preserved.
fn balanced_chunks<T>(items: &[T], batch_size: usize) -> Vec<&[T]> {
    if items.is_empty() {
        return Vec::new();
    }
    let batch_size = batch_size.max(1);
    let num_batches = items.len().div_ceil(batch_size);
    let base = items.len() / num_batches;
    let remainder = items.len() % num_batches;

    let mut chunks = Vec::with_capacity(num_batches);
    let mut start = 0;
    for i in 0..num_batches {
        // The first `remainder` batches take one extra item so sizes stay within 1.
        let size = base + usize::from(i < remainder);
        chunks.push(&items[start..start + size]);
        start += size;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::Agent;

    fn rr(name: &str, judges: u32, agent: &str, files: &[&str]) -> ResolvedRule {
        ResolvedRule {
            name: name.into(),
            description: format!("desc {name}"),
            judges,
            agent: agent.into(),
            files: files.iter().map(PathBuf::from).collect(),
            rationale: true,
            relevance: None,
        }
    }

    #[test]
    fn single_judge_rules_run_once() {
        let cfg = Config::default();
        let plan = build(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "default", &["f.rs"]),
                rr("b", 1, "default", &["f.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].rules.len(), 2);
        // No agent harness configured -> left unset for oneharness to default.
        assert_eq!(plan.runs[0].harness, None);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn multi_judge_expands_into_one_run_per_judge_index() {
        let cfg = Config::default();
        // a: 3 judges, b: 1 judge, same files -> judge1{a,b}, judge2{a}, judge3{a}.
        let plan = build(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 3, "default", &["f.rs"]),
                rr("b", 1, "default", &["f.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 3);
        let j1 = plan.runs.iter().find(|r| r.judge_index == 1).unwrap();
        assert_eq!(j1.rules.len(), 2);
        assert_eq!(plan.runs.iter().filter(|r| r.judge_index == 2).count(), 1);
        assert_eq!(plan.runs.iter().filter(|r| r.judge_index == 3).count(), 1);
    }

    #[test]
    fn batches_respect_agent_batch_size() {
        let mut cfg = Config::default();
        cfg.agents.insert(
            "small".into(),
            Agent {
                batch_size: Some(2),
                prompt_template: Some("be terse".into()),
                ..Default::default()
            },
        );
        let rules = vec![
            rr("a", 1, "small", &["f.rs"]),
            rr("b", 1, "small", &["f.rs"]),
            rr("c", 1, "small", &["f.rs"]),
        ];
        let plan = build(&cfg, "MASTER", 20, rules);
        assert_eq!(plan.runs.len(), 2); // 3 rules / batch 2 -> 2 batches
        assert!(plan.runs[0].template.contains("MASTER"));
        assert!(plan.runs[0].template.contains("be terse"));
    }

    #[test]
    fn batches_are_balanced_not_packed() {
        // 21 rules with batch_size 20 must split 11/10, not 20/1.
        let mut cfg = Config::default();
        cfg.agents.insert(
            "big".into(),
            Agent {
                batch_size: Some(20),
                ..Default::default()
            },
        );
        let rules: Vec<ResolvedRule> = (0..21)
            .map(|i| rr(&format!("r{i}"), 1, "big", &["f.rs"]))
            .collect();
        let plan = build(&cfg, "T", 20, rules);
        let mut sizes: Vec<usize> = plan.runs.iter().map(|r| r.rules.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![10, 11]);
        // Every rule still appears exactly once across the batches.
        let total: usize = plan.runs.iter().map(|r| r.rules.len()).sum();
        assert_eq!(total, 21);
    }

    #[test]
    fn balanced_chunks_covers_items_without_overlap() {
        // 25 items, cap 10 -> 3 batches sized 9/8/8 (differ by at most one).
        let items: Vec<usize> = (0..25).collect();
        let chunks = balanced_chunks(&items, 10);
        assert_eq!(chunks.len(), 3);
        let mut sizes: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![8, 8, 9]);
        // Concatenation reproduces the input in order — no gaps, no overlap.
        let flat: Vec<usize> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(flat, items);
    }

    #[test]
    fn balanced_chunks_single_full_batch() {
        let items: Vec<usize> = (0..20).collect();
        let chunks = balanced_chunks(&items, 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 20);
    }

    #[test]
    fn balanced_chunks_empty_is_no_batches() {
        let items: Vec<usize> = Vec::new();
        assert!(balanced_chunks(&items, 5).is_empty());
    }

    #[test]
    fn per_rule_rationale_flows_into_the_rule_spec() {
        let cfg = Config::default();
        let mut on = rr("on", 1, "default", &["f.rs"]);
        let mut off = rr("off", 1, "default", &["f.rs"]);
        on.rationale = true;
        off.rationale = false;
        let plan = build(&cfg, "T", 20, vec![on, off]);
        let specs = &plan.runs[0].rules;
        let find = |n: &str| specs.iter().find(|r| r.name == n).unwrap().rationale;
        assert!(find("on"));
        assert!(!find("off"));
    }

    #[test]
    fn relevance_condition_flows_into_the_rule_spec() {
        let cfg = Config::default();
        let mut conditional = rr("conditional", 1, "default", &["f.rs"]);
        let always = rr("always", 1, "default", &["f.rs"]);
        conditional.relevance = Some("the change touches SQL".into());
        let plan = build(&cfg, "T", 20, vec![conditional, always]);
        let specs = &plan.runs[0].rules;
        let find = |n: &str| {
            specs
                .iter()
                .find(|r| r.name == n)
                .unwrap()
                .relevance
                .clone()
        };
        assert_eq!(
            find("conditional").as_deref(),
            Some("the change touches SQL")
        );
        assert_eq!(find("always"), None);
    }

    #[test]
    fn distinct_file_sets_are_separate_runs() {
        let cfg = Config::default();
        let plan = build(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "default", &["x.rs"]),
                rr("b", 1, "default", &["y.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 2);
    }

    #[test]
    fn empty_file_set_is_skipped_not_run() {
        let cfg = Config::default();
        let plan = build(&cfg, "T", 20, vec![rr("a", 1, "default", &[])]);
        assert!(plan.runs.is_empty());
        assert_eq!(plan.skipped, vec!["a".to_string()]);
    }

    #[test]
    fn agent_harness_override_is_used() {
        let mut cfg = Config::default();
        cfg.agents.insert(
            "arch".into(),
            Agent {
                harness: Some("codex".into()),
                ..Default::default()
            },
        );
        let plan = build(&cfg, "T", 20, vec![rr("a", 1, "arch", &["f.rs"])]);
        assert_eq!(plan.runs[0].harness.as_deref(), Some("codex"));
    }

    #[test]
    fn no_agent_harness_leaves_it_unset() {
        let mut cfg = Config::default();
        // An agent that sets some fields but deliberately not `harness`.
        cfg.agents.insert(
            "arch".into(),
            Agent {
                model: Some("gpt-5".into()),
                ..Default::default()
            },
        );
        let plan = build(&cfg, "T", 20, vec![rr("a", 1, "arch", &["f.rs"])]);
        assert_eq!(plan.runs[0].harness, None);
        assert_eq!(plan.runs[0].model.as_deref(), Some("gpt-5"));
    }
}
