//! `llmlint lint` (the default): load config, resolve files, plan judge runs,
//! drive oneharness in parallel, aggregate votes, and report.

use std::collections::{BTreeMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::thread;

use crate::cli::{ColorChoice, LintArgs, OutputFormat};
use crate::domain::config::{validate, Agent, Config, FileFilter, Rule};
use crate::domain::plan::{self, JudgeRun};
use crate::domain::report::Report;
use crate::domain::template::{self};
use crate::domain::verdict::{RuleOutcome, RuleVerdict};
use crate::domain::{schema, vote};
use crate::errors::{io_err, Error, Result};
use crate::io::{assets, configfs, files, oneharness};

const DEFAULT_BATCH_SIZE: usize = 20;
const DEFAULT_TIMEOUT: u64 = 120;
const DEFAULT_MAX_PARALLEL: usize = 8;
const PROMPT_TRIGGER: &str =
    "Evaluate each rule against the target files and respond with the structured verdict object.";

pub fn run(args: LintArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };

    let loaded = configfs::load(&args.config, &cwd)?;
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
        emit(&report, args.format, args.verbose, args.color);
        return Ok(report.exit_code());
    }

    let master_template = config
        .prompt_template
        .clone()
        .unwrap_or_else(|| assets::DEFAULT_TEMPLATE.to_string());
    let cli_files = files::from_cli(&cwd, &args.files);

    let mut resolved = Vec::new();
    for rule in &selected {
        let agent_name = rule.agent.clone().unwrap_or_else(|| "default".to_string());
        let agent = config.agent_or_default(&agent_name);
        let target = resolve_files(&cwd, rule, &agent, &cli_files, &config.files)?;
        resolved.push(plan::ResolvedRule {
            name: rule.name.clone(),
            description: rule.description.clone(),
            judges: rule.judges(),
            agent: agent_name,
            files: target,
            rationale: rule.wants_rationale(session_rationales),
        });
    }

    // Rules whose rationale is disabled: llmlint is authoritative, so we drop any
    // rationale a harness returns anyway, keeping `--no-rationales` deterministic
    // regardless of harness behavior.
    let rationale_off: HashSet<String> = resolved
        .iter()
        .filter(|r| !r.rationale)
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

    // Bind Copy references so the per-judge `move` closures capture borrows
    // (which outlive the scope) rather than moving the owned `client`/`cwd`.
    let client_ref = &client;
    let cwd_ref = cwd.as_path();
    for wave in the_plan.runs.chunks(max_parallel) {
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
                        )
                    })
                })
                .collect();
            for (run, handle) in wave.iter().zip(handles) {
                let (trace, result) = handle.join().expect("judge thread panicked");
                if let Some(trace) = trace {
                    traces.push((judge_label(run), trace));
                }
                match result {
                    Ok(map) => {
                        for (name, verdict) in map {
                            verdicts.entry(name).or_default().push(verdict);
                        }
                    }
                    Err(e) => run_errors.push(format!("{}: {}", judge_label(run), e)),
                }
            }
        });
    }

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

    let report = Report::new(outcomes, run_errors);
    emit(&report, args.format, args.verbose, args.color);
    Ok(report.exit_code())
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

fn resolve_files(
    cwd: &Path,
    rule: &Rule,
    agent: &Agent,
    cli_files: &[PathBuf],
    global: &FileFilter,
) -> Result<Vec<PathBuf>> {
    if let Some(f) = &rule.files {
        return files::resolve(cwd, f);
    }
    if let Some(f) = &agent.files {
        return files::resolve(cwd, f);
    }
    if !cli_files.is_empty() {
        return Ok(cli_files.to_vec());
    }
    files::resolve(cwd, global)
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

/// Render a (relative) path with forward slashes, so the prompt the judge sees —
/// and the violation paths it echoes back — are consistent across platforms
/// (Windows `PathBuf` would otherwise render `\`).
fn to_slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
) -> (
    Option<oneharness::RunTrace>,
    Result<BTreeMap<String, RuleVerdict>>,
) {
    let files_str: Vec<String> = run.files.iter().map(|p| to_slash(p)).collect();
    // Show the rationale guidance when any rule in this batch wants a rationale.
    let want_rationale = run.rules.iter().any(|r| r.rationale);
    let system = match template::render(&run.template, &run.rules, &files_str, want_rationale) {
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
        })
        .collect();
    let schema = schema::build(&specs);
    let req = oneharness::RunRequest {
        harness: run.harness.as_deref(),
        model: run.model.as_deref().or(global_model),
        system: &system,
        prompt: PROMPT_TRIGGER,
        schema: &schema,
        schema_max_retries: run.schema_max_retries,
        cwd,
        timeout_secs: timeout,
        oneharness_config: oh_config,
        no_config: false,
    };
    if want_trace {
        let (trace, result) = client.run_with_trace(&req);
        (Some(trace), result)
    } else {
        (None, client.run(&req))
    }
}

fn emit(report: &Report, format: OutputFormat, verbosity: u8, color: ColorChoice) {
    match format {
        OutputFormat::Human => {
            // Resolve `--color` against the live stdout: `auto` colors only an
            // interactive terminal with `NO_COLOR` unset. The decision is made
            // here (the I/O boundary) and handed to the pure formatter as a bool.
            let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
            let on = color.resolve(std::io::stdout().is_terminal(), no_color);
            print!("{}", report.to_human(verbosity, on));
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

    fn rule(name: &str, agent: Option<&str>) -> Rule {
        Rule {
            name: name.into(),
            description: "true when ok; false otherwise.".into(),
            agent: agent.map(Into::into),
            judges: None,
            files: None,
            rationale: None,
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
