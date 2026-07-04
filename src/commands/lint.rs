//! `llmlint lint` (the default): load config, resolve files, plan judge runs,
//! drive oneharness in parallel, aggregate votes, and report.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;

use indicatif::ProgressDrawTarget;

use crate::cli::{ColorChoice, LintArgs, OutputFormat};
use crate::commands::ignores;
use crate::commands::progress::{LiveStatus, ProgressView};
use crate::domain::config::{validate, Config, RelevanceMode, Rule};
use crate::domain::ignore::Suppressions;
use crate::domain::plan::{self, JudgeRun};
use crate::domain::report::Report;
use crate::domain::template::{self};
use crate::domain::verdict::{Outcome, RuleOutcome, RuleVerdict};
use crate::domain::{applicability, attribution, ignore, schema, vote};
use crate::errors::{io_err, Error, Result};
use crate::io::configfs::RuleScope;
use crate::io::{assets, configfs, diff, files, history, oneharness};

const DEFAULT_BATCH_SIZE: usize = 20;
const DEFAULT_TIMEOUT: u64 = 120;
const DEFAULT_MAX_PARALLEL: usize = 8;
const PROMPT_TRIGGER: &str =
    "Evaluate each rule against the target files and respond with the structured verdict object.";
/// How many times a judge whose verdict strays outside a rule's file scope is
/// asked to rework its answer before llmlint drops the wrong-file violations
/// deterministically. One corrective round catches an honest slip without
/// looping on a judge that won't comply.
const MAX_REWORKS: usize = 1;

pub fn run(args: LintArgs) -> Result<i32> {
    let cwd = resolve_cwd(&args.cwd)?;

    // Explicit CLI files relevance-gate the subtree cascade: linting specific
    // files never loads an unrelated subtree's config (see `load_with_targets`).
    let loaded = configfs::load_with_targets(&args.config, &cwd, &args.files)?;
    run_loaded(loaded, cwd, args, "lint")
}

/// Resolve the working directory for a run: the `--cwd` override, else the
/// process cwd. Shared by `lint` and the `lint-config` subcommand.
pub(crate) fn resolve_cwd(arg: &Option<PathBuf>) -> Result<PathBuf> {
    match arg {
        Some(d) => Ok(d.clone()),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string())),
    }
}

