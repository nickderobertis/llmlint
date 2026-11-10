//! The document `llmlint rules` prints: which rules would judge a set of paths,
//! each as a minimal definition (name, description, judging agent), plus each of
//! those agents once with its framing.
//!
//! [`RulesDocument`] is the one declaration of that JSON shape — the command
//! serializes it, [`schema`] derives its JSON Schema from it, and the committed
//! golden `tests/fixtures/rules_document/schema.json` pins that schema
//! (`committed_schema_matches_the_declaration`; regenerate with
//! `LLMLINT_UPDATE_SCHEMA=1 cargo test -p llmlint rule_listing`). Change the shape
//! and bump [`SCHEMA_VERSION`] and the golden in the same commit: hosts parse it.
//!
//! The Markdown rendering lives here too, so a host can put the same set into a
//! task with no rendering code of its own. Pure: no I/O.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the [`RulesDocument`] shape. Bump it with any change to the
/// shape (and move the committed schema golden in the same commit).
pub const SCHEMA_VERSION: u32 = 1;

/// The rules that would judge a set of paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesDocument {
    /// The version of this document's shape.
    pub schema_version: u32,
    /// The paths asked about (relative to the working directory, forward
    /// slashes): the named paths, or under `--diff` the changed paths a
    /// `lint --diff` would judge.
    pub paths: Vec<String>,
    /// Each agent that judges at least one listed rule, once, in the order its
    /// first rule appears.
    pub agents: Vec<AgentEntry>,
    /// The rules whose scoping matches at least one path, in config order.
    pub rules: Vec<RuleEntry>,
}

/// An agent that judges at least one listed rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentEntry {
    /// The agent's name (`default` for rules that name none).
    pub name: String,
    /// The reviewer context the agent's prompt gives it (its `prompt_template`),
    /// empty when it sets none.
    pub framing: String,
}

/// A rule that would judge at least one of the paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleEntry {
    /// The rule's name.
    pub name: String,
    /// The invariant the judge evaluates.
    pub description: String,
    /// The agent that judges it (an entry in `agents`).
    pub agent: String,
    /// The paths this rule's scoping matches, a subset of the document's `paths`.
    pub paths: Vec<String>,
}

/// The JSON Schema (2020-12) of [`RulesDocument`], derived from the type.
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(RulesDocument)).expect("rules schema serializes")
}

impl RulesDocument {
    /// Build the document from the matched rules (in config order) and a lookup
    /// of each agent's framing. Agents are listed once each, in the order their
    /// first rule appears, and only when a listed rule names them.
    pub fn new(
        paths: Vec<String>,
        rules: Vec<RuleEntry>,
        framing: impl Fn(&str) -> String,
    ) -> Self {
        let mut agents: Vec<AgentEntry> = Vec::new();
        for rule in &rules {
            if !agents.iter().any(|a| a.name == rule.agent) {
                agents.push(AgentEntry {
                    name: rule.agent.clone(),
                    framing: framing(&rule.agent),
                });
            }
        }
        RulesDocument {
            schema_version: SCHEMA_VERSION,
            paths,
            agents,
            rules,
        }
    }

    /// Render the set as a Markdown section a task can carry: a `##` heading,
    /// then one `###` subsection per agent with its framing, then each of that
    /// agent's rules as a `####` heading with its description and the paths it
    /// judges. A heading inside a framing or description is demoted below the
    /// rule heading (fenced code is left alone), so it cannot open a section of
    /// the document it is pasted into.
    pub fn to_markdown(&self) -> String {
        let mut out: Vec<String> = vec![
            "## llmlint rules for these paths".to_string(),
            String::new(),
        ];
        if self.rules.is_empty() {
            out.push("No llmlint rule judges these paths.".to_string());
            return finish(out);
        }
        for agent in &self.agents {
            out.push(format!("### Agent `{}`", agent.name));
            out.push(String::new());
            let framing = demoted(&agent.framing);
            if !framing.is_empty() {
                out.push(framing);
                out.push(String::new());
            }
            for rule in self.rules.iter().filter(|r| r.agent == agent.name) {
                out.push(format!("#### `{}`", rule.name));
                out.push(String::new());
                out.push(demoted(&rule.description));
                out.push(String::new());
                let paths: Vec<String> = rule.paths.iter().map(|p| format!("`{p}`")).collect();
                out.push(format!("Judges: {}", paths.join(", ")));
                out.push(String::new());
            }
        }
        finish(out)
    }
}

