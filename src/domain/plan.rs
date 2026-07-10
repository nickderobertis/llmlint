//! Plan the judge runs: group rules by agent, narrow each to its effective files,
//! expand the multi-judge scheme, and assign rules to batches — each batch is one
//! `oneharness` invocation.
//!
//! Multi-judge majority vote (per the configured scheme): within an agent,
//! `maxJudges = max(rule.judges)`. For judge index `j ∈ 1..=maxJudges` the judge
//! evaluates `{rules | judges >= j}`. So a `judges: N` rule appears in judges
//! `1..=N` → N independent verdicts → majority; a `judges: 1` rule runs once.
//!
//! **Batch assignment.** Within the fixed batch count `ceil(n / batch_size)`,
//! [`cost::Model::assign`] picks the rule→batch layout that minimizes the
//! token-weighted objective — lexicographically the tokens *billed* (each file's
//! content re-billed per batch it lands in), then per-rule token *exposure* (each
//! rule reads its whole batch's files), then a balanced-size tiebreak. At a fixed
//! batch count these never trade off; `assign` is a provable minimum within its
//! search budget (else a deterministic heuristic). The order-based layout is costed
//! too, only to report what the optimization saved in the [`PlanExplanation`].
//! Agents are never merged, whatever the saving.
//!
//! **Ignore narrowing.** A whole-file `ignore-file` (via [`PlanContext`]) drops the
//! file from a rule's effective scope before batching; a rule left with no file is
//! reported [`SkipReason::AllFilesIgnored`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::domain::config::Config;
use crate::domain::cost;
use crate::domain::ignore::Suppressions;
use crate::domain::template::RuleSpec;
use crate::domain::to_slash;

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
    /// Whether every violation of this rule must cite a concrete file + line.
    pub require_line_attribution: bool,
}

/// Inputs the planner consults *besides* the rules themselves:
///
/// - the per-file inline-ignore suppressions (keyed by the file's forward-slash
///   path), so a file a rule wholly `ignore-file`s can be dropped from that rule's
///   effective scope before the judge sees it — never sending (nor paying tokens
///   for) a file whose every verdict would be discarded post-vote anyway;
/// - the per-file token weights (also slash-keyed), so the batcher minimizes *real*
///   token cost rather than a raw file count. A file absent from the map (or an
///   empty map) weighs 1, reducing the objective to file counts — the behavior
///   tests and weightless callers rely on.
#[derive(Debug, Clone, Copy)]
pub struct PlanContext<'a> {
    suppressions: &'a BTreeMap<String, Suppressions>,
    file_tokens: Option<&'a BTreeMap<String, usize>>,
}

impl<'a> PlanContext<'a> {
    /// Build a context from the per-file suppressions the caller already parsed,
    /// with unit (count-based) file weights.
    pub fn new(suppressions: &'a BTreeMap<String, Suppressions>) -> Self {
        PlanContext {
            suppressions,
            file_tokens: None,
        }
    }

    /// Attach per-file token weights (slash path → estimated tokens) so the batcher
    /// optimizes token cost, not file count.
    pub fn with_weights(mut self, file_tokens: &'a BTreeMap<String, usize>) -> Self {
        self.file_tokens = Some(file_tokens);
        self
    }

    /// Whether `rule` is suppressed for the whole of `file` (slash path) by an
    /// `ignore-file` directive, so the planner may drop it from the rule's scope.
    fn file_ignored(&self, file: &str, rule: &str) -> bool {
        self.suppressions
            .get(file)
            .is_some_and(|s| s.is_file_scoped(rule))
    }

    /// The token weight of `file` (slash path): its configured weight, or 1 when no
    /// weights were supplied (so the objective reduces to a file count).
    fn weight(&self, file: &str) -> usize {
        self.file_tokens
            .and_then(|m| m.get(file))
            .copied()
            .unwrap_or(1)
    }
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

/// Why a rule was not planned into any judge run — surfaced distinctly so the
/// report never conflates "nothing matched" with "everything was ignored".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// No file matched the rule's globs — there is nothing to lint.
    NoFiles,
    /// Every file the rule would cover is suppressed by a whole-file `ignore-file`
    /// directive for this rule, so nothing is left to judge. Reported as *ignored*
    /// (a deliberate, reasoned exemption), not as an incidental skip.
    AllFilesIgnored,
}

/// A rule left out of the run, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub rule: String,
    pub reason: SkipReason,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub runs: Vec<JudgeRun>,
    /// Rules not planned into any run, each with the reason (no files / all files
    /// ignored) so the caller can report them faithfully.
    pub skipped: Vec<Skip>,
    /// A human/JSON-readable account of *why* the runs are shaped as they are —
    /// which agent owns each judge call, how rules were batched, and which files
    /// were dropped as fully ignored. Built while planning so it can never drift
    /// from the actual runs.
    pub explanation: PlanExplanation,
}