/// Drive a lint from an already-loaded config. Split out from [`run`] so the
/// `lint-config` subcommand can hand in the bundled config-lint config (loaded
/// without discovery) and reuse the entire engine — validation, planning,
/// judging, voting, and reporting — unchanged.
pub(crate) fn run_loaded(
    loaded: configfs::Loaded,
    cwd: PathBuf,
    args: LintArgs,
    command: &str,
) -> Result<i32> {
    let scopes = loaded.scopes;
    let sources = loaded.sources;
    let mut config = loaded.config;
    validate(&config)?;
    validate_filters(&config, &args)?;
    // Overlay CLI overrides onto the merged config so every top-level arg can be
    // set on the command line, with the CLI winning over the config (which in
    // turn won over its plugins). After this, downstream (planning, schema,
    // template) reads the single effective config.
    apply_cli_overrides(&mut config, &args)?;
    let session_rationales = config.rationales_default();

    let selected = select_rules(&config, &args);
    if selected.is_empty() {
        let report = Report::new(Vec::new(), Vec::new());
        return Ok(finish(&report, &args, &cwd, &sources, command, &config));
    }

    let master_template = config
        .prompt_template
        .clone()
        .unwrap_or_else(|| assets::DEFAULT_TEMPLATE.to_string());
    let cli_files = files::from_cli(&cwd, &args.files);

    let mut resolved = Vec::new();
    // Rules declared statically not relevant (`relevance: false`) never reach a
    // judge — they are reported as not relevant directly.
    let mut not_relevant: Vec<String> = Vec::new();
    for rule in &selected {
        let relevance = match rule.relevance_mode() {
            RelevanceMode::Never => {
                not_relevant.push(rule.name.clone());
                continue;
            }
            RelevanceMode::Always => None,
            RelevanceMode::Conditional(cond) => Some(cond),
        };
        let agent_name = rule.agent.clone().unwrap_or_else(|| "default".to_string());
        let fallback;
        let scope = match scopes.get(&rule.name) {
            Some(s) => s,
            // Every selected rule has a scope; fall back to cwd defensively.
            None => {
                fallback = RuleScope {
                    dir: cwd.clone(),
                    files: config.files.clone(),
                };
                &fallback
            }
        };
        let target = ignores::resolve_files(&cwd, rule, &cli_files, scope)?;
        resolved.push(plan::ResolvedRule {
            name: rule.name.clone(),
            description: rule.description.clone(),
            judges: rule.judges(),
            agent: agent_name,
            files: target,
            rationale: rule.wants_rationale(session_rationales),
            relevance,
            require_line_attribution: rule.requires_line_attribution(),
        });
    }

    // Reject malformed inline `llmlint: ignore` directives in the target files
    // before spending any judge calls. Honoring well-formed ones is the judge's
    // job (the default template tells it how); their *structure* is enforced here
    // so a typo'd or reason-less ignore fails loudly instead of silently doing
    // nothing. This is the same check `llmlint check-ignores` runs standalone, so
    // the fast static loop and the full run never disagree.
    let targets: BTreeSet<PathBuf> = resolved
        .iter()
        .flat_map(|r| r.files.iter().cloned())
        .collect();
    let known = ignores::known_rules(&config);
    ignores::check(&cwd, &targets, &known)?;

    // Parse each target file's well-formed inline ignores into line-span
    // suppressions, keyed by the file's slash path. After a judge answers, any
    // violation an ignore covers is dropped deterministically — llmlint honors
    // the directives itself rather than trusting the judge to (the default
    // template still documents line/block ignores as a backstop).
    let mut suppressions: BTreeMap<String, Suppressions> = BTreeMap::new();
    for rel in &targets {
        if let Some(text) = files::read_text(&cwd, rel)? {
            let s = ignore::suppressions(&text, &known);
            if !s.is_empty() {
                suppressions.insert(files::to_slash(rel), s);
            }
        }
    }

    // Under `--diff`, compute each target file's changed-line diff once (at the
    // I/O boundary) so every judge prompt can show exactly what changed. The
    // backend is selected behind the `DiffProvider` trait, comparing against
    // `--diff-base` (a branch/tag/commit/range) or the backend default; an
    // unchanged file simply has no entry. Absent the flag this is empty and
    // nothing renders.
    let diffs: BTreeMap<PathBuf, String> = match args.diff {
        Some(backend) => {
            let targets: Vec<PathBuf> = targets.iter().cloned().collect();
            // The base is the effective config value: `--diff-base` already won
            // over a config `diff_base` in `apply_cli_overrides`; `None` leaves
            // the backend's built-in default (`HEAD` for git).
            diff::provider(backend, config.diff_base.clone()).diffs(&cwd, &targets)?
        }
        None => BTreeMap::new(),
    };

    // Rules whose rationale is disabled: llmlint is authoritative, so we drop any
    // rationale a harness returns anyway, keeping `--no-rationales` deterministic
    // regardless of harness behavior.
    let rationale_off: HashSet<String> = resolved
        .iter()
        .filter(|r| !r.rationale)
        .map(|r| r.name.clone())
        .collect();

    // Rules that opted into line attribution: every violation they surface must
    // cite a file+line. The schema makes oneharness re-prompt for it; this set
    // backs the deterministic post-vote backstop below.
    let require_attribution: BTreeSet<String> = resolved
        .iter()
        .filter(|r| r.require_line_attribution)
        .map(|r| r.name.clone())
        .collect();

    let the_plan = plan::build(&config, &master_template, DEFAULT_BATCH_SIZE, resolved);

    let bin = args
        .oneharness_bin
        .clone()
        .or_else(|| {
            std::env::var("LLMLINT_ONEHARNESS_BIN")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| config.oneharness.bin.clone());
    let client = oneharness::Client::new(bin.as_deref());
    // Pre-flight: read-only mode (so the harness never edits target files)
    // requires oneharness >= MIN_VERSION. Check once up front and fail with a
    // clear message rather than letting every judge's `--mode read-only` error.
    client.check_min_version()?;
    let timeout = args
        .timeout
        .or(config.oneharness.timeout)
        .unwrap_or(DEFAULT_TIMEOUT);
    let oh_config = resolve_oneharness_config(&args, &config);
    let oh_config_ref = oh_config.as_deref();
    let global_model = config.oneharness.model.as_deref();
    let max_parallel = args.max_parallel.unwrap_or(DEFAULT_MAX_PARALLEL).max(1);

    let mut verdicts: BTreeMap<String, Vec<RuleVerdict>> = BTreeMap::new();
    let mut run_errors: Vec<String> = Vec::new();
    // At `-v` we collect each oneharness invocation's exact command + raw result
    // and print them to stderr (the debug view); off by default to avoid the
    // cost of formatting the full command line on every judge.
    let want_trace = args.verbose >= 1;
    let mut traces: Vec<(String, oneharness::RunTrace)> = Vec::new();

    // The live-progress view (rules resolving as their judges return), drawn to
    // stderr for an interactive human. Decide whether to animate from the audience
    // (`--progress` + TTY/CI/agent), then build the view with a real or hidden
    // draw target. It is *always* constructed — hidden when not interactive — so
    // the driving glue below runs on every path (a piped/CI/agent run just draws
    // to nothing, emitting no escape codes). The report on stdout is unaffected.
    let ctx = crate::io::terminal::detect();
    let show_progress = args.format == OutputFormat::Human
        && args
            .progress
            .resolve(ctx.stderr_tty, ctx.is_ci, ctx.is_agent, ctx.color_ok());
    // Rules shown in the view, in a stable (sorted) order: every judged rule plus
    // the ones already resolved without a judge (skipped / statically not
    // relevant), which we finish immediately below.
    let mut view_rules: BTreeSet<String> = BTreeSet::new();
    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    for run in &the_plan.runs {
        for rs in &run.rules {
            view_rules.insert(rs.name.clone());
            *expected.entry(rs.name.clone()).or_default() += 1;
        }
    }
    view_rules.extend(the_plan.skipped.iter().cloned());
    view_rules.extend(not_relevant.iter().cloned());
    let view_rules: Vec<String> = view_rules.into_iter().collect();
    let view = ProgressView::new(
        progress_target(show_progress),
        &view_rules,
        the_plan.runs.len(),
        show_progress,
    );
    for name in &the_plan.skipped {
        view.finish_rule(name, LiveStatus::Skipped);
    }
    for name in &not_relevant {
        view.finish_rule(name, LiveStatus::NotRelevant);
    }

    // Bind Copy references so the per-judge `move` closures capture borrows
    // (which outlive the scope) rather than moving the owned `client`/`cwd`.
    let client_ref = &client;
    let cwd_ref = cwd.as_path();
    let diffs_ref = &diffs;
    let suppressions_ref = &suppressions;
    for wave in the_plan.runs.chunks(max_parallel) {
        // The wave's rules are now in flight — spin their lines.
        for run in wave {
            for rs in &run.rules {
                view.set_running(&rs.name);
            }
        }
        thread::scope(|s| {
            let handles: Vec<_> = wave
                .iter()
                .map(|run| {
                    s.spawn(move || {
                        execute(
                            client_ref,
                            run,
                            cwd_ref,
                            timeout,
                            oh_config_ref,
                            global_model,
                            want_trace,
                            diffs_ref,
                            suppressions_ref,
                        )
                    })
                })
                .collect();
            for (run, handle) in wave.iter().zip(handles) {
                let (trace, result) = handle.join().expect("judge thread panicked");
                if let Some(trace) = trace {
                    traces.push((judge_label(run), trace));
                }
                view.tick_run();
                match result {
                    Ok(map) => {
                        for (name, verdict) in map {
                            verdicts.entry(name).or_default().push(verdict);
                        }
                    }
                    Err(e) => run_errors.push(format!("{}: {}", judge_label(run), e)),
                }
                // Resolve each of this run's rules whose judges are now all in:
                // tally with the same `vote::tally` the report uses, so the live
                // ✓/✗ can never disagree with the final verdict. A rule with no
                // usable verdict (all its runs errored) shows an error glyph.
                for rs in &run.rules {
                    let remaining = expected.get_mut(rs.name.as_str());
                    if let Some(remaining) = remaining {
                        *remaining = remaining.saturating_sub(1);
                        if *remaining == 0 {
                            let status = match verdicts.get(rs.name.as_str()) {
                                Some(vs) if !vs.is_empty() => {
                                    live_status(&vote::tally(&rs.name, vs))
                                }
                                _ => LiveStatus::Error,
                            };
                            view.finish_rule(&rs.name, status);
                        }
                    }
                }
            }
        });
    }
    // Clear the whole view from stderr before any report/trace output, so nothing
    // is interleaved with progress fragments.
    view.finish();

    if want_trace {
        print_traces(&traces);
    }

    let mut outcomes: Vec<RuleOutcome> = verdicts
        .iter()
        .map(|(name, vs)| vote::tally(name, vs))
        .collect();
    for o in &mut outcomes {
        if rationale_off.contains(&o.name) {
            o.rationale = None;
            for j in &mut o.judges {
                j.rationale = None;
            }
        }
    }
    for name in &the_plan.skipped {
        outcomes.push(RuleOutcome::skipped(name));
    }
    for name in &not_relevant {
        outcomes.push(RuleOutcome::not_relevant(name));
    }

    // Backstop the `require_line_attribution` contract: any failing rule that
    // opted in but still surfaced a violation without a file+line is a hard error
    // (the schema already had oneharness re-prompt for it in one batched turn).
    if !require_attribution.is_empty() {
        run_errors.extend(attribution::unlocalized_errors(
            &outcomes,
            &require_attribution,
        ));
    }

    let report = Report::new(outcomes, run_errors);
    Ok(finish(&report, &args, &cwd, &sources, command, &config))
}

/// Emit the report, log the run's full results to disk (best-effort, when results
/// logging is on), and return the process exit code. Shared by both `run_loaded`
/// return paths so every completed run — including a zero-rule one — is both
/// reported and recorded identically.
fn finish(
    report: &Report,
    args: &LintArgs,
    cwd: &Path,
    sources: &[String],
    command: &str,
    config: &Config,
) -> i32 {
    emit(report, args.format, args.verbose, args.color);
    let code = report.exit_code();
    log_history(report, args, cwd, sources, command, config, code);
    code
}

/// Write this run's full results as a JSON record and, for the human report,
/// print the run id + how to retrieve it. Logging is best-effort: any failure is
/// a stderr warning, never a change to the lint's exit code — a broken history
/// dir must not fail an otherwise-good run. Suppressed entirely when logging is
/// off or no history directory can be determined.
fn log_history(
    report: &Report,
    args: &LintArgs,
    cwd: &Path,
    sources: &[String],
    command: &str,
    config: &Config,
    exit_code: i32,
) {
    let settings = history::resolve(config, args.no_history);
    if !settings.enabled {
        return;
    }
    let Some(dir) = &settings.dir else {
        eprintln!(
            "llmlint: warning: results logging is on but no history directory could be \
             determined (set history.dir or LLMLINT_HISTORY_DIR)"
        );
        return;
    };
    let now = std::time::SystemTime::now();
    let id = history::generate_id(now);
    let timestamp = history::format_timestamp(now);
    let record = history::build_record(&id, &timestamp, command, cwd, exit_code, sources, report);
    match history::write_record(dir, &id, &record, settings.max_runs) {
        Ok(_) => {
            // The report on stdout stays the clean report/JSON channel; the note
            // goes to stderr, and only for the human format (a JSON consumer reads
            // the record file itself). This keeps stdout byte-identical to before.
            if args.format == OutputFormat::Human {
                eprintln!("See full results with `llmlint history {id}`");
            }
        }
        Err(e) => eprintln!("llmlint: warning: could not log run results: {e}"),
    }
}

/// Reject `--rule`/`--agent` targets that name nothing in the config. Without
/// this a typo (`--rule no_todos` vs `no_todo`) selects zero rules and exits 0 —
/// a false green. A genuinely-empty-but-valid selection (real names that just
/// don't intersect) is allowed through to exit 0 by `select_rules`. `default`
/// is always a valid agent target (it runs rules with no explicit agent).
fn validate_filters(config: &Config, args: &LintArgs) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();

    if !args.rule.is_empty() {
        let known: HashSet<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        let mut unknown: Vec<&str> = args
            .rule
            .iter()
            .map(String::as_str)
            .filter(|n| !known.contains(n))
            .collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            unknown.dedup();
            let mut available: Vec<&str> = known.into_iter().collect();
            available.sort_unstable();
            problems.push(format!(
                "no rule named {}; available rules: {}",
                unknown.join(", "),
                join_or_none(&available)
            ));
        }
    }

    if let Some(agent) = &args.agent {
        if agent != "default" && !config.agents.contains_key(agent) {
            let mut available: Vec<&str> = config.agents.keys().map(String::as_str).collect();
            if !available.contains(&"default") {
                available.push("default");
            }
            available.sort_unstable();
            problems.push(format!(
                "no agent named {}; available agents: {}",
                agent,
                join_or_none(&available)
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::UnknownFilter(problems.join("; ")))
    }
}

fn join_or_none(names: &[&str]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

fn select_rules<'a>(config: &'a Config, args: &LintArgs) -> Vec<&'a Rule> {
    let rule_filter: Option<HashSet<&str>> = if args.rule.is_empty() {
        None
    } else {
        Some(args.rule.iter().map(String::as_str).collect())
    };
    config
        .rules
        .iter()
        .filter(|r| {
            let agent_ok = args.agent.as_deref().is_none_or(|a| {
                r.agent.as_deref() == Some(a) || (a == "default" && r.agent.is_none())
            });
            let name_ok = rule_filter
                .as_ref()
                .is_none_or(|set| set.contains(r.name.as_str()));
            agent_ok && name_ok
        })
        .collect()
}

