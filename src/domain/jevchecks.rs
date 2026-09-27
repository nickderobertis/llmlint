//! `llmlint jev-checks`: have a harness compile rules into Jev `checks` — the
//! atomic violation checks Jev answers far better than a whole rule (see
//! `jev: { checks }` in the README). Pure: the prompt, the output schema, and
//! reading/rendering the answer; the harness call lives in the command.
//!
//! The prompt is the one that produced the checks measured on the eval corpus
//! (`evals/`): written from the rule text alone, they took Jev's held-out
//! recall to the harness's on every unit.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::domain::config::JevUnit;
use crate::domain::jev::MAX_CHECKS;

/// One rule, as the harness is briefed on it.
#[derive(Debug, Clone)]
pub struct RuleBrief {
    pub name: String,
    pub description: String,
    pub relevance: Option<String>,
    pub unit: JevUnit,
    pub pattern: Option<String>,
}

pub const SYSTEM: &str = "You compile lint rules into atomic violation checks for a small, \
literal-minded classifier. Answer from the rule text alone; you do not need to read any file.";

/// What the classifier sees for a unit, so each check can name it.
fn unit_view(unit: JevUnit) -> &'static str {
    match unit {
        JevUnit::File => "the whole file (`content`); phrase checks about \"the file\"",
        JevUnit::DocClaims => {
            "one doc-comment sentence (`claim`) and the documented function's code (`code`); \
             phrase checks about how the code goes against `claim`"
        }
        JevUnit::Sites => {
            "one flagged source line (`site`) inside its enclosing function (`code`), plus \
             same-file helpers it calls; phrase checks about \"the line in `site`\""
        }
        JevUnit::YamlEntries => {
            "one entry of a YAML list (`entry`); phrase checks about \"the entry\""
        }
        JevUnit::MultiFile => {
            "one item (`item`) plus all the rule's files (`files`); phrase checks about `item`"
        }
    }
}

/// The task prompt for a batch of rules.
pub fn prompt(rules: &[RuleBrief]) -> String {
    let mut out = format!(
        "Compile each lint rule below into 2-{MAX_CHECKS} ATOMIC VIOLATION CHECKS. The classifier \
answers one yes/no question at a time over a small snippet and cannot reason in several steps.\n\
\n\
Each check is one sentence describing ONE concrete way the rule can be violated, phrased so that \
\"yes, this happens\" means a violation. Together the checks must cover every way the rule's \
description can be violated; each one alone must be narrow and concrete (name concrete \
constructs, tokens, or phrases where possible). Where the rule has look-alikes that comply \
(things that look like a violation but are fine, or exemptions the rule states), fold the \
exemption into the relevant check (\"..., unless ...\"). Do not restate the whole rule as a \
check.\n\nRules:\n"
    );
    for r in rules {
        out.push_str(&format!(
            "\n- name: {}\n  the classifier sees: {}\n",
            r.name,
            unit_view(r.unit)
        ));
        if let Some(p) = &r.pattern {
            out.push_str(&format!("  lines/items are selected by the regex: {p}\n"));
        }
        if let Some(c) = &r.relevance {
            out.push_str(&format!("  only judged when: {c}\n"));
        }
        out.push_str(&format!(
            "  description: {}\n",
            r.description.trim().replace('\n', " ")
        ));
    }
    out
}

/// The output schema: every rule, with its checks.
pub fn schema(names: &[&str]) -> Value {
    let props: serde_json::Map<String, Value> = names
        .iter()
        .map(|n| {
            (
                n.to_string(),
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "checks": {
                            "type": "array",
                            "items": { "type": "string", "minLength": 1 },
                            "minItems": 1,
                            "maxItems": MAX_CHECKS,
                        }
                    },
                    "required": ["checks"],
                }),
            )
        })
        .collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": props,
        "required": names,
    })
}

/// Read the harness's answer: every asked rule's checks, trimmed and
/// validated against the same limits config validation applies.
pub fn parse(structured: &Value, names: &[&str]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::new();
    for name in names {
        let checks = structured
            .pointer(&format!("/{name}/checks"))
            .and_then(Value::as_array)
            .ok_or_else(|| format!("the harness returned no checks for rule {name}"))?;
        let checks: Vec<String> = checks
            .iter()
            .filter_map(Value::as_str)
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        if checks.is_empty() || checks.len() > MAX_CHECKS {
            return Err(format!(
                "the harness returned {} checks for rule {name}; expected 1-{MAX_CHECKS}",
                checks.len()
            ));
        }
        out.insert(name.to_string(), checks);
    }
    Ok(out)
}

/// A snippet to paste under a rule's `jev:` block. Each check is a JSON
/// string, which is also a valid double-quoted YAML scalar.
pub fn render(name: &str, checks: &[String]) -> String {
    let mut out = format!("# rule `{name}` — paste under its `jev:` block\nchecks:\n");
    for c in checks {
        out.push_str(&format!(
            "  - {}\n",
            serde_json::to_string(c).unwrap_or_default()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(name: &str, unit: JevUnit) -> RuleBrief {
        RuleBrief {
            name: name.into(),
            description: "Errors carry context.\nFalse when bare.".into(),
            relevance: Some("the file does I/O".into()),
            unit,
            pattern: Some(r"\?".into()),
        }
    }

    #[test]
    fn the_prompt_briefs_each_rule_with_what_jev_will_see() {
        let p = prompt(&[
            brief("ctx", JevUnit::Sites),
            brief("docs", JevUnit::DocClaims),
        ]);
        assert!(p.contains("- name: ctx\n  the classifier sees: one flagged source line (`site`)"));
        assert!(p.contains("selected by the regex: \\?"));
        assert!(p.contains("only judged when: the file does I/O"));
        assert!(p.contains("description: Errors carry context. False when bare."));
        assert!(p.contains("- name: docs\n  the classifier sees: one doc-comment sentence"));
    }

    #[test]
    fn the_schema_requires_every_rule_and_bounds_the_checks() {
        let s = schema(&["a", "b"]);
        assert_eq!(s["required"], json!(["a", "b"]));
        assert_eq!(
            s["properties"]["a"]["properties"]["checks"]["maxItems"],
            MAX_CHECKS
        );
    }

    #[test]
    fn answers_are_trimmed_validated_and_rendered_as_yaml() {
        let got = parse(
            &json!({"a": {"checks": [" one ", "", "say \"two\""]}}),
            &["a"],
        )
        .unwrap();
        assert_eq!(got["a"], ["one", "say \"two\""]);
        assert_eq!(
            render("a", &got["a"]),
            "# rule `a` — paste under its `jev:` block\nchecks:\n  - \"one\"\n  - \"say \\\"two\\\"\"\n"
        );
        assert!(parse(&json!({}), &["a"])
            .unwrap_err()
            .contains("no checks for rule a"));
        let many: Vec<String> = (0..=MAX_CHECKS).map(|i| i.to_string()).collect();
        assert!(parse(&json!({"a": {"checks": many}}), &["a"]).is_err());
    }
}