/// One rule with its scope narrowed to the files that still need judging: its
/// declared files minus any a whole-file `ignore-file` directive suppresses for
/// it. The atomic unit the planner batches.
struct Eligible<'a> {
    rule: &'a ResolvedRule,
    /// The rule's declared files that survive its file-scoped ignores.
    effective: Vec<PathBuf>,
}

/// Build the plan. Deterministic: groups and runs come out in a stable order.
///
/// `ctx` supplies the inline-ignore suppressions so a file a rule wholly
/// `ignore-file`s is dropped from that rule's *effective* scope up front — the
/// judge is never sent (and the prompt never pays for) a file whose every verdict
/// for that rule would be discarded post-vote. A rule left with no effective file
/// is reported as [`SkipReason::AllFilesIgnored`]; one that never matched a file
/// is [`SkipReason::NoFiles`].
pub fn build(
    config: &Config,
    master_template: &str,
    default_batch_size: usize,
    resolved: Vec<ResolvedRule>,
    ctx: &PlanContext,
) -> Plan {
    let mut plan = Plan::default();

    // Group by agent (stable order via BTreeMap). Agents are the outermost — and
    // hardest — boundary: rules in different agents are NEVER co-batched, even
    // when their harness/model/template are identical and merging would save
    // tokens. An agent split is user intent (isolating rules that interfere when
    // judged together), and no batching optimization may cross it.
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

        // Narrow each rule to its effective files (declared minus fully-ignored),
        // then partition: nothing matched -> NoFiles; matched but every file is
        // ignore-file'd -> AllFilesIgnored; otherwise eligible. The eligible rules
        // are batched together (per judge index) over the *union* of their
        // effective files, and the rendered prompt tells the judge, per file,
        // which rules apply (see `domain::applicability`).
        let mut eligible: Vec<Eligible> = Vec::new();
        for rule in &rules {
            if rule.files.is_empty() {
                plan.skipped.push(Skip {
                    rule: rule.name.clone(),
                    reason: SkipReason::NoFiles,
                });
                continue;
            }
            let effective: Vec<PathBuf> = rule
                .files
                .iter()
                .filter(|f| !ctx.file_ignored(&to_slash(f), &rule.name))
                .cloned()
                .collect();
            if effective.is_empty() {
                plan.skipped.push(Skip {
                    rule: rule.name.clone(),
                    reason: SkipReason::AllFilesIgnored,
                });
            } else {
                eligible.push(Eligible { rule, effective });
            }
        }

        let mut agent_plan = AgentPlan {
            agent: agent_name.clone(),
            batch_size,
            model: agent.model.clone(),
            harness: harness.clone(),
            judges: Vec::new(),
        };
        if eligible.is_empty() {
            // Still record the agent so the explanation shows it owned rules that
            // all skipped/ignored, rather than vanishing silently.
            if !rules.is_empty() {
                plan.explanation.agents.push(agent_plan);
            }
            continue;
        }

        let max_judges = eligible.iter().map(|e| e.rule.judges).max().unwrap_or(1);
        for j in 1..=max_judges {
            let subset: Vec<&Eligible> = eligible.iter().filter(|e| e.rule.judges >= j).collect();
            // Assign rules to batches to minimize the token cost, within the fixed
            // batch count `ceil(n / batch_size)`. The objective is lexicographic:
            // first the tokens billed (each file's content re-billed per batch it
            // lands in), then per-rule token exposure (each rule reads its whole
            // batch's files — so a big union shared by many rules is expensive).
            // At a fixed batch count these never trade off; `Model::assign` returns a
            // provable minimum (see `domain::cost`). The order-based layout is costed
            // too, only to report what the optimization saved.
            let model = build_cost_model(ctx, &subset);
            let batch_count = subset.len().div_ceil(batch_size.max(1));
            let chunks = model.assign(batch_count, batch_size);
            let baseline: Vec<Vec<usize>> =
                balanced_chunks(&(0..subset.len()).collect::<Vec<_>>(), batch_size)
                    .into_iter()
                    .map(|c| c.to_vec())
                    .collect();
            let chosen_obj = model.objective(&chunks);
            let baseline_obj = model.objective(&baseline);
            let opt = &mut plan.explanation.optimization;
            opt.billed_tokens += chosen_obj.billed;
            opt.per_rule_tokens += chosen_obj.per_rule;
            opt.baseline_billed_tokens += baseline_obj.billed;
            opt.baseline_per_rule_tokens += baseline_obj.per_rule;

            let mut judge_plan = JudgePlan {
                judge_index: j,
                batches: Vec::new(),
            };
            for (bi, idxs) in chunks.into_iter().enumerate() {
                let chunk: Vec<&Eligible> = idxs.iter().map(|&i| subset[i]).collect();
                // The call's file list is the union of its rules' *effective*
                // files; a file every rule in the batch ignore-file's simply never
                // appears (recorded as an exclusion in the explanation below).
                let mut files: Vec<PathBuf> = chunk
                    .iter()
                    .flat_map(|e| e.effective.iter().cloned())
                    .collect();
                files.sort();
                files.dedup();

                let excluded = excluded_files(&chunk);
                let reused = reused_files(&chunk);

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
                        .map(|e| RuleSpec {
                            name: e.rule.name.clone(),
                            description: e.rule.description.clone(),
                            rationale: e.rule.rationale,
                            relevance: e.rule.relevance.clone(),
                            require_line_attribution: e.rule.require_line_attribution,
                            // Effective (post-ignore) files, so the per-file
                            // applicability the judge sees never lists a rule for a
                            // file it wholly ignores.
                            files: e.effective.iter().map(|p| to_slash(p)).collect(),
                        })
                        .collect(),
                });
                judge_plan.batches.push(BatchPlan {
                    id: bi + 1,
                    rules: chunk.iter().map(|e| e.rule.name.clone()).collect(),
                    files: files.iter().map(|p| to_slash(p)).collect(),
                    excluded_files: excluded,
                    reused_files: reused,
                });
            }
            agent_plan.judges.push(judge_plan);
        }
        plan.explanation.agents.push(agent_plan);
    }

    // The actual lint set: the distinct union of every batch's files, so the
    // explanation can state "what gets linted" once at the top rather than leaving
    // the reader to union the per-batch lists by hand.
    let mut linted: BTreeSet<String> = BTreeSet::new();
    for a in &plan.explanation.agents {
        for j in &a.judges {
            for b in &j.batches {
                linted.extend(b.files.iter().cloned());
            }
        }
    }
    plan.explanation.linted_files = linted.into_iter().collect();

    let opt = &mut plan.explanation.optimization;
    opt.saved_billed_tokens = opt.baseline_billed_tokens.saturating_sub(opt.billed_tokens);
    opt.saved_per_rule_tokens = opt
        .baseline_per_rule_tokens
        .saturating_sub(opt.per_rule_tokens);
    plan.explanation.skipped = plan
        .skipped
        .iter()
        .map(|s| SkipEntry {
            rule: s.rule.clone(),
            reason: s.reason,
        })
        .collect();
    plan
}

