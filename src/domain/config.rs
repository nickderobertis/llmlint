//! The llmlint configuration model, plus deterministic validation.
//!
//! Deserialized from YAML (anchors/aliases and `<<` merge keys are resolved by
//! the YAML layer in [`crate::io::configfs`], so prompt text can be shared
//! across agents with no framework support). Structural checks that *can* be
//! deterministic (unique, valid, resolvable names) are enforced here; judgment
//! about a rule's quality is left to the bundled config-lint plugin.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// Include/exclude glob set used to select target files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl FileFilter {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// Passthrough/defaults for how llmlint invokes `oneharness`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OneharnessCfg {
    /// oneharness config file(s) to forward via `--config` (single-file today).
    #[serde(default)]
    pub config: Vec<String>,
    /// Override the oneharness binary path.
    #[serde(default)]
    pub bin: Option<String>,
    /// Default model for every judge (an agent's `model` overrides it).
    #[serde(default)]
    pub model: Option<String>,
    /// Per-judge timeout in seconds (default 120).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Schema-validation re-prompt budget passed to oneharness `--schema-max-retries`.
    #[serde(default)]
    pub schema_max_retries: Option<u32>,
}

/// A group of rules sharing reviewer context and harness/model/batch config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Harness id from `oneharness list` (default `claude-code`).
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Max rules per judge run (default 20).
    #[serde(default)]
    pub batch_size: Option<usize>,
    /// Extra prompt text appended to the master template before rendering.
    #[serde(default)]
    pub prompt_template: Option<String>,
    /// Override the target files for this agent's rules.
    #[serde(default)]
    pub files: Option<FileFilter>,
}

/// A single lint rule: a statement judged true/false about the target files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub agent: Option<String>,
    /// Independent judges to run; the majority verdict wins (default 1).
    #[serde(default)]
    pub judges: Option<u32>,
    /// Override the target files for this rule.
    #[serde(default)]
    pub files: Option<FileFilter>,
}

impl Rule {
    pub fn judges(&self) -> u32 {
        self.judges.unwrap_or(1)
    }
}

/// A whole llmlint config (one file, before include-merging) or the merged
/// result. Unknown *top-level* keys are allowed so anchors can be stashed in a
/// throwaway key (e.g. `x-prompts:`); nested structs reject unknown fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub files: FileFilter,
    #[serde(default)]
    pub oneharness: OneharnessCfg,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub agents: BTreeMap<String, Agent>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Config {
    /// Merge another config's rules and agents into this one (used to fold in
    /// `include`d configs). Top-level scalars (template, files, oneharness) are
    /// the entry config's and are left untouched; on an agent-name clash the
    /// existing (earlier/root) definition wins.
    pub fn merge_rules_and_agents(&mut self, other: Config) {
        for (name, agent) in other.agents {
            self.agents.entry(name).or_insert(agent);
        }
        self.rules.extend(other.rules);
    }

    /// The agent named `name`, or a default agent when it is not declared.
    pub fn agent_or_default(&self, name: &str) -> Agent {
        self.agents.get(name).cloned().unwrap_or_default()
    }
}

/// Whether `name` is a valid, terse rule identifier: an ASCII letter followed
/// by letters, digits, or underscores. Keeps names safe as JSON Schema keys and
/// nudges toward descriptive snake_case. (Placeholder/nonsense names are a
/// judgment call left to the config-lint plugin.)
pub fn is_valid_rule_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Deterministic validation. Collects *all* problems so one run surfaces every
/// fix, rather than failing on the first.
pub fn validate(config: &Config) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();

    for rule in &config.rules {
        if !is_valid_rule_name(&rule.name) {
            problems.push(format!(
                "rule name {:?} is not a valid identifier (letters, digits, underscore; \
                 must start with a letter)",
                rule.name
            ));
        }
        if !seen.insert(rule.name.as_str()) {
            problems.push(format!("duplicate rule name {:?}", rule.name));
        }
        if rule.description.trim().is_empty() {
            problems.push(format!("rule {:?} has an empty description", rule.name));
        }
        if rule.judges == Some(0) {
            problems.push(format!("rule {:?} has judges: 0 (must be >= 1)", rule.name));
        }
        if let Some(agent) = &rule.agent {
            if !config.agents.contains_key(agent) {
                problems.push(format!(
                    "rule {:?} references unknown agent {:?}",
                    rule.name, agent
                ));
            }
        }
    }

    for (name, agent) in &config.agents {
        if agent.batch_size == Some(0) {
            problems.push(format!("agent {:?} has batch_size: 0 (must be >= 1)", name));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidConfig(problems.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str) -> Rule {
        Rule {
            name: name.into(),
            description: "TRUE when ok; FALSE otherwise.".into(),
            agent: None,
            judges: None,
            files: None,
        }
    }

    #[test]
    fn valid_config_passes() {
        let c = Config {
            rules: vec![rule("alpha_rule"), rule("beta_rule")],
            ..Default::default()
        };
        assert!(validate(&c).is_ok());
    }

    #[test]
    fn name_validation_rules() {
        assert!(is_valid_rule_name("good_name1"));
        assert!(!is_valid_rule_name("1leading_digit"));
        assert!(!is_valid_rule_name("has-dash"));
        assert!(!is_valid_rule_name(""));
        assert!(!is_valid_rule_name("with space"));
    }

    #[test]
    fn collects_duplicate_invalid_and_unknown_agent() {
        let mut bad = rule("dup");
        bad.agent = Some("missing".into());
        let c = Config {
            rules: vec![
                rule("dup"),
                bad,
                rule("bad-name"),
                Rule {
                    judges: Some(0),
                    ..rule("zero_judges")
                },
            ],
            ..Default::default()
        };
        let err = validate(&c).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("duplicate rule name"));
        assert!(msg.contains("unknown agent"));
        assert!(msg.contains("not a valid identifier"));
        assert!(msg.contains("judges: 0"));
    }

    #[test]
    fn empty_description_is_invalid() {
        let c = Config {
            rules: vec![Rule {
                description: "   ".into(),
                ..rule("empty_desc")
            }],
            ..Default::default()
        };
        assert!(validate(&c).is_err());
    }

    #[test]
    fn merge_keeps_root_agent_and_appends_rules() {
        let mut root = Config {
            rules: vec![rule("root_rule")],
            ..Default::default()
        };
        root.agents.insert(
            "shared".into(),
            Agent {
                harness: Some("claude-code".into()),
                ..Default::default()
            },
        );
        let mut other = Config {
            rules: vec![rule("plugin_rule")],
            ..Default::default()
        };
        other.agents.insert(
            "shared".into(),
            Agent {
                harness: Some("codex".into()),
                ..Default::default()
            },
        );
        root.merge_rules_and_agents(other);
        assert_eq!(root.rules.len(), 2);
        // Root's agent definition wins on clash.
        assert_eq!(
            root.agents["shared"].harness.as_deref(),
            Some("claude-code")
        );
    }

    #[test]
    fn agent_batch_size_zero_is_invalid() {
        let mut c = Config::default();
        c.agents.insert(
            "a".into(),
            Agent {
                batch_size: Some(0),
                ..Default::default()
            },
        );
        assert!(validate(&c).is_err());
    }
}
