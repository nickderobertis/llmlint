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
    /// Per-judge timeout in seconds (default 600).
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

/// Whether, how many, and where to log each run's full results to disk. When
/// logging is on (the default) every `lint`/`lint-config` run is written as one
/// JSON record so callers can retrieve the complete results later — including
/// the per-rule detail the terminal report omits — via `llmlint history <id>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryCfg {
    /// Whether to log each run's results (default `true`). Set `false` to turn
    /// the feature off entirely; nothing is written and no run id is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How many of the most recent runs to keep (default 100). After each run the
    /// oldest records beyond this count are pruned. Must be >= 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_runs: Option<usize>,
    /// Directory the JSON records are written to. Defaults to the platform
    /// per-user data directory for llmlint (e.g. `~/.local/share/llmlint/history`
    /// on Linux, `%LOCALAPPDATA%\llmlint\data\history` on Windows). The
    /// `LLMLINT_HISTORY_DIR` environment variable overrides this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

impl HistoryCfg {
    /// Whether every field is unset — used both as the merge "is unset" test and
    /// by provenance to decide whether this config contributed a `history` block.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.max_runs.is_none() && self.dir.is_none()
    }

    /// Fill any unset field from `other` (a plugin's or more-distant config's
    /// `history` block), keeping this (nearer-root) config's own values. Mirrors
    /// [`OneharnessCfg::merge_under`].
    pub fn merge_under(&mut self, other: HistoryCfg) {
        self.enabled = self.enabled.or(other.enabled);
        self.max_runs = self.max_runs.or(other.max_runs);
        self.dir = self.dir.take().or(other.dir);
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
}

/// When a rule should be evaluated. Mirrors the `description`/verdict split: a
/// boolean is resolved deterministically by llmlint, a string is a
/// natural-language condition the judge decides about the change first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum Relevance {
    /// `true` (the default): always evaluate — the judge may not opt out.
    /// `false`: never evaluate — the rule is statically not applicable and is
    /// reported as not relevant without calling a judge.
    Always(bool),
    /// A natural-language condition describing when the rule applies. The judge
    /// decides whether it holds for the change *before* evaluating the verdict,
    /// and reports the rule "not relevant" (with no verdict) when it does not.
    When(String),
}

/// How a rule's relevance resolves once the default is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelevanceMode {
    /// Always evaluate; the judge does not get to opt out.
    Always,
    /// Never evaluate; statically not applicable (reported not relevant with no
    /// judge call).
    Never,
    /// The judge first decides whether this condition holds for the change.
    Conditional(String),
}

/// `skip_serializing_if` predicate for the rarely-set `override` flag, so a
/// serialized config (e.g. `llmlint config`) stays clean.
fn is_false(b: &bool) -> bool {
    !*b
}

/// A single lint rule: a statement judged true/false about the target files.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Terse snake_case identifier: an ASCII letter followed by letters, digits,
    /// or underscores. Used as a JSON Schema key for the judge's verdict.
    #[schemars(regex(pattern = r"^[A-Za-z][A-Za-z0-9_]*$"))]
    pub name: String,
    /// The invariant the judge evaluates. State clearly what is true (passes)
    /// and what is false (a violation). Required for a normal rule (an empty one
    /// is rejected); an `override` rule may omit it to inherit the base rule's
    /// text, so the schema leaves it optional.
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub description: String,
    /// Override a same-named rule contributed by a plugin: inherit all of the
    /// base rule's fields, replacing only the ones set here. Without this, a
    /// duplicate rule name is an error. Set it on the consuming (nearer-root)
    /// config; the override is resolved into the base when the config loads.
    #[serde(default, rename = "override", skip_serializing_if = "is_false")]
    pub r#override: bool,
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
    /// When this rule should be evaluated. `true` (the default) always
    /// evaluates; `false` never does (the rule is reported not relevant without
    /// a judge call); a string is a condition the judge decides about the change
    /// first, reporting the rule "not relevant" when it does not hold. Lets a
    /// rule scope itself to applicable changes instead of every `description`
    /// needing its own "or not applicable" escape hatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance: Option<Relevance>,
    /// Whether every violation of this rule must cite a concrete `file` and
    /// `line`. Off by default — some findings (e.g. a cross-cutting
    /// architectural drift) genuinely can't be pinned to one source line, so a
    /// violation may omit its location. Set `true` for a rule whose violations
    /// must always be localizable: the generated schema then marks each
    /// violation's `file`/`line` **required**, so oneharness re-prompts the judge
    /// to localize *every* violation in one batched turn (no per-violation back
    /// and forth), and the default template asks for it up front. A violation
    /// that still arrives without a file+line is a hard error rather than a
    /// silently-imprecise report. Inherited/overridable like the other per-rule
    /// fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_line_attribution: Option<bool>,
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

    /// Whether every violation of this rule must carry a concrete file + line
    /// (the `require_line_attribution` flag; default `false`).
    pub fn requires_line_attribution(&self) -> bool {
        self.require_line_attribution.unwrap_or(false)
    }

    /// How this rule's relevance resolves, applying the default (always evaluate
    /// when `relevance` is unset or `true`).
    pub fn relevance_mode(&self) -> RelevanceMode {
        match &self.relevance {
            None | Some(Relevance::Always(true)) => RelevanceMode::Always,
            Some(Relevance::Always(false)) => RelevanceMode::Never,
            Some(Relevance::When(cond)) => RelevanceMode::Conditional(cond.clone()),
        }
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
    /// Default base the `--diff` git backend compares target files against when
    /// `--diff-base` is not passed. Any git revision — a branch, tag, commit, or
    /// `A..B`/`A...B` range — e.g. `main` to make a quality gate review whatever
    /// the current branch changed versus the default branch. A plain ref uses
    /// three-dot / merge-base semantics (like a PR's "Files changed"), so a
    /// branch behind its base doesn't see base-branch drift as its own changes;
    /// an `A..B` range is forwarded to git as-is. Unset keeps the built-in `HEAD`
    /// (working-tree) base; the `--diff-base` flag overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_base: Option<String>,
    /// Whether/how/where to log each run's full results to disk (default: on, the
    /// last 100 runs, in the platform data dir). See [`HistoryCfg`].
    #[serde(default, skip_serializing_if = "HistoryCfg::is_empty")]
    pub history: HistoryCfg,
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
        self.diff_base = self.diff_base.take().or(other.diff_base);
        self.history.merge_under(other.history);
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

    /// Whether results logging is on after merging: the config's `history.enabled`
    /// value, or `true` when unset (the feature is on by default).
    pub fn history_enabled(&self) -> bool {
        self.history.enabled.unwrap_or(true)
    }

    /// How many recent runs to keep after merging: the config's `history.max_runs`
    /// value, or `100` when unset.
    pub fn history_max_runs(&self) -> usize {
        self.history.max_runs.unwrap_or(100)
    }
}