/// The effective (post-ignore) file set of one eligible rule, as slash paths.
fn eligible_files(e: &Eligible) -> BTreeSet<String> {
    e.effective.iter().map(|p| to_slash(p)).collect()
}

/// Build the token-cost model for a judge's rule subset: number each distinct
/// effective file, weight it via the plan context (unit weight when none supplied),
/// and list each rule's file ids. Feeds [`cost::Model::assign`], which returns a
/// provably minimum-cost batch layout.
fn build_cost_model(ctx: &PlanContext, subset: &[&Eligible]) -> cost::Model {
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut weights: Vec<usize> = Vec::new();
    let mut items: Vec<Vec<usize>> = Vec::with_capacity(subset.len());
    for e in subset {
        let mut files = Vec::new();
        for f in &e.effective {
            let slash = to_slash(f);
            let id = *ids.entry(slash.clone()).or_insert_with(|| {
                weights.push(ctx.weight(&slash));
                weights.len() - 1
            });
            files.push(id);
        }
        items.push(files);
    }
    cost::Model::new(items, weights)
}

/// Files used by two or more rules in a batch — the reuse batching captured (shown
/// in the explanation to justify the grouping). Deterministic order.
fn reused_files(chunk: &[&Eligible]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in chunk {
        for f in eligible_files(e) {
            *counts.entry(f).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .map(|(f, _)| f)
        .collect()
}

/// The files dropped entirely from a batch because *every* rule in the batch that
/// declared the file also `ignore-file`s it — so the union no longer carries it.
/// Each entry names the rules that ignored it, for the explanation. Deterministic
/// order (by file, then rule).
fn excluded_files(chunk: &[&Eligible]) -> Vec<ExcludedFile> {
    // Every file any rule in the batch declared.
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for e in chunk {
        for f in &e.rule.files {
            declared.insert(to_slash(f));
        }
    }
    // Files that survived into some rule's effective scope stay in the union.
    let mut effective: BTreeSet<String> = BTreeSet::new();
    for e in chunk {
        for f in &e.effective {
            effective.insert(to_slash(f));
        }
    }
    declared
        .into_iter()
        .filter(|f| !effective.contains(f))
        .map(|file| {
            let rules = chunk
                .iter()
                .filter(|e| e.rule.files.iter().any(|f| to_slash(f) == file))
                .map(|e| e.rule.name.clone())
                .collect();
            ExcludedFile { file, rules }
        })
        .collect()
}

/// A readable account of how the runs were planned — built alongside them so it
/// can never disagree with what actually ran. Rendered into the report at `-v`,
/// persisted in history, and printed on its own by `--plan-only`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanExplanation {
    /// One entry per agent that owned any selected rule, in stable (sorted) order.
    pub agents: Vec<AgentPlan>,
    /// The distinct files the plan will actually send to a judge — the union across
    /// every batch of every agent, deduplicated and sorted. This is the real
    /// "what gets linted" set, surfaced at the top level so a reader sees it (and
    /// its size) at a glance instead of counting across batches. Built while
    /// planning, so it can never drift from the batches.
    pub linted_files: Vec<String>,
    /// Files that matched the configured globs but were dropped *before* planning —
    /// today only by `--diff` narrowing (unchanged vs the base, or a deleted path
    /// with no file left to read). The planner itself is diff-unaware, so the `lint`
    /// command sets this after building; empty otherwise. Surfaced so a `--diff` run
    /// makes clear the glob set was larger than what's actually linted, rather than
    /// silently showing a smaller set than the config implies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diff_excluded_files: Vec<String>,
    /// Rules that were not judged, each with the reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkipEntry>,
    /// The batching cost outcome: the file-load total the chosen layout pays vs the
    /// order-based baseline, and what grouping saved.
    pub optimization: Optimization,
}

/// The counterfactual for the batching decision, in estimated tokens (unit file
/// counts when no weights were supplied). `billed_tokens` is the file content the
/// chosen layout bills (a file re-billed per batch it lands in); `per_rule_tokens`
/// is the per-rule exposure (Σ over rules of their batch's file tokens — the
/// quality axis). Each has a `baseline_*` (what order-based chunking would pay) and
/// a `saved_*` (baseline − chosen, ≥ 0).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Optimization {
    pub billed_tokens: usize,
    pub baseline_billed_tokens: usize,
    pub saved_billed_tokens: usize,
    pub per_rule_tokens: usize,
    pub baseline_per_rule_tokens: usize,
    pub saved_per_rule_tokens: usize,
}

