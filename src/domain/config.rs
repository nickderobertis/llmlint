//! The llmlint configuration model, plus deterministic validation.
//!
//! Deserialized from YAML (anchors/aliases and `<<` merge keys are resolved by
//! the YAML layer in [`crate::io::configfs`], so prompt text can be shared
//! across agents with no framework support). Structural checks that *can* be
//! deterministic (unique, valid, resolvable names) are enforced here; judgment
//! about a rule's quality is left to the bundled config-lint plugin.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::version::Version;
use crate::errors::{Error, Result};

/// Include/exclude glob set used to select target files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileFilter {
    /// Globs selecting files to lint.
    #[serde(default)]
    pub include: Vec<String>,
    /// Globs subtracted from the included set.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl FileFilter {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// Passthrough/defaults for how llmlint invokes `oneharness`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OneharnessCfg {
    /// oneharness config file(s) to forward via `--config` (single-file today;
    /// extras are warned and dropped).
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
    #[schemars(range(min = 1))]
    pub timeout: Option<u64>,
    /// Schema-validation re-prompt budget passed to oneharness `--schema-max-retries`.
    #[serde(default)]
    pub schema_max_retries: Option<u32>,
}

impl OneharnessCfg {
    /// Fill any unset field from `other` (a plugin's `oneharness` block), keeping
    /// this (nearer-root) config's own values. Lets a plugin supply defaults the
    /// including config didn't set, while the including config always wins.
    pub fn merge_under(&mut self, other: OneharnessCfg) {
        if self.config.is_empty() {
            self.config = other.config;
        }
        self.bin = self.bin.take().or(other.bin);
        self.model = self.model.take().or(other.model);
        self.timeout = self.timeout.or(other.timeout);
        self.schema_max_retries = self.schema_max_retries.or(other.schema_max_retries);
    }
}

/// A group of rules sharing reviewer context and harness/model/batch config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Harness id from `oneharness list`. When unset, llmlint omits `--harness`
    /// and oneharness selects its own configured default harness.
    #[serde(default)]
    pub harness: Option<String>,
    /// Model override for this agent's judges.
    #[serde(default)]
    pub model: Option<String>,
    /// Max rules per judge run (default 20).
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub batch_size: Option<usize>,
    /// Extra prompt text appended to the master template before rendering.
    #[serde(default)]
    pub prompt_template: Option<String>,
    /// Override the target files for this agent's rules.
    #[serde(default)]
    pub files: Option<FileFilter>,
}

/// A single lint rule: a statement judged true/false about the target files.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Terse snake_case identifier: an ASCII letter followed by letters, digits,
    /// or underscores. Used as a JSON Schema key for the judge's verdict.
    #[schemars(regex(pattern = r"^[A-Za-z][A-Za-z0-9_]*$"))]
    pub name: String,
    /// The invariant the judge evaluates. State clearly what is TRUE (passes)
    /// and what is FALSE (a violation).
    #[schemars(length(min = 1))]
    pub description: String,
    /// Name of the agent (under `agents`) this rule runs on. Defaults to the
    /// `default` agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// Independent judges to run; the majority verdict wins (default 1). Must be
    /// odd so the vote can't tie.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub judges: Option<u32>,
    /// Override the target files for this rule.
    #[serde(default)]
    pub files: Option<FileFilter>,
    /// Whether the judge must justify this rule's verdict with a `rationale`.
    /// Overrides the session-wide `rationales` default for this one rule; unset
    /// inherits it.
    #[serde(default)]
    pub rationale: Option<bool>,
}

impl Rule {
    pub fn judges(&self) -> u32 {
        self.judges.unwrap_or(1)
    }

    /// Whether this rule requires a rationale, given the session-wide default
    /// (from config `rationales` or the `--rationales`/`--no-rationales` flag).
    pub fn wants_rationale(&self, session_default: bool) -> bool {
        self.rationale.unwrap_or(session_default)
    }
}