/// Where each item in the merged config came from — built as configs are folded
/// together during load (see [`crate::io::configfs::load`]), so a rule, agent,
/// or setting in the merged result can be traced back to the file (or plugin
/// URL) that contributed it. The source strings are exactly the keys in
/// [`crate::io::configfs::Loaded::sources`]. Build it with a
/// [`ProvenanceBuilder`].
///
/// Provenance mirrors the merge precedence ([`Config::merge_plugin`]): each
/// top-level setting and each agent records the **first** (nearest-root) source
/// that set it — the one that wins the merge. A rule is reported at field
/// granularity ([`RuleProvenance`]) because an `override` resolves field by
/// field, so a single resolved rule can draw different fields from different
/// files.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Provenance {
    /// Source of each top-level setting that was set, by field name (`version`,
    /// `prompt_template`, `files`, `rationales`, and the `oneharness.*`
    /// sub-fields). Only fields actually set by some config appear.
    pub settings: BTreeMap<String, String>,
    /// Source of each agent, by name. An agent is kept whole from its first
    /// (nearest-root) writer on a name clash, so one source is exact.
    pub agents: BTreeMap<String, String>,
    /// Per-rule provenance, by name: where the rule is defined and which fields,
    /// if any, an `override` pulled from a different file.
    pub rules: BTreeMap<String, RuleProvenance>,
}

/// Top-level setting keys that carry provenance, mirroring the config/JSON
/// shape (the `oneharness.*` block is flattened). The single source of truth for
/// which paths [`resolve_source`] treats as a setting, and kept in sync with the
/// keys [`ProvenanceBuilder::record`] emits.
pub const SETTING_KEYS: &[&str] = &[
    "version",
    "prompt_template",
    "files.include",
    "files.exclude",
    "oneharness.config",
    "oneharness.bin",
    "oneharness.model",
    "oneharness.timeout",
    "oneharness.schema_max_retries",
    "rationales",
    "diff_base",
    "history.enabled",
    "history.max_runs",
    "history.dir",
];

/// The per-rule fields a `rules.<name>.<field>` query can name. `name` always
/// resolves to the definition site; the rest may be set by an `override`.
const RULE_FIELDS: &[&str] = &[
    "name",
    "description",
    "agent",
    "judges",
    "files",
    "rationale",
    "relevance",
    "require_line_attribution",
];

/// Where a resolved rule, and each of its fields, comes from. A rule with no
/// `override` has every field at its single definition site, so `fields` is
/// empty; an `override` that changes a field from another file surfaces that
/// field here, pointing at the file to edit for it.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct RuleProvenance {
    /// The rule's definition site: the source of its base (non-`override`)
    /// declaration, the default place to edit it.
    pub source: String,
    /// Fields whose resolved value came from a **different** source than
    /// `source` because an `override` layer set them — field name -> the source
    /// to edit for that field. Empty when the rule resolves entirely from its
    /// definition site (the common case), so it is omitted from the output then.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

/// Accumulates provenance as configs are folded in during load, then resolves it
/// to a [`Provenance`]. Kept separate from `Provenance` because rule field
/// provenance can only be computed once every `override` occurrence has been
/// collected, the same two-phase shape as merge-then-[`resolve_overrides`].
#[derive(Debug, Default)]
pub struct ProvenanceBuilder {
    settings: BTreeMap<String, String>,
    agents: BTreeMap<String, String>,
    /// Every occurrence of each rule name, in load order (nearest-root first):
    /// its source paired with the raw (pre-resolution) rule.
    rule_occurrences: BTreeMap<String, Vec<(String, Rule)>>,
}