/// Overlay the lint CLI's top-level overrides onto the merged config so the CLI
/// wins over the config. Each knob also has a config field and (transitively) a
/// plugin precedence; this is the final, highest-priority layer. `--oneharness-bin`,
/// `--timeout`, `--oneharness-config`, and the file/agent/rule selectors are
/// resolved at their use sites (they fold in env/discovery too) and are not
/// overlaid here.
fn apply_cli_overrides(config: &mut Config, args: &LintArgs) -> Result<()> {
    if let Some(path) = &args.prompt_template {
        let text = std::fs::read_to_string(path)
            .map_err(|e| io_err(format!("reading prompt template {}", path.display()), e))?;
        config.prompt_template = Some(text);
    }
    if let Some(model) = &args.model {
        config.oneharness.model = Some(model.clone());
    }
    if let Some(n) = args.schema_max_retries {
        config.oneharness.schema_max_retries = Some(n);
    }
    if let Some(b) = args.rationales() {
        config.rationales = Some(b);
    }
    if args.diff_base.is_some() {
        config.diff_base = args.diff_base.clone();
    }
    Ok(())
}

fn resolve_oneharness_config(args: &LintArgs, config: &Config) -> Option<PathBuf> {
    let mut all: Vec<PathBuf> = args.oneharness_config.clone();
    all.extend(config.oneharness.config.iter().map(PathBuf::from));
    if all.len() > 1 {
        eprintln!(
            "llmlint: warning: oneharness `--config` takes a single file; using {} and ignoring \
             {} other(s)",
            all[0].display(),
            all.len() - 1
        );
    }
    all.into_iter().next()
}