/// A whole llmlint config (one file, before include-merging) or the merged
/// result. Unknown *top-level* keys are allowed so anchors can be stashed in a
/// throwaway key (e.g. `x-prompts:`); nested structs reject unknown fields.
///
/// This type is the single source of the published config JSON Schema: the
/// `JsonSchema` derive (post-processed in [`crate::domain::config_schema`])
/// generates `assets/llmlint.schema.json`, so the schema can never drift from
/// the model. Field doc comments become the schema's property descriptions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[schemars(
    title = "llmlint configuration",
    description = "Configuration for llmlint, an LLM-as-judge linter for code-quality checks \
                   deterministic linters can't express. Docs: https://github.com/nickderobertis/llmlint"
)]
pub struct Config {
    /// The config's published version (`1`, `1.1`, or `1.1.1`). Set this when
    /// the config is consumed as a plugin: a consumer pins a desired version
    /// with an `@` suffix on the plugin URL, validated against this value.
    #[serde(default)]
    pub version: Option<Version>,
    /// Master minijinja prompt template, rendered with `rules` (each with name +
    /// description) and `files` (the target paths). Overrides the built-in one.
    #[serde(default)]
    pub prompt_template: Option<String>,
    /// Default include/exclude globs selecting target files when none are passed
    /// on the CLI.
    #[serde(default)]
    pub files: FileFilter,
    /// Defaults for how llmlint invokes the oneharness subprocess.
    #[serde(default)]
    pub oneharness: OneharnessCfg,
    /// Whether judges must justify each verdict with a short `rationale`
    /// (default `true`). Rationales aid auditability, debugging, and reliability
    /// (the judge reasons before concluding) but cost extra output tokens on
    /// every request. A per-rule `rationale` overrides this default. The
    /// `--rationales`/`--no-rationales` CLI flags override the config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationales: Option<bool>,
    /// Plugins (shared rule sets) merged in, one entry each: a local file path
    /// or a URL (`http(s)://`, `file://`), the URL optionally pinned with an
    /// `@version` suffix. Named `plugins` (not `include`) to avoid confusion
    /// with `files.include`. Resolution lives in [`crate::io::plugins`].
    #[serde(default)]
    pub plugins: Vec<String>,
    /// Named agents that group rules and share harness/model/batch config. A
    /// rule with no `agent` uses the `default` agent.
    #[serde(default)]
    pub agents: BTreeMap<String, Agent>,
    /// The lint rules. Each is a positive invariant judged true/false about the
    /// target files (holds=true passes; holds=false is a violation).
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Config {
    /// Fold a plugin (an included config) into this one. `self` is nearer the
    /// root of the include graph, so **`self` always wins**: every top-level
    /// setting it already specifies is kept, and the plugin only fills in the
    /// gaps. Resolution is a pre-order walk (root, then its plugins, then their
    /// plugins), so this first-writer-wins rule gives the documented precedence —
    /// the current config over its plugins, a plugin over its own plugins, and an
    /// earlier-listed plugin over a later sibling. Rules are appended in include
    /// order; on an agent-name clash the existing (nearer-root) agent is kept.
    pub fn merge_plugin(&mut self, other: Config) {
        // Top-level scalars: keep ours when set, otherwise adopt the plugin's.
        self.version = self.version.take().or(other.version);
        self.prompt_template = self.prompt_template.take().or(other.prompt_template);
        if self.files.is_empty() {
            self.files = other.files;
        }
        self.oneharness.merge_under(other.oneharness);
        self.rationales = self.rationales.or(other.rationales);
        for (name, agent) in other.agents {
            self.agents.entry(name).or_insert(agent);
        }
        self.rules.extend(other.rules);
    }

    /// The agent named `name`, or a default agent when it is not declared.
    pub fn agent_or_default(&self, name: &str) -> Agent {
        self.agents.get(name).cloned().unwrap_or_default()
    }

    /// The session-wide rationale default after merging: the config's
    /// `rationales` value, or `true` when unset.
    pub fn rationales_default(&self) -> bool {
        self.rationales.unwrap_or(true)
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
        } else if let Some(judges) = rule.judges {
            if judges % 2 == 0 {
                problems.push(format!(
                    "rule {:?} has judges: {} (must be odd so the majority verdict can't tie)",
                    rule.name, judges
                ));
            }
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
            rationale: None,
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
        root.merge_plugin(other);
        assert_eq!(root.rules.len(), 2);
        // Root's agent definition wins on clash.
        assert_eq!(
            root.agents["shared"].harness.as_deref(),
            Some("claude-code")
        );
    }

    #[test]
    fn merge_top_level_scalars_keep_root_then_fall_back_to_plugin() {
        // Root sets some scalars; the plugin sets others (and clashes on one).
        let mut root = Config {
            prompt_template: Some("root template".into()),
            rationales: Some(false),
            ..Default::default()
        };
        root.oneharness.model = Some("opus".into());
        let plugin = Config {
            // Clashes: root must win.
            prompt_template: Some("plugin template".into()),
            rationales: Some(true),
            // Gaps the root left open: the plugin fills them.
            files: FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            },
            oneharness: OneharnessCfg {
                model: Some("haiku".into()),
                timeout: Some(99),
                ..Default::default()
            },
            ..Default::default()
        };
        root.merge_plugin(plugin);
        // Root wins every clash...
        assert_eq!(root.prompt_template.as_deref(), Some("root template"));
        assert_eq!(root.rationales, Some(false));
        assert_eq!(root.oneharness.model.as_deref(), Some("opus"));
        // ...and the plugin fills only what the root left unset.
        assert_eq!(root.files.include, vec!["src/**".to_string()]);
        assert_eq!(root.oneharness.timeout, Some(99));
    }

    #[test]
    fn rationales_default_is_true_when_unset() {
        assert!(Config::default().rationales_default());
        let off = Config {
            rationales: Some(false),
            ..Default::default()
        };
        assert!(!off.rationales_default());
    }

    #[test]
    fn per_rule_rationale_overrides_session_default() {
        let r = rule("r");
        assert!(r.wants_rationale(true));
        assert!(!r.wants_rationale(false));
        let forced_on = Rule {
            rationale: Some(true),
            ..rule("on")
        };
        let forced_off = Rule {
            rationale: Some(false),
            ..rule("off")
        };
        assert!(forced_on.wants_rationale(false));
        assert!(!forced_off.wants_rationale(true));
    }

    #[test]
    fn even_judges_is_invalid() {
        let c = Config {
            rules: vec![Rule {
                judges: Some(2),
                ..rule("even_judges")
            }],
            ..Default::default()
        };
        let err = validate(&c).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must be odd"), "got: {msg}");
    }

    #[test]
    fn odd_judges_is_valid() {
        let c = Config {
            rules: vec![Rule {
                judges: Some(3),
                ..rule("odd_judges")
            }],
            ..Default::default()
        };
        assert!(validate(&c).is_ok());
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