impl ProvenanceBuilder {
    /// Record `cfg`'s contributions as coming from `origin`. Call once per config
    /// as it is folded in, in merge order, so first-writer-wins settings/agents
    /// line up with the value that survives the merge, and rule occurrences are
    /// collected nearest-root first (see [`Config::merge_plugin`]).
    pub fn record(&mut self, cfg: &Config, origin: &str) {
        self.record_settings(cfg, origin);
        self.record_items(cfg, origin);
    }

    /// Record only `cfg`'s agents and rules, not its top-level settings. Used for a
    /// **cascaded subtree config** ([`crate::io::configfs`]): its settings never
    /// retune the session (only `cwd`-and-up configs do), so they must not appear
    /// as a setting's source — but its agents and rules are still contributed.
    pub fn record_items(&mut self, cfg: &Config, origin: &str) {
        for name in cfg.agents.keys() {
            self.agents
                .entry(name.clone())
                .or_insert_with(|| origin.to_string());
        }
        for rule in &cfg.rules {
            self.rule_occurrences
                .entry(rule.name.clone())
                .or_default()
                .push((origin.to_string(), rule.clone()));
        }
    }

    /// Record only `cfg`'s top-level settings (the first writer of each wins).
    fn record_settings(&mut self, cfg: &Config, origin: &str) {
        // Each top-level setting, paired with whether this config sets it. The
        // predicates match the merge's "is unset" tests, so the recorded source
        // is the one whose value wins.
        let settings: &[(&str, bool)] = &[
            ("version", cfg.version.is_some()),
            ("prompt_template", cfg.prompt_template.is_some()),
            // `files` is reported at sub-field granularity (like `oneharness.*` /
            // `history.*`), so an env override of one list traces precisely and a
            // `where files.exclude` query resolves.
            ("files.include", !cfg.files.include.is_empty()),
            ("files.exclude", !cfg.files.exclude.is_empty()),
            ("oneharness.config", !cfg.oneharness.config.is_empty()),
            ("oneharness.bin", cfg.oneharness.bin.is_some()),
            ("oneharness.model", cfg.oneharness.model.is_some()),
            ("oneharness.timeout", cfg.oneharness.timeout.is_some()),
            (
                "oneharness.schema_max_retries",
                cfg.oneharness.schema_max_retries.is_some(),
            ),
            ("rationales", cfg.rationales.is_some()),
            ("diff_base", cfg.diff_base.is_some()),
            ("history.enabled", cfg.history.enabled.is_some()),
            ("history.max_runs", cfg.history.max_runs.is_some()),
            ("history.dir", cfg.history.dir.is_some()),
        ];
        for (key, present) in settings {
            if *present {
                self.settings
                    .entry((*key).to_string())
                    .or_insert_with(|| origin.to_string());
            }
        }
    }

    /// Resolve the collected occurrences into per-rule provenance. Call after
    /// [`resolve_overrides`] has validated the rules (so every name has exactly
    /// one base); a malformed group still yields a safe entry rather than
    /// panicking, since the caller surfaces the real error and discards this.
    pub fn finish(self) -> Provenance {
        let mut rules: BTreeMap<String, RuleProvenance> = BTreeMap::new();
        for (name, occ) in self.rule_occurrences {
            rules.insert(name, rule_provenance(&occ));
        }
        Provenance {
            settings: self.settings,
            agents: self.agents,
            rules,
        }
    }
}

