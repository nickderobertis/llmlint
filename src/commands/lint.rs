//! `llmlint lint` (the default): load config, resolve files, plan judge runs,
//! drive oneharness in parallel, aggregate votes, and report.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

use crate::cli::{LintArgs, OutputFormat};
use crate::domain::config::{validate, Agent, Config, FileFilter, Rule};
use crate::domain::plan::{self, JudgeRun};
use crate::domain::report::Report;
use crate::domain::template::{self};
use crate::domain::verdict::{RuleOutcome, RuleVerdict};
use crate::domain::{schema, vote};
use crate::errors::{Error, Result};
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
    let config = loaded.config;
    validate(&config)?;

    let selected = select_rules(&config, &args);
    if selected.is_empty() {
        let report = Report::new(Vec::new(), Vec::new());
        emit(&report, args.format, args.verbose);
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
        });
    }

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
                        )
                    })
                })
                .collect();
            for (run, handle) in wave.iter().zip(handles) {
                match handle.join().expect("judge thread panicked") {
                    Ok(map) => {
                        for (name, verdict) in map {
                            verdicts.entry(name).or_default().push(verdict);
                        }
                    }
                    Err(e) => run_errors.push(format!(
                        "agent {} judge {} [{}]: {}",
                        run.agent,
                        run.judge_index,
                        run.rules
                            .iter()
                            .map(|r| r.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        e
                    )),
                }
            }
        });
    }

    let mut outcomes: Vec<RuleOutcome> = verdicts
        .iter()
        .map(|(name, vs)| vote::tally(name, vs))
        .collect();
    for name in &the_plan.skipped {
        outcomes.push(RuleOutcome::skipped(name));
    }

    let report = Report::new(outcomes, run_errors);
    emit(&report, args.format, args.verbose);
    Ok(report.exit_code())
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

fn execute(
    client: &oneharness::Client,
    run: &JudgeRun,
    cwd: &Path,
    timeout: u64,
    oh_config: Option<&Path>,
    global_model: Option<&str>,
) -> Result<BTreeMap<String, RuleVerdict>> {
    let files_str: Vec<String> = run.files.iter().map(|p| to_slash(p)).collect();
    let system = template::render(&run.template, &run.rules, &files_str)?;
    let names: Vec<&str> = run.rules.iter().map(|r| r.name.as_str()).collect();
    let schema = schema::build(&names);
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
    client.run(&req)
}

fn emit(report: &Report, format: OutputFormat, verbosity: u8) {
    match format {
        OutputFormat::Human => print!("{}", report.to_human(verbosity)),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
            )
        }
    }
}
