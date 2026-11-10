//! `llmlint rules (--files <PATH>... | --diff [<BASE>])`: print the rules that
//! would judge a set of paths, so an agent can read the bar its work will meet
//! before writing anything.
//!
//! Selection is `lint`'s own, step for step, never a re-implementation: the same
//! config load (nested discovery relevance-gated by the named paths, plugins,
//! `--config`), the same `LLMLINT_*` env layer and `--exclude`, the same per-rule
//! [`ignores::resolve_files`] that intersects named files with each rule's scope
//! as path strings (so a path that does not exist yet matches exactly as it would
//! once written), `relevance: false` rules left out as `lint` never judges them,
//! and a whole-file `ignore-file` honored for a path that exists. `--diff` takes
//! the targets `lint --diff` keeps — the glob walk narrowed by the same
//! `lint::target_diffs` and `lint::is_changed` — so a deleted path adds no
//! rule. No harness is resolved or spawned and no model is called.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::cli::{RulesArgs, RulesFormat};
use crate::commands::{ignores, lint};
use crate::domain::config::{validate, Config, RelevanceMode, Rule};
use crate::domain::ignore;
use crate::domain::rule_listing::{AgentName, RuleEntry, RuleName, RulesDocument};
use crate::errors::{Error, Result};
use crate::io::configfs::{self, RuleScope};
use crate::io::diff::DiffBackend;
use crate::io::plugins::PluginResolution;
use crate::io::{env, files};

pub fn run(args: RulesArgs) -> Result<i32> {
    let cwd = lint::resolve_cwd(&args.cwd)?;
    let named = files::from_cli(&cwd, &args.files);
    // Named paths relevance-gate the subtree cascade exactly as `lint FILES` does;
    // `--diff` names none, so every subtree config loads, as for `lint --diff`.
    let loaded = configfs::load_with_targets(&args.config, &cwd, &named)?;
    let mut scopes = loaded.scopes;
    let mut config = loaded.config;
    let plugins = loaded.plugins;
    validate(&config)?;
    // The same layering `lint` applies: env over config, then the CLI flags this
    // command shares with it (`--exclude`, and the `--diff` base over
    // `diff_base`), with a changed session `files` filter re-pointed into the
    // cwd-rooted rule scopes.
    let pre_files = config.files.clone();
    env::apply_overrides(&mut config)?;
    if let Some(Some(base)) = &args.diff {
        config.diff_base = Some(base.clone());
    }
    config.files.exclude.extend(args.exclude.iter().cloned());
    if config.files != pre_files {
        ignores::retarget_session_scopes(&mut scopes, &cwd, &config.files);
    }

    let mut matched: Vec<(&Rule, Vec<PathBuf>)> = Vec::new();
    for rule in &config.rules {
        // A rule switched off never reaches a judge, so it never judges a path.
        if matches!(rule.relevance_mode(), RelevanceMode::Never) {
            continue;
        }
        let paths = ignores::resolve_files(
            &cwd,
            rule,
            &named,
            &scope_of(&scopes, rule, &cwd, &config),
            &config.files.exclude,
        )?;
        matched.push((rule, paths));
    }

    let asked: Vec<PathBuf> = match &args.diff {
        None => named,
        Some(_) => {
            let all: Vec<PathBuf> = matched
                .iter()
                .flat_map(|(_, paths)| paths.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let selected: Vec<&Rule> = config.rules.iter().collect();
            let diffs =
                lint::target_diffs(DiffBackend::Git, &config, &cwd, &selected, &scopes, &all)?;
            for (_, paths) in &mut matched {
                paths.retain(|p| lint::is_changed(&diffs, &cwd, p));
            }
            matched
                .iter()
                .flat_map(|(_, paths)| paths.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        }
    };

    drop_file_ignored(&cwd, &config, &plugins, &mut matched)?;

    let rules: Vec<RuleEntry> = matched
        .into_iter()
        .filter(|(_, paths)| !paths.is_empty())
        .map(|(rule, paths)| RuleEntry {
            name: RuleName(rule.name.clone()),
            description: rule.description.clone(),
            agent: AgentName(rule.agent.clone().unwrap_or_else(|| "default".to_string())),
            paths: paths.iter().map(|p| files::to_slash(p)).collect(),
        })
        .collect();
    let doc = RulesDocument::new(
        asked.iter().map(|p| files::to_slash(p)).collect(),
        rules,
        |agent| {
            config
                .agent_or_default(&agent.0)
                .prompt_template
                .unwrap_or_default()
        },
    );

    let format = args.format.unwrap_or(if std::io::stdout().is_terminal() {
        RulesFormat::Markdown
    } else {
        RulesFormat::Json
    });
    match format {
        RulesFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| Error::Io(e.to_string()))?
        ),
        RulesFormat::Markdown => print!("{}", doc.to_markdown()),
    }
    Ok(0)
}

/// The rule's directory scope from the load, or the cwd-rooted session filter for
/// a rule the load recorded none for — the same fallback `lint` uses.
fn scope_of(
    scopes: &BTreeMap<String, RuleScope>,
    rule: &Rule,
    cwd: &Path,
    config: &Config,
) -> RuleScope {
    scopes
        .get(&rule.name)
        .cloned()
        .unwrap_or_else(|| RuleScope {
            dir: cwd.to_path_buf(),
            files: config.files.clone(),
        })
}

/// Drop each existing path a rule's whole-file `ignore-file` directive exempts,
/// as `lint`'s planner drops it before any judge sees it. A path that does not
/// exist carries no directive, so it is never dropped here.
///
/// The directives are first held to `lint`'s own structural pre-flight
/// ([`ignores::check`]): a malformed one (no reason, an unknown rule, an
/// unbalanced block) is the same exit-2 error `lint` raises, never a silent
/// exemption — the parser alone would still honor a reason-less `ignore-file`.
fn drop_file_ignored(
    cwd: &Path,
    config: &Config,
    plugins: &[PluginResolution],
    matched: &mut [(&Rule, Vec<PathBuf>)],
) -> Result<()> {
    let known = ignores::known_rules(config);
    let existing: BTreeSet<PathBuf> = matched
        .iter()
        .flat_map(|(_, paths)| paths.iter())
        .filter(|p| cwd.join(p).is_file())
        .cloned()
        .collect();
    ignores::check(cwd, &existing, &known, plugins)?;
    let mut exempt: BTreeMap<PathBuf, ignore::Suppressions> = BTreeMap::new();
    for path in existing {
        if let Some(text) = files::read_text(cwd, &path)? {
            let s = ignore::suppressions(&text, &known);
            if !s.is_empty() {
                exempt.insert(path, s);
            }
        }
    }
    for (rule, paths) in matched.iter_mut() {
        paths.retain(|p| !exempt.get(p).is_some_and(|s| s.is_file_scoped(&rule.name)));
    }
    Ok(())
}