/// Compute one rule's field provenance from its occurrences (source + raw rule),
/// in load order. Uses the shared [`field_winners`] precedence so it can't drift
/// from how [`resolve_overrides`] actually resolves the rule.
fn rule_provenance(occ: &[(String, Rule)]) -> RuleProvenance {
    let bases: Vec<&(String, Rule)> = occ.iter().filter(|(_, r)| !r.r#override).collect();
    // Exactly one base is the validated case; on anything else fall back to the
    // first occurrence's source (the caller will have already errored out).
    let base = match bases.as_slice() {
        [b] => *b,
        _ => {
            return RuleProvenance {
                source: occ.first().map(|(s, _)| s.clone()).unwrap_or_default(),
                fields: BTreeMap::new(),
            }
        }
    };
    let base_src = base.0.as_str();
    let overrides: Vec<&Rule> = occ
        .iter()
        .filter(|(_, r)| r.r#override)
        .map(|(_, r)| r)
        .collect();
    let override_srcs: Vec<&str> = occ
        .iter()
        .filter(|(_, r)| r.r#override)
        .map(|(s, _)| s.as_str())
        .collect();

    let w = field_winners(&overrides);
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    // For each resolved field, the source is the winning override's (when one set
    // it) or the base's. Record only the fields whose source differs from the
    // definition site — those are the ones an override pulled from elsewhere.
    let mut note = |field: &str, present: bool, winner: Option<usize>| {
        if !present {
            return;
        }
        let src = winner.map(|i| override_srcs[i]).unwrap_or(base_src);
        if src != base_src {
            fields.insert(field.to_string(), src.to_string());
        }
    };
    note("description", true, w.description);
    note(
        "agent",
        base.1.agent.is_some() || w.agent.is_some(),
        w.agent,
    );
    note(
        "judges",
        base.1.judges.is_some() || w.judges.is_some(),
        w.judges,
    );
    note(
        "files",
        base.1.files.is_some() || w.files.is_some(),
        w.files,
    );
    note(
        "rationale",
        base.1.rationale.is_some() || w.rationale.is_some(),
        w.rationale,
    );
    note(
        "relevance",
        base.1.relevance.is_some() || w.relevance.is_some(),
        w.relevance,
    );
    note(
        "require_line_attribution",
        base.1.require_line_attribution.is_some() || w.require_line_attribution.is_some(),
        w.require_line_attribution,
    );
    RuleProvenance {
        source: base_src.to_string(),
        fields,
    }
}

/// Resolve a dotted config path to the source (file path or plugin URL) that
/// contributes it — the place to edit it — for the `where` command. The path
/// mirrors the config/JSON structure:
/// - `agents.<name>` -> the agent's source;
/// - `rules.<name>` -> the rule's definition site;
/// - `rules.<name>.<field>` -> the file to edit that field: the `override` that
///   set it, or the definition site when no override did;
/// - anything else -> a top-level setting key ([`SETTING_KEYS`], e.g. `version`,
///   `oneharness.model`).
///
/// `Ok` is the source string; `Err` is an actionable message (an unknown name
/// lists what is available; a real setting left at its built-in default says so;
/// an unrecognized path shows the accepted forms).
pub fn resolve_source(prov: &Provenance, path: &str) -> std::result::Result<String, String> {
    if let Some(name) = path.strip_prefix("agents.") {
        return prov.agents.get(name).cloned().ok_or_else(|| {
            format!(
                "no agent named {name:?} in the merged config (agents: {})",
                list_or_none(prov.agents.keys())
            )
        });
    }
    if let Some(rest) = path.strip_prefix("rules.") {
        // Rule names are validated identifiers (no dots), so a single trailing
        // dotted segment is unambiguously a field selector.
        let mut parts = rest.splitn(2, '.');
        let name = parts.next().unwrap_or("");
        let field = parts.next();
        let rule = prov.rules.get(name).ok_or_else(|| {
            format!(
                "no rule named {name:?} in the merged config (rules: {})",
                list_or_none(prov.rules.keys())
            )
        })?;
        return match field {
            None => Ok(rule.source.clone()),
            Some(f) if RULE_FIELDS.contains(&f) => {
                // A field an override set lives elsewhere; otherwise it resolves
                // from (and is edited at) the rule's definition site.
                Ok(rule
                    .fields
                    .get(f)
                    .cloned()
                    .unwrap_or_else(|| rule.source.clone()))
            }
            Some(f) => Err(format!(
                "unknown rule field {f:?}; valid fields: {}",
                RULE_FIELDS.join(", ")
            )),
        };
    }
    if SETTING_KEYS.contains(&path) {
        return prov.settings.get(path).cloned().ok_or_else(|| {
            format!(
                "`{path}` is not set by any config; the built-in default applies (nothing to edit)"
            )
        });
    }
    Err(format!(
        "unknown config path {path:?}; expected a setting (e.g. `oneharness.model`, `version`), \
         `agents.<name>`, `rules.<name>`, or `rules.<name>.<field>`"
    ))
}

/// Join keys for an error message, or `none` when empty.
fn list_or_none<'a>(keys: impl Iterator<Item = &'a String>) -> String {
    let mut names: Vec<&str> = keys.map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// Layer `override` rules onto the base rule they extend, in place. For each
/// rule name, the single rule declared without `override` is the *base*; rules
/// marked `override` inherit every field they leave unset from it, replacing
/// only the ones they set. The resolved rule keeps the position of the first
/// occurrence (nearest the include root).
///
/// Precedence follows the merge order ([`Config::merge_plugin`] appends rules
/// nearer the root first), so when more than one override targets the same base
/// the nearest-root override wins each field. Errors are collected, not
/// fail-fast: an `override` with no base to extend, and a duplicate name where
/// no occurrence opts into `override`, are each reported (the latter is the
/// "duplicate rule name" error, surfaced here so the message can point at the
/// fix).
pub fn resolve_overrides(config: &mut Config) -> Result<()> {
    // Group the rule indices by name, remembering first-seen order so the output
    // is stable and a resolved rule lands where its name first appeared.
    let mut order: Vec<&str> = Vec::new();
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, r) in config.rules.iter().enumerate() {
        let group = groups.entry(r.name.as_str()).or_default();
        if group.is_empty() {
            order.push(r.name.as_str());
        }
        group.push(i);
    }

    let mut problems: Vec<String> = Vec::new();
    let mut resolved: Vec<Rule> = Vec::new();
    for name in &order {
        let idxs = &groups[name];
        let bases: Vec<&Rule> = idxs
            .iter()
            .map(|&i| &config.rules[i])
            .filter(|r| !r.r#override)
            .collect();
        let overrides: Vec<&Rule> = idxs
            .iter()
            .map(|&i| &config.rules[i])
            .filter(|r| r.r#override)
            .collect();

        match bases.as_slice() {
            [] => problems.push(format!(
                "rule {name:?} is marked `override` but no base rule with that name is defined \
                 (a plugin must declare it first)"
            )),
            [base] if overrides.is_empty() => resolved.push((*base).clone()),
            [base] => resolved.push(merge_override(base, &overrides)),
            _ => problems.push(format!(
                "duplicate rule name {name:?} (set `override: true` to extend a plugin's rule \
                 instead of redefining it)"
            )),
        }
    }

    if problems.is_empty() {
        config.rules = resolved;
        Ok(())
    } else {
        Err(Error::InvalidConfig(problems.join("; ")))
    }
}

/// Which layer supplies each overridable field of a resolved rule: `Some(i)` =
/// `overrides[i]`, `None` = the base. `overrides` are ordered nearest-root
/// first, and the nearest-root layer that sets a field wins (`description`
/// ignores blank overrides; the optionals win on `Some`). This is the single
/// definition of override precedence, shared by [`merge_override`] (which builds
/// the resolved rule) and [`rule_provenance`] (which traces each field's source)
/// so the two can't drift.
struct FieldWinners {
    description: Option<usize>,
    agent: Option<usize>,
    judges: Option<usize>,
    files: Option<usize>,
    rationale: Option<usize>,
    relevance: Option<usize>,
    require_line_attribution: Option<usize>,
}

fn field_winners(overrides: &[&Rule]) -> FieldWinners {
    FieldWinners {
        description: overrides
            .iter()
            .position(|ov| !ov.description.trim().is_empty()),
        agent: overrides.iter().position(|ov| ov.agent.is_some()),
        judges: overrides.iter().position(|ov| ov.judges.is_some()),
        files: overrides.iter().position(|ov| ov.files.is_some()),
        rationale: overrides.iter().position(|ov| ov.rationale.is_some()),
        relevance: overrides.iter().position(|ov| ov.relevance.is_some()),
        require_line_attribution: overrides
            .iter()
            .position(|ov| ov.require_line_attribution.is_some()),
    }
}

/// Fold `overrides` (ordered nearest-root first) onto a clone of `base`: the
/// nearest-root override wins each field it sets ([`field_winners`]); an unset
/// field (`None`, or an empty `description`) leaves the base's value.
fn merge_override(base: &Rule, overrides: &[&Rule]) -> Rule {
    let w = field_winners(overrides);
    let mut out = base.clone();
    out.r#override = false;
    if let Some(i) = w.description {
        out.description = overrides[i].description.clone();
    }
    if let Some(i) = w.agent {
        out.agent = overrides[i].agent.clone();
    }
    if let Some(i) = w.judges {
        out.judges = overrides[i].judges;
    }
    if let Some(i) = w.files {
        out.files = overrides[i].files.clone();
    }
    if let Some(i) = w.rationale {
        out.rationale = overrides[i].rationale;
    }
    if let Some(i) = w.relevance {
        out.relevance = overrides[i].relevance.clone();
    }
    if let Some(i) = w.require_line_attribution {
        out.require_line_attribution = overrides[i].require_line_attribution;
    }
    out
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
        if let Some(Relevance::When(cond)) = &rule.relevance {
            if cond.trim().is_empty() {
                problems.push(format!(
                    "rule {:?} has an empty relevance condition (use `true`/`false` for an \
                     always/never rule, or a non-empty condition)",
                    rule.name
                ));
            }
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

    if config.history.max_runs == Some(0) {
        problems.push(
            "history.max_runs is 0 (must be >= 1; set history.enabled: false to turn logging off)"
                .to_string(),
        );
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
            description: "true when ok; false otherwise.".into(),
            r#override: false,
            agent: None,
            judges: None,
            files: None,
            rationale: None,
            relevance: None,
            require_line_attribution: None,
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
            diff_base: Some("main".into()),
            ..Default::default()
        };
        root.oneharness.model = Some("opus".into());
        let plugin = Config {
            // Clashes: root must win.
            prompt_template: Some("plugin template".into()),
            rationales: Some(true),
            diff_base: Some("develop".into()),
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
        assert_eq!(root.diff_base.as_deref(), Some("main"));
        assert_eq!(root.oneharness.model.as_deref(), Some("opus"));
        // ...and the plugin fills only what the root left unset.
        assert_eq!(root.files.include, vec!["src/**".to_string()]);
        assert_eq!(root.oneharness.timeout, Some(99));
    }

    #[test]
    fn diff_base_falls_back_to_plugin_when_root_unset() {
        // The root leaves `diff_base` unset, so a plugin supplies the default.
        let mut root = Config::default();
        let plugin = Config {
            diff_base: Some("main".into()),
            ..Default::default()
        };
        root.merge_plugin(plugin);
        assert_eq!(root.diff_base.as_deref(), Some("main"));
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
    fn require_line_attribution_defaults_off_and_reads_the_flag() {
        assert!(!rule("r").requires_line_attribution());
        let on = Rule {
            require_line_attribution: Some(true),
            ..rule("on")
        };
        let off = Rule {
            require_line_attribution: Some(false),
            ..rule("off")
        };
        assert!(on.requires_line_attribution());
        assert!(!off.requires_line_attribution());
    }

    #[test]
    fn override_resolves_require_line_attribution_like_the_other_fields() {
        // The base (as if from a plugin) leaves it unset; the override (nearer the
        // root, listed first) turns it on, and the resolved rule inherits that.
        let base = rule("attr");
        let over = Rule {
            name: "attr".into(),
            r#override: true,
            require_line_attribution: Some(true),
            ..rule("attr")
        };
        let mut c = Config {
            rules: vec![over, base],
            ..Default::default()
        };
        resolve_overrides(&mut c).unwrap();
        assert_eq!(c.rules.len(), 1);
        assert!(c.rules[0].requires_line_attribution());

        // Field provenance traces the override's contribution to its file.
        let mut b = ProvenanceBuilder::default();
        let near = Config {
            rules: vec![Rule {
                name: "attr".into(),
                r#override: true,
                require_line_attribution: Some(true),
                ..rule("attr")
            }],
            ..Default::default()
        };
        let far = Config {
            rules: vec![rule("attr")],
            ..Default::default()
        };
        b.record(&near, "near.yml");
        b.record(&far, "far.yml");
        let prov = b.finish();
        assert_eq!(prov.rules["attr"].source, "far.yml");
        assert_eq!(
            prov.rules["attr"].fields["require_line_attribution"],
            "near.yml"
        );
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
    fn relevance_mode_resolves_the_default_and_the_three_forms() {
        // Unset and `true` both mean always-evaluate (the judge can't opt out).
        assert_eq!(rule("r").relevance_mode(), RelevanceMode::Always);
        let always = Rule {
            relevance: Some(Relevance::Always(true)),
            ..rule("r")
        };
        assert_eq!(always.relevance_mode(), RelevanceMode::Always);
        let never = Rule {
            relevance: Some(Relevance::Always(false)),
            ..rule("r")
        };
        assert_eq!(never.relevance_mode(), RelevanceMode::Never);
        let when = Rule {
            relevance: Some(Relevance::When("the change touches SQL".into())),
            ..rule("r")
        };
        assert_eq!(
            when.relevance_mode(),
            RelevanceMode::Conditional("the change touches SQL".into())
        );
    }

    #[test]
    fn empty_relevance_condition_is_invalid() {
        let c = Config {
            rules: vec![Rule {
                relevance: Some(Relevance::When("   ".into())),
                ..rule("blank_relevance")
            }],
            ..Default::default()
        };
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("empty relevance condition"));
    }

    #[test]
    fn override_inherits_unset_fields_and_replaces_set_ones() {
        // Base (as if from a plugin) carries the full text + judges; the override
        // (nearer the root, so listed first) bumps judges and adds an agent,
        // leaving description to inherit.
        let base = Rule {
            judges: Some(1),
            ..rule("style")
        };
        let over = Rule {
            name: "style".into(),
            description: String::new(), // omitted -> inherit
            r#override: true,
            agent: Some("strict".into()),
            judges: Some(3),
            ..rule("style")
        };
        let mut c = Config {
            rules: vec![over, base],
            ..Default::default()
        };
        resolve_overrides(&mut c).unwrap();
        assert_eq!(c.rules.len(), 1);
        let r = &c.rules[0];
        assert_eq!(r.name, "style");
        assert_eq!(r.description, "true when ok; false otherwise."); // inherited
        assert_eq!(r.judges, Some(3)); // replaced
        assert_eq!(r.agent.as_deref(), Some("strict")); // added
        assert!(!r.r#override); // flag cleared on the resolved rule
    }

    #[test]
    fn override_can_replace_the_description() {
        let base = rule("r");
        let over = Rule {
            name: "r".into(),
            description: "a sharper invariant.".into(),
            r#override: true,
            ..rule("r")
        };
        let mut c = Config {
            rules: vec![over, base],
            ..Default::default()
        };
        resolve_overrides(&mut c).unwrap();
        assert_eq!(c.rules[0].description, "a sharper invariant.");
    }

    #[test]
    fn override_without_a_base_is_an_error() {
        let mut c = Config {
            rules: vec![Rule {
                r#override: true,
                ..rule("orphan")
            }],
            ..Default::default()
        };
        let err = resolve_overrides(&mut c).unwrap_err();
        assert!(err.to_string().contains("no base rule"), "{err}");
    }

    #[test]
    fn duplicate_name_without_override_is_an_error() {
        let mut c = Config {
            rules: vec![rule("dup"), rule("dup")],
            ..Default::default()
        };
        let err = resolve_overrides(&mut c).unwrap_err();
        assert!(err.to_string().contains("duplicate rule name"), "{err}");
    }

    #[test]
    fn nearest_root_override_wins_each_field() {
        // Two overrides target one base; the first-listed (nearest root) wins.
        let near = Rule {
            name: "r".into(),
            r#override: true,
            judges: Some(5),
            ..rule("r")
        };
        let far = Rule {
            name: "r".into(),
            r#override: true,
            judges: Some(3),
            agent: Some("far".into()),
            ..rule("r")
        };
        let base = rule("r");
        let mut c = Config {
            rules: vec![near, far, base],
            ..Default::default()
        };
        resolve_overrides(&mut c).unwrap();
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].judges, Some(5)); // nearest wins
        assert_eq!(c.rules[0].agent.as_deref(), Some("far")); // only the far set it
    }

    #[test]
    fn override_keeps_first_occurrence_position() {
        let mut c = Config {
            rules: vec![
                rule("a"),
                Rule {
                    r#override: true,
                    judges: Some(3),
                    ..rule("b")
                },
                rule("c"),
                rule("b"), // the base, declared later (as if by a plugin)
            ],
            ..Default::default()
        };
        resolve_overrides(&mut c).unwrap();
        let names: Vec<&str> = c.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(c.rules[1].judges, Some(3));
    }

    #[test]
    fn provenance_traces_settings_agents_and_each_rule_field() {
        let mut b = ProvenanceBuilder::default();

        // Nearest-root file: top-level settings, a plain rule `a`, and an
        // `override` of `shared` that bumps only `judges`.
        let mut near = Config {
            version: Some(Version::parse("1").unwrap()),
            rationales: Some(false),
            files: FileFilter {
                include: vec!["src/**".into()],
                exclude: vec![],
            },
            rules: vec![
                rule("a"),
                Rule {
                    name: "shared".into(),
                    description: String::new(), // omitted -> inherit the base's
                    r#override: true,
                    judges: Some(3),
                    ..rule("shared")
                },
            ],
            ..Default::default()
        };
        near.oneharness.model = Some("opus".into());
        near.oneharness.config = vec!["oh.yml".into()];
        near.agents.insert("g".into(), Agent::default());

        // Farther file: a rule `b`, the *base* `shared`, and settings that either
        // clash with `near` (near wins) or fill gaps it left open.
        let mut far = Config {
            version: Some(Version::parse("2").unwrap()), // clash -> near wins
            prompt_template: Some("t".into()),           // gap -> far is the source
            rules: vec![rule("b"), rule("shared")],
            ..Default::default()
        };
        far.oneharness.model = Some("haiku".into()); // clash -> near
        far.oneharness.timeout = Some(9); // only far set it
        far.oneharness.schema_max_retries = Some(2);
        far.agents.insert("g".into(), Agent::default());

        b.record(&near, "near.yml");
        b.record(&far, "far.yml");
        let prov = b.finish();

        // Settings + agents: first (nearest-root) writer wins.
        assert_eq!(prov.settings["version"], "near.yml");
        assert_eq!(prov.settings["rationales"], "near.yml");
        assert_eq!(prov.settings["files.include"], "near.yml");
        // Only `include` was set on `near`; `exclude` was empty, so it is absent.
        assert!(!prov.settings.contains_key("files.exclude"));
        assert_eq!(prov.settings["oneharness.model"], "near.yml");
        assert_eq!(prov.settings["oneharness.config"], "near.yml");
        assert_eq!(prov.settings["prompt_template"], "far.yml");
        assert_eq!(prov.settings["oneharness.timeout"], "far.yml");
        assert_eq!(prov.settings["oneharness.schema_max_retries"], "far.yml");
        assert_eq!(prov.agents["g"], "near.yml");
        // A setting nobody set is absent (no `oneharness.bin` was given).
        assert!(!prov.settings.contains_key("oneharness.bin"));

        // Plain rules: a single definition site, no per-field divergence.
        assert_eq!(prov.rules["a"].source, "near.yml");
        assert!(prov.rules["a"].fields.is_empty());
        assert_eq!(prov.rules["b"].source, "far.yml");
        assert!(prov.rules["b"].fields.is_empty());

        // `shared` is defined in `far`, but its `judges` came from the `near`
        // override -> the field is traced to `near` while the rule (and its
        // inherited description) stays `far`.
        let shared = &prov.rules["shared"];
        assert_eq!(shared.source, "far.yml");
        assert_eq!(shared.fields["judges"], "near.yml");
        assert!(!shared.fields.contains_key("description"));
    }

    #[test]
    fn setting_keys_match_what_record_emits() {
        // A config that sets every setting must produce exactly `SETTING_KEYS`,
        // so the `where` resolver's notion of valid settings can't drift from
        // what the builder records.
        let mut b = ProvenanceBuilder::default();
        let mut cfg = Config {
            version: Some(Version::parse("1").unwrap()),
            prompt_template: Some("t".into()),
            rationales: Some(true),
            diff_base: Some("main".into()),
            history: HistoryCfg {
                enabled: Some(true),
                max_runs: Some(50),
                dir: Some("h".into()),
            },
            files: FileFilter {
                include: vec!["x".into()],
                exclude: vec!["y".into()],
            },
            ..Default::default()
        };
        cfg.oneharness = OneharnessCfg {
            config: vec!["c".into()],
            bin: Some("b".into()),
            model: Some("m".into()),
            timeout: Some(1),
            schema_max_retries: Some(1),
        };
        b.record(&cfg, "f.yml");
        let prov = b.finish();
        let got: BTreeSet<&str> = prov.settings.keys().map(String::as_str).collect();
        let want: BTreeSet<&str> = SETTING_KEYS.iter().copied().collect();
        assert_eq!(got, want);
    }

    #[test]
    fn resolve_source_walks_settings_agents_and_rule_fields() {
        let mut prov = Provenance::default();
        prov.settings
            .insert("oneharness.model".into(), "team.yml".into());
        prov.agents.insert("security".into(), "team.yml".into());
        prov.rules.insert(
            "secrets".into(),
            RuleProvenance {
                source: "team.yml".into(),
                fields: BTreeMap::from([("judges".to_string(), "root.yml".to_string())]),
            },
        );

        // Settings, agents, a rule's definition site.
        assert_eq!(
            resolve_source(&prov, "oneharness.model").unwrap(),
            "team.yml"
        );
        assert_eq!(
            resolve_source(&prov, "agents.security").unwrap(),
            "team.yml"
        );
        assert_eq!(resolve_source(&prov, "rules.secrets").unwrap(), "team.yml");
        // An overridden field resolves to the override's file; a field nobody
        // overrode resolves to the definition site.
        assert_eq!(
            resolve_source(&prov, "rules.secrets.judges").unwrap(),
            "root.yml"
        );
        assert_eq!(
            resolve_source(&prov, "rules.secrets.description").unwrap(),
            "team.yml"
        );
    }

    #[test]
    fn resolve_source_errors_are_actionable() {
        let mut prov = Provenance::default();
        prov.agents.insert("a".into(), "f.yml".into());
        prov.rules.insert(
            "r".into(),
            RuleProvenance {
                source: "f.yml".into(),
                ..Default::default()
            },
        );

        // A real setting nobody set -> distinct "default applies" message.
        let unset = resolve_source(&prov, "oneharness.bin").unwrap_err();
        assert!(unset.contains("built-in default applies"), "{unset}");
        // Unknown names list what's available.
        assert!(resolve_source(&prov, "agents.missing")
            .unwrap_err()
            .contains("agents: a"));
        assert!(resolve_source(&prov, "rules.missing")
            .unwrap_err()
            .contains("rules: r"));
        // Unknown rule field lists the valid ones.
        assert!(resolve_source(&prov, "rules.r.bogus")
            .unwrap_err()
            .contains("valid fields"));
        // An unrecognized path shows the accepted forms.
        assert!(resolve_source(&prov, "nonsense")
            .unwrap_err()
            .contains("expected a setting"));
    }

    #[test]
    fn history_defaults_on_with_hundred_runs() {
        // Unset -> logging on, keep the last 100.
        let c = Config::default();
        assert!(c.history_enabled());
        assert_eq!(c.history_max_runs(), 100);
        // Explicit values win.
        let c = Config {
            history: HistoryCfg {
                enabled: Some(false),
                max_runs: Some(5),
                dir: Some("/tmp/h".into()),
            },
            ..Default::default()
        };
        assert!(!c.history_enabled());
        assert_eq!(c.history_max_runs(), 5);
    }

    #[test]
    fn history_merges_under_a_plugin_root_first() {
        // Root sets `enabled`; the plugin fills the gaps it left (max_runs, dir).
        let mut root = Config {
            history: HistoryCfg {
                enabled: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let plugin = Config {
            history: HistoryCfg {
                enabled: Some(true), // clash -> root wins
                max_runs: Some(7),
                dir: Some("/plugin/h".into()),
            },
            ..Default::default()
        };
        root.merge_plugin(plugin);
        assert_eq!(root.history.enabled, Some(false));
        assert_eq!(root.history.max_runs, Some(7));
        assert_eq!(root.history.dir.as_deref(), Some("/plugin/h"));
    }

    #[test]
    fn history_max_runs_zero_is_invalid() {
        let c = Config {
            history: HistoryCfg {
                max_runs: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("history.max_runs is 0"));
    }

    #[test]
    fn history_provenance_traces_each_subfield() {
        // The root sets `enabled`; a plugin fills `max_runs`/`dir` -> each
        // sub-field traces to its first (nearest-root) writer.
        let mut b = ProvenanceBuilder::default();
        let root = Config {
            history: HistoryCfg {
                enabled: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let plugin = Config {
            history: HistoryCfg {
                enabled: Some(false),
                max_runs: Some(9),
                dir: Some("/h".into()),
            },
            ..Default::default()
        };
        b.record(&root, "root.yml");
        b.record(&plugin, "plugin.yml");
        let prov = b.finish();
        assert_eq!(prov.settings["history.enabled"], "root.yml");
        assert_eq!(prov.settings["history.max_runs"], "plugin.yml");
        assert_eq!(prov.settings["history.dir"], "plugin.yml");
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