/// The draw target for the live view: real stderr when it should show (indicatif
/// still self-hides if stderr isn't a terminal), else a hidden target that draws
/// nothing. Split out so the selection is unit-testable off a real TTY.
fn progress_target(show: bool) -> ProgressDrawTarget {
    if show {
        ProgressDrawTarget::stderr()
    } else {
        ProgressDrawTarget::hidden()
    }
}

/// Map a tallied rule outcome to the live view's status glyph.
fn live_status(o: &RuleOutcome) -> LiveStatus {
    match o.outcome {
        Outcome::Pass => LiveStatus::Pass,
        Outcome::Fail => LiveStatus::Fail,
        Outcome::Skipped => LiveStatus::Skipped,
        Outcome::NotRelevant => LiveStatus::NotRelevant,
    }
}

/// A stable per-judge label used for both error messages and the `-v` debug
/// trace headers: `agent <name> judge <i> [<rule>, ...]`.
fn judge_label(run: &JudgeRun) -> String {
    format!(
        "agent {} judge {} [{}]",
        run.agent,
        run.judge_index,
        run.rules
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Print the oneharness debug view (exact command + raw result per judge) to
/// stderr, keeping the parseable report on stdout. Shown at `-v`.
fn print_traces(traces: &[(String, oneharness::RunTrace)]) {
    for (label, trace) in traces {
        eprintln!("\n# oneharness: {label}");
        eprintln!("$ {}", trace.command);
        if let Some(code) = trace.exit_code {
            eprintln!("exit: {code}");
        }
        let stdout = trace.stdout.trim();
        if !stdout.is_empty() {
            eprintln!("result:\n{stdout}");
        }
        let stderr = trace.stderr.trim();
        if !stderr.is_empty() {
            eprintln!("stderr:\n{stderr}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute(
    client: &oneharness::Client,
    run: &JudgeRun,
    cwd: &Path,
    timeout: u64,
    oh_config: Option<&Path>,
    global_model: Option<&str>,
    want_trace: bool,
    diffs: &BTreeMap<PathBuf, String>,
    suppressions: &BTreeMap<String, Suppressions>,
) -> (
    Option<oneharness::RunTrace>,
    Result<BTreeMap<String, RuleVerdict>>,
) {
    let files_str: Vec<String> = run.files.iter().map(|p| files::to_slash(p)).collect();
    // Per-file diffs for this run's files, in the same order as `files_str`. Only
    // files that actually changed appear in `diffs`, so unchanged ones are
    // skipped here; the slice is empty when `--diff` wasn't passed.
    let file_diffs: Vec<template::FileDiff> = run
        .files
        .iter()
        .filter_map(|p| {
            diffs.get(p).map(|d| template::FileDiff {
                file: files::to_slash(p),
                diff: d.clone(),
            })
        })
        .collect();
    // Show the rationale guidance when any rule in this batch wants a rationale,
    // and the relevance guidance when any rule is conditional on relevance.
    let want_rationale = run.rules.iter().any(|r| r.rationale);
    let want_relevance = run.rules.iter().any(|r| r.relevance.is_some());
    let want_line_attribution = run.rules.iter().any(|r| r.require_line_attribution);
    let system = match template::render(
        &run.template,
        &run.rules,
        &files_str,
        &file_diffs,
        want_rationale,
        want_relevance,
        want_line_attribution,
    ) {
        Ok(s) => s,
        // A render failure happens before any oneharness call, so there is no
        // command to trace.
        Err(e) => return (None, Err(e)),
    };
    let specs: Vec<schema::SchemaRule> = run
        .rules
        .iter()
        .map(|r| schema::SchemaRule {
            name: r.name.as_str(),
            rationale: r.rationale,
            relevance: r.relevance.is_some(),
            require_line_attribution: r.require_line_attribution,
        })
        .collect();
    let schema = schema::build(&specs);

    // Per-rule scope (which files each rule covers) + the per-file applicability
    // shown to the judge — reused to validate the verdict and to phrase a rework.
    let pairs: Vec<(String, Vec<String>)> = run
        .rules
        .iter()
        .map(|r| (r.name.clone(), r.files.clone()))
        .collect();
    let scope = applicability::scope_map(&pairs);
    let file_rules = applicability::per_file(&pairs, &files_str);

    // One judge call, plus up to MAX_REWORKS corrective rounds: if the verdict
    // pins a violation to a file outside that rule's scope (a "wrong rule in
    // wrong file"), re-ask with the exact per-file rule lists before falling back
    // to deterministic cleanup.
    let mut prompt = PROMPT_TRIGGER.to_string();
    let mut last_trace;
    let mut verdicts;
    let mut attempt = 0;
    loop {
        let req = oneharness::RunRequest {
            harness: run.harness.as_deref(),
            model: run.model.as_deref().or(global_model),
            system: &system,
            prompt: &prompt,
            schema: &schema,
            schema_max_retries: run.schema_max_retries,
            cwd,
            timeout_secs: timeout,
            oneharness_config: oh_config,
            no_config: false,
        };
        let (trace, result) = if want_trace {
            let (t, r) = client.run_with_trace(&req);
            (Some(t), r)
        } else {
            (None, client.run(&req))
        };
        last_trace = trace;
        verdicts = match result {
            Ok(v) => v,
            Err(e) => return (last_trace, Err(e)),
        };
        let problems = applicability::scope_problems(&scope, &verdicts);
        if problems.is_empty() || attempt >= MAX_REWORKS {
            break;
        }
        prompt = applicability::rework_prompt(&problems, &file_rules);
        attempt += 1;
    }

    // Deterministic cleanup, applied whether or not a rework ran: drop any
    // remaining wrong-file violation and any an inline ignore covers, flipping a
    // fail to a pass when that removes its entire basis.
    let empty_scope = BTreeSet::new();
    for (name, verdict) in verdicts.iter_mut() {
        let rule_scope = scope.get(name.as_str()).unwrap_or(&empty_scope);
        applicability::clean_verdict(verdict, rule_scope, |file, line| {
            suppressions.get(file).is_some_and(|s| s.covers(name, line))
        });
    }
    (last_trace, Ok(verdicts))
}

fn emit(report: &Report, format: OutputFormat, verbosity: u8, color: ColorChoice) {
    match format {
        OutputFormat::Human => {
            // Resolve `--color` against the live stdout: `auto` colors only an
            // interactive terminal with `NO_COLOR`/`TERM=dumb` unset that is not an
            // AI agent (captured ANSI is unreliable there). The decision is made
            // here (the I/O boundary) and handed to the pure formatter as a bool.
            let ctx = crate::io::terminal::detect();
            let on = color.resolve(std::io::stdout().is_terminal(), ctx.no_color, ctx.is_agent);
            // Write through anstream's `AutoStream` so the ANSI we emit renders
            // everywhere, not just on Unix. On a *legacy* Windows console (no
            // virtual-terminal processing) raw ANSI prints as `←[31m` garbage;
            // `AutoStream` enables VT when it can and otherwise translates the SGR
            // codes into Win32 console attribute calls (`anstyle-wincon`). The
            // `--color` decision already lives in `on`, so feed the stream a
            // concrete choice rather than letting it re-detect: `Always` when
            // color is on (never second-guess `--color always`), `Never`
            // otherwise — a no-op strip over already-plain text that keeps the
            // sink uniform. On a pipe/redirect (no console) `Always` still emits
            // plain ANSI on every OS, so `--color always` and the screenshot/e2e
            // byte assertions stay byte-identical.
            let choice = if on {
                anstream::ColorChoice::Always
            } else {
                anstream::ColorChoice::Never
            };
            let mut out = anstream::AutoStream::new(std::io::stdout().lock(), choice);
            let _ = write!(out, "{}", report.to_human(verbosity, on));
            let _ = out.flush();
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::Agent;

    fn rule(name: &str, agent: Option<&str>) -> Rule {
        Rule {
            name: name.into(),
            description: "true when ok; false otherwise.".into(),
            r#override: false,
            agent: agent.map(Into::into),
            judges: None,
            files: None,
            rationale: None,
            relevance: None,
            require_line_attribution: None,
        }
    }

    fn config_with(rules: Vec<Rule>, agents: &[&str]) -> Config {
        let mut c = Config {
            rules,
            ..Default::default()
        };
        for a in agents {
            c.agents.insert((*a).into(), Agent::default());
        }
        c
    }

    fn args(rules: &[&str], agent: Option<&str>) -> LintArgs {
        LintArgs {
            rule: rules.iter().map(|s| (*s).to_string()).collect(),
            agent: agent.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn no_filters_is_ok() {
        let cfg = config_with(vec![rule("a_rule", None)], &[]);
        assert!(validate_filters(&cfg, &args(&[], None)).is_ok());
    }

    #[test]
    fn progress_target_hidden_when_not_showing() {
        // The suppressed path is always a hidden target (draws nothing).
        assert!(progress_target(false).is_hidden());
        // The showing path builds a real stderr target; under the test runner
        // stderr isn't a TTY, so indicatif still resolves it to hidden — the point
        // is the construction line runs. (A real terminal makes it visible.)
        let _ = progress_target(true);
    }

    #[test]
    fn live_status_maps_every_outcome() {
        let outcome = |o: Outcome| RuleOutcome {
            name: "r".into(),
            rationale: None,
            outcome: o,
            votes_total: 1,
            votes_hold: 0,
            judges: vec![],
            violations: vec![],
        };
        assert!(matches!(
            live_status(&outcome(Outcome::Pass)),
            LiveStatus::Pass
        ));
        assert!(matches!(
            live_status(&outcome(Outcome::Fail)),
            LiveStatus::Fail
        ));
        assert!(matches!(
            live_status(&outcome(Outcome::Skipped)),
            LiveStatus::Skipped
        ));
        assert!(matches!(
            live_status(&outcome(Outcome::NotRelevant)),
            LiveStatus::NotRelevant
        ));
    }

    #[test]
    fn known_rule_and_agent_are_ok() {
        let cfg = config_with(vec![rule("a_rule", Some("special"))], &["special"]);
        assert!(validate_filters(&cfg, &args(&["a_rule"], Some("special"))).is_ok());
    }

    #[test]
    fn default_agent_is_always_valid() {
        let cfg = config_with(vec![rule("a_rule", None)], &[]);
        assert!(validate_filters(&cfg, &args(&[], Some("default"))).is_ok());
    }

    #[test]
    fn unknown_rule_lists_available_sorted() {
        let cfg = config_with(vec![rule("beta", None), rule("alpha", None)], &[]);
        let err = validate_filters(&cfg, &args(&["typo", "alpha"], None)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no rule named typo"), "got: {msg}");
        assert!(msg.contains("available rules: alpha, beta"), "got: {msg}");
    }

    #[test]
    fn unknown_agent_lists_available_with_default() {
        let cfg = config_with(vec![rule("a_rule", Some("special"))], &["special"]);
        let err = validate_filters(&cfg, &args(&[], Some("ghost"))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no agent named ghost"), "got: {msg}");
        assert!(
            msg.contains("available agents: default, special"),
            "got: {msg}"
        );
    }

    #[test]
    fn unknown_rule_with_no_rules_says_none() {
        let cfg = config_with(vec![], &[]);
        let err = validate_filters(&cfg, &args(&["x"], None)).unwrap_err();
        assert!(err.to_string().contains("available rules: (none)"));
    }

    #[test]
    fn cli_overrides_win_over_config() {
        let mut cfg = Config {
            rationales: Some(true),
            ..Default::default()
        };
        cfg.oneharness.model = Some("config-model".into());
        let args = LintArgs {
            model: Some("cli-model".into()),
            schema_max_retries: Some(5),
            no_rationales: true,
            ..Default::default()
        };
        apply_cli_overrides(&mut cfg, &args).unwrap();
        assert_eq!(cfg.oneharness.model.as_deref(), Some("cli-model"));
        assert_eq!(cfg.oneharness.schema_max_retries, Some(5));
        assert_eq!(cfg.rationales, Some(false));
        assert!(!cfg.rationales_default());
    }

    #[test]
    fn no_rationale_flags_leaves_config_untouched() {
        let mut cfg = Config {
            rationales: Some(false),
            ..Default::default()
        };
        apply_cli_overrides(&mut cfg, &LintArgs::default()).unwrap();
        // No --model/--schema-max-retries/--rationales: config value survives.
        assert_eq!(cfg.rationales, Some(false));
    }

    #[test]
    fn both_unknown_rule_and_agent_are_reported() {
        let cfg = config_with(vec![rule("a_rule", None)], &[]);
        let err = validate_filters(&cfg, &args(&["nope"], Some("ghost"))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no rule named nope"), "got: {msg}");
        assert!(msg.contains("no agent named ghost"), "got: {msg}");
    }
}
