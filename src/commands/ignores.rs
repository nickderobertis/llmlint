//! Shared inline-`llmlint: ignore` directive validation: resolve which target
//! files to scan and check the *structure* of any directives they carry.
//!
//! Two commands lean on this: the `lint` pre-flight (so a typo'd ignore fails
//! before any judge call) and the standalone `check-ignores` command (so the
//! same check can run in the fast, deterministic linter loop without touching a
//! model or oneharness). Routing both through one module keeps the two from ever
//! disagreeing about what a well-formed directive is.
//!
//! Honoring a well-formed directive is **not** done here — that is the judge's
//! job, specified in the default prompt template. This module only enforces that
//! each directive names specific, configured rule(s) and a reason; see
//! [`crate::domain::ignore`] for the pure parser.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::domain::config::{Agent, Config, FileFilter, RelevanceMode, Rule};
use crate::domain::ignore;
use crate::errors::{Error, Result};
use crate::io::files;

/// The configured rule names — the set a directive may legitimately reference.
/// A directive may name any configured rule, not just the ones a given run
/// selects, so this is always the full config.
pub fn known_rules(config: &Config) -> BTreeSet<&str> {
    config.rules.iter().map(|r| r.name.as_str()).collect()
}

/// Resolve the union of every evaluated rule's target files (relative to `cwd`),
/// de-duplicated and ordered. This mirrors what `lint` would scan: rules
/// disabled with `relevance: false` never run, so their files are not scanned
/// here either. `cli_files`, when non-empty, overrides the config globs exactly
/// as it does for a lint run (per-rule / per-agent `files` still win).
pub fn target_files(
    cwd: &Path,
    config: &Config,
    cli_files: &[PathBuf],
) -> Result<BTreeSet<PathBuf>> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    for rule in &config.rules {
        if matches!(rule.relevance_mode(), RelevanceMode::Never) {
            continue;
        }
        let agent_name = rule.agent.clone().unwrap_or_else(|| "default".to_string());
        let agent = config.agent_or_default(&agent_name);
        for f in resolve_files(cwd, rule, &agent, cli_files, &config.files)? {
            out.insert(f);
        }
    }
    Ok(out)
}

/// The target files for a single rule, applying the same precedence a lint run
/// uses: a per-rule `files` filter wins, then the agent's, then explicit CLI
/// files, then the global filter.
pub fn resolve_files(
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

/// Scan each file (read once, relative to `cwd`) for inline `llmlint: ignore`
/// directives and reject any whose structure is malformed — no rule named, an
/// unknown/invalid rule, a missing reason, or unbalanced block pairing.
/// Non-UTF-8 (binary) files can't carry a text directive and are skipped. Every
/// problem across every file is collected into one [`Error::IgnoreDirective`]
/// (exit 2) so a single run surfaces all the fixes; an empty file set is clean.
pub fn check(cwd: &Path, targets: &BTreeSet<PathBuf>, known: &BTreeSet<&str>) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    for rel in targets {
        let Some(text) = files::read_text(cwd, rel)? else {
            continue;
        };
        for p in ignore::validate(&text, known) {
            problems.push(format!(
                "  {}:{}: {}",
                files::to_slash(rel),
                p.line,
                p.message
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::IgnoreDirective(problems.join("\n")))
    }
}