/// One agent's slice of the plan: its config knobs and the judge calls it drives.
#[derive(Debug, Clone, Serialize)]
pub struct AgentPlan {
    pub agent: String,
    pub batch_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    pub judges: Vec<JudgePlan>,
}

/// One judge index within an agent (multi-judge rules expand across indices), and
/// the batches it splits its rules into.
#[derive(Debug, Clone, Serialize)]
pub struct JudgePlan {
    pub judge_index: u32,
    pub batches: Vec<BatchPlan>,
}

/// One judge call: the rules judged together and the file union they see, plus any
/// files dropped from that union as fully ignored.
#[derive(Debug, Clone, Serialize)]
pub struct BatchPlan {
    /// 1-based batch number within its judge index.
    pub id: usize,
    pub rules: Vec<String>,
    /// The effective file union the judge is sent (forward-slash paths).
    pub files: Vec<String>,
    /// Files a batch would have carried but every declaring rule `ignore-file`s.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excluded_files: Vec<ExcludedFile>,
    /// Files used by two or more of the batch's rules — the shared scope that made
    /// grouping them worthwhile (paid once instead of once per batch).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reused_files: Vec<String>,
}

/// A file dropped from a batch's union, and the rules whose whole-file ignore
/// dropped it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExcludedFile {
    pub file: String,
    pub rules: Vec<String>,
}

/// A rule left out of the run, with a human phrase for the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkipEntry {
    pub rule: String,
    pub reason: SkipReason,
}

impl SkipReason {
    /// A short human phrase for the reason, used in the rendered explanation.
    fn phrase(self) -> &'static str {
        match self {
            SkipReason::NoFiles => "no files matched",
            SkipReason::AllFilesIgnored => "all matching files ignored (ignore-file)",
        }
    }
}

