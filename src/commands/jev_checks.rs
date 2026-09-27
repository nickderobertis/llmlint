//! `llmlint jev-checks`: draft `jev.checks` for rules — one harness call that
//! compiles each rule's description into atomic violation checks — and print
//! them as YAML to review and paste. It never edits a config: the checks are
//! part of the rule, so they are reviewed and committed like the rule itself.

use crate::cli::JevChecksArgs;
use crate::domain::config::validate;
use crate::domain::jevchecks::{self, RuleBrief};
use crate::errors::{Error, Result};
use crate::io::{configfs, env, oneharness};

pub fn run(args: JevChecksArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };
    let loaded = configfs::load(&args.config, &cwd)?;
    let mut config = loaded.config;
    env::apply_overrides(&mut config)?;
    validate(&config)?;

    let unknown: Vec<&str> = args
        .rule
        .iter()
        .map(String::as_str)
        .filter(|n| !config.rules.iter().any(|r| r.name == *n))
        .collect();
    if !unknown.is_empty() {
        let mut available: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
        available.sort_unstable();
        return Err(Error::UnknownFilter(format!(
            "no rule named {}; available rules: {}",
            unknown.join(", "),
            available.join(", ")
        )));
    }
    // Named rules, else every rule that has no checks yet.
    let rules: Vec<_> = config
        .rules
        .iter()
        .filter(|r| {
            if args.rule.is_empty() {
                r.jev_spec().checks.is_empty()
            } else {
                args.rule.contains(&r.name)
            }
        })
        .collect();
    if rules.is_empty() {
        eprintln!("every rule already has `jev.checks` (name one with --rule to redo it)");
        return Ok(0);
    }

    let briefs: Vec<RuleBrief> = rules
        .iter()
        .map(|r| {
            let spec = r.jev_spec();
            RuleBrief {
                name: r.name.clone(),
                description: r.description.clone(),
                relevance: match r.relevance_mode() {
                    crate::domain::config::RelevanceMode::Conditional(c) => Some(c),
                    _ => None,
                },
                unit: spec.unit,
                pattern: spec.pattern,
            }
        })
        .collect();
    let names: Vec<&str> = briefs.iter().map(|b| b.name.as_str()).collect();

    // The default agent's harness and model, unless overridden.
    let agent = config.agent_or_default("default");
    let harness = args.harness.clone().or_else(|| agent.harness.clone());
    let model = args
        .model
        .clone()
        .or_else(|| agent.model.clone())
        .or_else(|| config.oneharness.model.clone());
    let bin = args
        .oneharness_bin
        .clone()
        .or_else(|| config.oneharness.bin.clone());
    let client = oneharness::Client::new(bin.as_deref());
    client.check_min_version()?;
    eprintln!(
        "drafting checks for {} rule(s) via {} (this makes a real model call)…",
        briefs.len(),
        harness.as_deref().unwrap_or("the default harness")
    );
    let prompt = jevchecks::prompt(&briefs);
    let schema = jevchecks::schema(&names);
    let structured = client.run_structured(&oneharness::RunRequest {
        harness: harness.as_deref(),
        model: model.as_deref(),
        system: jevchecks::SYSTEM,
        prompt: &prompt,
        schema: &schema,
        schema_max_retries: config.oneharness.schema_max_retries,
        cwd: &cwd,
        timeout_secs: args.timeout,
        oneharness_config: None,
        no_config: false,
    })?;
    let checks = jevchecks::parse(&structured, &names).map_err(Error::Oneharness)?;
    let rendered: Vec<String> = names
        .iter()
        .map(|n| jevchecks::render(n, &checks[*n]))
        .collect();
    print!("{}", rendered.join("\n"));
    Ok(0)
}