fn finish(out: Vec<String>) -> String {
    format!("{}\n", out.join("\n").trim_end())
}

/// Trim `text` and push every ATX heading outside a fenced code block below the
/// `####` rule heading, so pasted prose can never open a sibling section.
fn demoted(text: &str) -> String {
    let mut fence: Option<&str> = None;
    text.trim()
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
            match (fence, marker) {
                (None, Some(m)) => fence = Some(m),
                (Some(open), Some(m)) if open == m => fence = None,
                _ => {}
            }
            if fence.is_none() && marker.is_none() && trimmed.starts_with('#') {
                format!("####{trimmed}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, agent: &str, paths: &[&str]) -> RuleEntry {
        RuleEntry {
            name: name.into(),
            description: format!("{name} holds."),
            agent: agent.into(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn agents_are_listed_once_in_first_rule_order() {
        let doc = RulesDocument::new(
            vec!["a".into()],
            vec![
                rule("r1", "style", &["a"]),
                rule("r2", "default", &["a"]),
                rule("r3", "style", &["a"]),
            ],
            |name| format!("frame {name}"),
        );
        let names: Vec<&str> = doc.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["style", "default"]);
        assert_eq!(doc.agents[0].framing, "frame style");
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn markdown_groups_rules_under_their_agent_and_demotes_headings() {
        let mut r = rule("r1", "style", &["src/a.rs", "src/b.rs"]);
        r.description = "# Heading\nbody\n```sh\n# a comment\n```".into();
        let doc = RulesDocument::new(vec![], vec![r], |_| "## Read carefully".into());
        let md = doc.to_markdown();
        assert!(md.starts_with("## llmlint rules for these paths\n"), "{md}");
        assert!(
            md.contains("### Agent `style`\n\n###### Read carefully\n"),
            "{md}"
        );
        assert!(
            md.contains("#### `r1`\n\n##### Heading\nbody\n```sh\n# a comment\n```"),
            "{md}"
        );
        assert!(md.contains("Judges: `src/a.rs`, `src/b.rs`"), "{md}");
        assert!(md.ends_with("`src/b.rs`\n"), "{md}");
    }

    #[test]
    fn markdown_says_so_when_no_rule_matches() {
        let doc = RulesDocument::new(vec!["x".into()], vec![], |_| String::new());
        assert_eq!(
            doc.to_markdown(),
            "## llmlint rules for these paths\n\nNo llmlint rule judges these paths.\n"
        );
    }

    #[test]
    fn an_agent_with_no_framing_renders_no_blank_paragraph() {
        let doc = RulesDocument::new(vec![], vec![rule("r", "default", &["a"])], |_| {
            String::new()
        });
        assert!(doc
            .to_markdown()
            .contains("### Agent `default`\n\n#### `r`"));
    }

    /// The committed golden is the schema the declaration generates, so a shape
    /// change cannot land without the golden (and [`SCHEMA_VERSION`]) moving.
    #[test]
    fn committed_schema_matches_the_declaration() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rules_document/schema.json"
        );
        let generated = format!("{}\n", serde_json::to_string_pretty(&schema()).unwrap());
        if std::env::var_os("LLMLINT_UPDATE_SCHEMA").is_some() {
            std::fs::write(path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            committed, generated,
            "rules document schema drifted; regenerate with LLMLINT_UPDATE_SCHEMA=1"
        );
    }
}