impl PlanExplanation {
    /// True when nothing was planned or skipped — used to omit the section
    /// entirely for a zero-rule run.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.skipped.is_empty()
    }

    /// The total number of judge calls the plan will make.
    pub fn total_runs(&self) -> usize {
        self.agents
            .iter()
            .flat_map(|a| &a.judges)
            .map(|j| j.batches.len())
            .sum()
    }

    /// Render the explanation as an indented, plain-text tree. Deterministic
    /// wording (no timestamps/ordering surprises) so it can be snapshot-tested and
    /// diffed. The caller decides where it goes (the `-v` report, `--plan-only`
    /// stdout, a history dump).
    pub fn to_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Plan: {} judge call(s) across {} agent(s), linting {} file(s)\n",
            self.total_runs(),
            self.agents.len(),
            self.linted_files.len(),
        ));
        // Under `--diff`, files that matched the globs but didn't change are dropped
        // before planning — call that out so the smaller lint set is explained, not
        // a mystery. Named up to a small cap, then summarized, so a huge glob set
        // doesn't flood the plan (the full list stays in `--format json`).
        if !self.diff_excluded_files.is_empty() {
            const SHOWN: usize = 8;
            let n = self.diff_excluded_files.len();
            let head = self
                .diff_excluded_files
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = n.saturating_sub(SHOWN);
            let suffix = if more > 0 {
                format!(", …+{more} more")
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  {n} file(s) matched globs but excluded as unchanged/deleted vs base (--diff): {head}{suffix}\n",
            ));
        }
        for a in &self.agents {
            out.push_str(&format!(
                "  agent \"{}\" (batch_size {}{}{})\n",
                a.agent,
                a.batch_size,
                a.model
                    .as_deref()
                    .map(|m| format!(", model {m}"))
                    .unwrap_or_default(),
                a.harness
                    .as_deref()
                    .map(|h| format!(", harness {h}"))
                    .unwrap_or_default(),
            ));
            for j in &a.judges {
                let multi = a.judges.len() > 1;
                if multi {
                    out.push_str(&format!(
                        "    judge {} — {} batch(es)\n",
                        j.judge_index,
                        j.batches.len()
                    ));
                }
                let indent = if multi { "      " } else { "    " };
                for b in &j.batches {
                    out.push_str(&format!(
                        "{indent}batch {}: [{}]\n",
                        b.id,
                        b.rules.join(", ")
                    ));
                    if b.files.len() == 1 {
                        out.push_str(&format!("{indent}  file: {}\n", b.files[0]));
                    } else {
                        out.push_str(&format!(
                            "{indent}  {} files: {}\n",
                            b.files.len(),
                            b.files.join(", ")
                        ));
                    }
                    if !b.reused_files.is_empty() {
                        out.push_str(&format!(
                            "{indent}  grouped: shares {} with other rules in this batch (paid once)\n",
                            b.reused_files.join(", ")
                        ));
                    }
                    for x in &b.excluded_files {
                        out.push_str(&format!(
                            "{indent}  excluded {}: ignored (ignore-file) by {}\n",
                            x.file,
                            x.rules.join(", ")
                        ));
                    }
                }
            }
        }
        // The counterfactual: worth a line when grouping saved billed tokens or
        // tightened per-rule focus over the order-based baseline.
        let opt = &self.optimization;
        if opt.saved_billed_tokens > 0 || opt.saved_per_rule_tokens > 0 {
            out.push_str(&format!(
                "  batching: ~{} file tokens billed (down from {} order-based, saved {}); \
                 per-rule exposure ~{} (down from {}, saved {})\n",
                opt.billed_tokens,
                opt.baseline_billed_tokens,
                opt.saved_billed_tokens,
                opt.per_rule_tokens,
                opt.baseline_per_rule_tokens,
                opt.saved_per_rule_tokens,
            ));
        }
        if !self.skipped.is_empty() {
            out.push_str("  not judged:\n");
            for s in &self.skipped {
                out.push_str(&format!("    {} — {}\n", s.rule, s.reason.phrase()));
            }
        }
        out
    }
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

    /// Build a plan with no inline-ignore suppressions (every declared file is
    /// effective) — the common case for the batching/expansion tests.
    fn bp(cfg: &Config, tmpl: &str, bs: usize, rules: Vec<ResolvedRule>) -> Plan {
        let empty = BTreeMap::new();
        build(cfg, tmpl, bs, rules, &PlanContext::new(&empty))
    }

    /// Build a plan whose context marks the given `(file, rule)` pairs as
    /// whole-file-ignored (an `ignore-file` for that rule in that file).
    fn bp_ignoring(cfg: &Config, rules: Vec<ResolvedRule>, ignores: &[(&str, &str)]) -> Plan {
        use crate::domain::ignore::suppressions;
        use std::collections::BTreeSet;
        // Group the ignored rules per file so a file ignored by several rules
        // yields one `ignore-file[a, b]` directive (not one that overwrites the
        // other).
        let mut by_file: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (file, rule) in ignores {
            by_file.entry(file).or_default().insert(rule);
        }
        let mut per_file: BTreeMap<String, Suppressions> = BTreeMap::new();
        for (file, rules) in by_file {
            let list = rules.iter().copied().collect::<Vec<_>>().join(", ");
            let text = format!("// llmlint: ignore-file[{list}] test fixture\n");
            per_file.insert(file.to_string(), suppressions(&text, &rules));
        }
        build(cfg, "T", 20, rules, &PlanContext::new(&per_file))
    }

    fn rr(name: &str, judges: u32, agent: &str, files: &[&str]) -> ResolvedRule {
        ResolvedRule {
            name: name.into(),
            description: format!("desc {name}"),
            judges,
            agent: agent.into(),
            files: files.iter().map(PathBuf::from).collect(),
            rationale: true,
            relevance: None,
            require_line_attribution: false,
        }
    }

    #[test]
    fn single_judge_rules_run_once() {
        let cfg = Config::default();
        let plan = bp(
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
        let plan = bp(
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
        let plan = bp(&cfg, "MASTER", 20, rules);
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
        let plan = bp(&cfg, "T", 20, rules);
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
        let plan = bp(&cfg, "T", 20, vec![on, off]);
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
        let plan = bp(&cfg, "T", 20, vec![conditional, always]);
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
    fn require_line_attribution_flows_into_the_rule_spec() {
        let cfg = Config::default();
        let mut strict = rr("strict", 1, "default", &["f.rs"]);
        let lax = rr("lax", 1, "default", &["f.rs"]);
        strict.require_line_attribution = true;
        let plan = bp(&cfg, "T", 20, vec![strict, lax]);
        let specs = &plan.runs[0].rules;
        let find = |n: &str| {
            specs
                .iter()
                .find(|r| r.name == n)
                .unwrap()
                .require_line_attribution
        };
        assert!(find("strict"));
        assert!(!find("lax"));
    }

    #[test]
    fn distinct_file_sets_merge_into_one_call_over_the_union() {
        // Two rules on different files now share one judge call (fewer oneharness
        // invocations); the call carries the union of files, and each rule keeps
        // its own scoped file list for the per-file applicability context.
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "default", &["x.rs"]),
                rr("b", 1, "default", &["y.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 1);
        let run = &plan.runs[0];
        assert_eq!(
            run.files,
            vec![PathBuf::from("x.rs"), PathBuf::from("y.rs")]
        );
        let a = run.rules.iter().find(|r| r.name == "a").unwrap();
        let b = run.rules.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(a.files, vec!["x.rs"]);
        assert_eq!(b.files, vec!["y.rs"]);
    }

    #[test]
    fn batch_size_splits_the_union_merge_too() {
        // Even across distinct file sets, batch_size caps rules per call.
        let mut cfg = Config::default();
        cfg.agents.insert(
            "small".into(),
            Agent {
                batch_size: Some(1),
                ..Default::default()
            },
        );
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "small", &["x.rs"]),
                rr("b", 1, "small", &["y.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 2, "batch_size 1 -> one rule per call");
    }

    #[test]
    fn empty_file_set_is_skipped_not_run() {
        let cfg = Config::default();
        let plan = bp(&cfg, "T", 20, vec![rr("a", 1, "default", &[])]);
        assert!(plan.runs.is_empty());
        assert_eq!(
            plan.skipped,
            vec![Skip {
                rule: "a".into(),
                reason: SkipReason::NoFiles,
            }]
        );
    }

    #[test]
    fn a_fully_ignored_rule_is_skipped_as_all_files_ignored() {
        // Rule `a` covers only f.rs, which it `ignore-file`s -> nothing to judge,
        // reported distinctly from a no-files skip.
        let cfg = Config::default();
        let plan = bp_ignoring(
            &cfg,
            vec![rr("a", 1, "default", &["f.rs"])],
            &[("f.rs", "a")],
        );
        assert!(plan.runs.is_empty());
        assert_eq!(
            plan.skipped,
            vec![Skip {
                rule: "a".into(),
                reason: SkipReason::AllFilesIgnored,
            }]
        );
    }

    #[test]
    fn file_ignored_for_one_rule_is_dropped_from_its_scope_but_kept_for_others() {
        // a and b both cover shared.rs; only a `ignore-file`s it. b covers b.rs
        // too. The union keeps shared.rs (b still needs it); a's effective scope
        // loses it, so the judge is told a does not apply there.
        let cfg = Config::default();
        let plan = bp_ignoring(
            &cfg,
            vec![
                rr("a", 1, "default", &["shared.rs"]),
                rr("b", 1, "default", &["shared.rs", "b.rs"]),
            ],
            &[("shared.rs", "a")],
        );
        // a's only file was ignored -> a is skipped; b runs over both files.
        assert_eq!(
            plan.skipped,
            vec![Skip {
                rule: "a".into(),
                reason: SkipReason::AllFilesIgnored,
            }]
        );
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(
            plan.runs[0].files,
            vec![PathBuf::from("b.rs"), PathBuf::from("shared.rs")]
        );
        // Only b remains in the batch, scoped to both its files.
        let names: Vec<&str> = plan.runs[0].rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn a_file_ignored_by_every_declaring_rule_is_excluded_from_the_batch() {
        // Both a and b cover gen.rs and both ignore-file it; a also covers a.rs.
        // gen.rs leaves the union entirely and shows up as an exclusion.
        let cfg = Config::default();
        let plan = bp_ignoring(
            &cfg,
            vec![
                rr("a", 1, "default", &["a.rs", "gen.rs"]),
                rr("b", 1, "default", &["gen.rs"]),
            ],
            &[("gen.rs", "a"), ("gen.rs", "b")],
        );
        // b lost its only file -> skipped; a keeps a.rs.
        assert_eq!(
            plan.skipped,
            vec![Skip {
                rule: "b".into(),
                reason: SkipReason::AllFilesIgnored,
            }]
        );
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].files, vec![PathBuf::from("a.rs")]);
        // The explanation records gen.rs as excluded by the rules that ignored it.
        let batch = &plan.explanation.agents[0].judges[0].batches[0];
        assert_eq!(
            batch.excluded_files,
            vec![ExcludedFile {
                file: "gen.rs".into(),
                rules: vec!["a".into()],
            }],
            "gen.rs excluded, attributed to the batch rule (a) that declared+ignored it"
        );
    }

    #[test]
    fn different_agents_are_never_co_batched_even_when_identical() {
        // Two agents with byte-identical config and overlapping files. Merging
        // would save tokens, but an agent split is an isolation boundary the
        // planner must honor: two separate runs, never one.
        let mut cfg = Config::default();
        for name in ["alpha", "beta"] {
            cfg.agents.insert(
                name.into(),
                Agent {
                    batch_size: Some(20),
                    ..Default::default()
                },
            );
        }
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("r1", 1, "alpha", &["shared.rs"]),
                rr("r2", 1, "beta", &["shared.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 2, "one run per agent, never merged");
        let agents: BTreeSet<&str> = plan.runs.iter().map(|r| r.agent.as_str()).collect();
        assert_eq!(agents, ["alpha", "beta"].into_iter().collect());
    }

    #[test]
    fn explanation_mirrors_the_runs_and_renders_a_readable_tree() {
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "default", &["x.rs"]),
                rr("b", 1, "default", &["y.rs"]),
            ],
        );
        let ex = &plan.explanation;
        assert_eq!(ex.total_runs(), plan.runs.len());
        assert_eq!(ex.agents.len(), 1);
        assert_eq!(ex.agents[0].agent, "default");
        let text = ex.to_human();
        assert!(
            text.contains("Plan: 1 judge call(s) across 1 agent(s)"),
            "{text}"
        );
        assert!(text.contains("agent \"default\" (batch_size 20)"), "{text}");
        assert!(text.contains("batch 1: [a, b]"), "{text}");
        assert!(text.contains("x.rs"), "{text}");
    }

    #[test]
    fn linted_files_is_the_distinct_union_and_the_header_states_its_size() {
        // Two rules over two distinct files -> the plan lints two files, deduped
        // across batches, and the header states the count.
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 1, "default", &["x.rs"]),
                rr("b", 1, "default", &["y.rs", "x.rs"]),
            ],
        );
        assert_eq!(
            plan.explanation.linted_files,
            vec!["x.rs".to_string(), "y.rs".into()],
            "distinct, sorted union of every batch's files"
        );
        let text = plan.explanation.to_human();
        assert!(
            text.contains("Plan: 1 judge call(s) across 1 agent(s), linting 2 file(s)"),
            "{text}"
        );
        // With no diff narrowing there is no exclusion line.
        assert!(!text.contains("excluded as unchanged"), "{text}");
    }

    #[test]
    fn diff_excluded_files_render_a_summary_line() {
        // The `lint` command sets `diff_excluded_files` after building; the header
        // then names them so a `--diff` run explains its smaller lint set.
        let cfg = Config::default();
        let mut plan = bp(&cfg, "T", 20, vec![rr("a", 1, "default", &["x.rs"])]);
        plan.explanation.diff_excluded_files =
            vec!["unchanged_a.rs".into(), "unchanged_b.rs".into()];
        let text = plan.explanation.to_human();
        assert!(text.contains("linting 1 file(s)"), "{text}");
        assert!(
            text.contains(
                "2 file(s) matched globs but excluded as unchanged/deleted vs base (--diff): \
                 unchanged_a.rs, unchanged_b.rs"
            ),
            "{text}"
        );
    }

    #[test]
    fn many_diff_excluded_files_are_capped_with_a_more_suffix() {
        // A large excluded set is truncated in the human view (the full list stays
        // in JSON), so the plan never floods.
        let cfg = Config::default();
        let mut plan = bp(&cfg, "T", 20, vec![rr("a", 1, "default", &["x.rs"])]);
        plan.explanation.diff_excluded_files =
            (0..10).map(|i| format!("f{i}.rs")).collect::<Vec<_>>();
        let text = plan.explanation.to_human();
        assert!(text.contains("10 file(s) matched globs"), "{text}");
        assert!(text.contains("…+2 more"), "{text}");
        // Only the first 8 are named inline.
        assert!(text.contains("f7.rs"), "{text}");
        assert!(!text.contains("f8.rs"), "{text}");
    }

    #[test]
    fn explanation_lists_skipped_rules_with_their_reason() {
        let cfg = Config::default();
        let plan = bp(&cfg, "T", 20, vec![rr("a", 1, "default", &[])]);
        let text = plan.explanation.to_human();
        assert!(text.contains("not judged:"), "{text}");
        assert!(text.contains("a — no files matched"), "{text}");
    }

    #[test]
    fn affinity_groups_shared_scopes_to_cut_duplicate_file_loads() {
        // Four rules, two file scopes, interleaved so order-based chunking (bs 2)
        // would split each scope across both batches (4 file loads). Affinity groups
        // by shared file (2 loads), a strict win, so it is chosen.
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            2, // default_batch_size -> the "default" agent's batch size
            vec![
                rr("a", 1, "default", &["shared_x.rs"]),
                rr("c", 1, "default", &["shared_y.rs"]),
                rr("b", 1, "default", &["shared_x.rs"]),
                rr("d", 1, "default", &["shared_y.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 2);
        // Each run carries exactly one file — the scope its two rules share.
        for run in &plan.runs {
            assert_eq!(run.files.len(), 1, "each batch is one shared scope");
            assert_eq!(run.rules.len(), 2);
        }
        // The batches are the shared-scope groups, not the input order.
        let groups: BTreeSet<Vec<String>> = plan
            .runs
            .iter()
            .map(|r| {
                let mut ns: Vec<String> = r.rules.iter().map(|s| s.name.clone()).collect();
                ns.sort();
                ns
            })
            .collect();
        assert_eq!(
            groups,
            [
                vec!["a".to_string(), "b".into()],
                vec!["c".into(), "d".into()]
            ]
            .into_iter()
            .collect()
        );
        // The counterfactual (unit weights): 2 file tokens billed vs 4 order-based,
        // per-rule exposure 2 vs 8 (each of 4 rules read a 2-file union order-based).
        let opt = plan.explanation.optimization;
        assert_eq!(opt.billed_tokens, 2);
        assert_eq!(opt.baseline_billed_tokens, 4);
        assert_eq!(opt.saved_billed_tokens, 2);
        // per_rule: chosen 2×1 + 2×1 = 4; baseline each rule reads a 2-file union
        // (2×2 + 2×2 = 8) -> saved 4.
        assert_eq!(opt.per_rule_tokens, 4);
        assert_eq!(opt.baseline_per_rule_tokens, 8);
        assert_eq!(opt.saved_per_rule_tokens, 4);
        // Rendered explanation names the reuse and the saving.
        let text = plan.explanation.to_human();
        assert!(text.contains("grouped: shares shared_x.rs"), "{text}");
        assert!(text.contains("batching: ~2 file tokens billed"), "{text}");
    }

    #[test]
    fn a_single_batch_run_keeps_order_and_reports_no_saving() {
        // When everything fits one batch, affinity ties order-based -> order kept,
        // no counterfactual line, saved 0.
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("b", 1, "default", &["f.rs"]),
                rr("a", 1, "default", &["f.rs"]),
            ],
        );
        assert_eq!(plan.runs.len(), 1);
        // Input order is preserved (a single batch is the whole subset in order).
        let names: Vec<&str> = plan.runs[0].rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a"]);
        assert_eq!(plan.explanation.optimization.saved_billed_tokens, 0);
        assert_eq!(plan.explanation.optimization.saved_per_rule_tokens, 0);
        assert!(!plan.explanation.to_human().contains("batching:"));
    }

    #[test]
    fn multi_judge_explanation_labels_each_judge_index() {
        // A judges:2 rule renders per-judge-index headers in the tree.
        let cfg = Config::default();
        let plan = bp(
            &cfg,
            "T",
            20,
            vec![
                rr("a", 2, "default", &["f.rs"]),
                rr("b", 1, "default", &["f.rs"]),
            ],
        );
        let text = plan.explanation.to_human();
        assert!(text.contains("judge 1 — 1 batch(es)"), "{text}");
        assert!(text.contains("judge 2 — 1 batch(es)"), "{text}");
    }

    #[test]
    fn affinity_assignment_is_deterministic() {
        let cfg = Config::default();
        let rules = || {
            vec![
                rr("a", 1, "default", &["x.rs"]),
                rr("c", 1, "default", &["y.rs"]),
                rr("b", 1, "default", &["x.rs"]),
                rr("d", 1, "default", &["y.rs"]),
            ]
        };
        let p1 = bp(&cfg, "T", 2, rules());
        let p2 = bp(&cfg, "T", 2, rules());
        let names = |p: &Plan| -> Vec<Vec<String>> {
            p.runs
                .iter()
                .map(|r| r.rules.iter().map(|s| s.name.clone()).collect())
                .collect()
        };
        assert_eq!(names(&p1), names(&p2), "same input -> same plan");
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
        let plan = bp(&cfg, "T", 20, vec![rr("a", 1, "arch", &["f.rs"])]);
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
        let plan = bp(&cfg, "T", 20, vec![rr("a", 1, "arch", &["f.rs"])]);
        assert_eq!(plan.runs[0].harness, None);
        assert_eq!(plan.runs[0].model.as_deref(), Some("gpt-5"));
    }
}
