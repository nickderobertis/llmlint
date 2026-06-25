//! Fixtures shared by the bench targets (`engine`, `engine_allocs`). This file
//! lives in a subdirectory so cargo's bench auto-discovery never treats it as a
//! target of its own; each bench pulls it in with `#[path]`.

// Each bench target uses a subset of these helpers; the unused remainder in any
// one target is expected.
#![allow(dead_code)]

use std::path::PathBuf;

use llmlint::domain::plan::ResolvedRule;
use llmlint::domain::template::RuleSpec;
use llmlint::domain::verdict::{Outcome, RuleOutcome, RuleVerdict, Violation};
use llmlint::io::{assets, configfs};

/// The built-in master prompt template — the same one a default `llmlint` run
/// renders. The realistic floor for the `template_render` group.
pub fn example_template() -> &'static str {
    assets::DEFAULT_TEMPLATE
}

/// The two bundled config documents (`llmlint init`'s starter config and the
/// bundled config-lint plugin), labelled. The realistic floor for the
/// `config_parse` group: the exact YAML the binary parses on a default run.
pub fn example_configs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", assets::INIT_CONFIG),
        ("config-lint", assets::CONFIG_LINT_PLUGIN),
    ]
}

/// Rule specs as presented to a judge, taken from the bundled config-lint
/// plugin so the render/schema numbers reflect real rule text rather than
/// synthetic stubs.
pub fn example_rule_specs() -> Vec<RuleSpec> {
    let cfg = configfs::parse(assets::CONFIG_LINT_PLUGIN, "config-lint")
        .expect("bundled config-lint plugin parses");
    cfg.rules
        .into_iter()
        .map(|r| RuleSpec {
            name: r.name,
            description: r.description,
        })
        .collect()
}

/// A realistic set of target file paths for prompt rendering.
pub fn example_files() -> Vec<String> {
    vec![
        "src/lib.rs".into(),
        "src/domain/mod.rs".into(),
        "src/domain/plan.rs".into(),
        "src/io/configfs.rs".into(),
        "src/commands/lint.rs".into(),
    ]
}

/// Resolved rules for the planner, all sharing one agent + file set so they
/// batch together — the common case a default run plans.
pub fn example_resolved() -> Vec<ResolvedRule> {
    example_rule_specs()
        .into_iter()
        .map(|r| ResolvedRule {
            name: r.name,
            description: r.description,
            judges: 1,
            agent: "default".into(),
            files: vec![PathBuf::from("src/lib.rs")],
        })
        .collect()
}

/// `n` synthetic resolved rules with `judges` judges each, one agent + file set.
/// The planner's cost grows with both the rule count (batching) and the
/// per-rule judge count (the multi-judge fan-out emits one run per judge index),
/// so both axes are charted with these.
pub fn synthetic_resolved(n: usize, judges: u32) -> Vec<ResolvedRule> {
    (0..n)
        .map(|i| ResolvedRule {
            name: format!("rule_{i}"),
            description: format!("true when rule {i} holds; false otherwise."),
            judges,
            agent: "default".into(),
            files: vec![PathBuf::from("src/lib.rs")],
        })
        .collect()
}

/// A YAML config of `n` rules, for charting how config parse + deserialization
/// scales with rule count.
pub fn synthetic_config_text(n: usize) -> String {
    let mut s = String::from("version: 1\nrules:\n");
    for i in 0..n {
        s.push_str(&format!(
            "  - name: rule_{i}\n    description: \"true when rule {i} holds; false otherwise.\"\n"
        ));
    }
    s
}

/// `n` synthetic rule specs (name + description) for the render scaling group.
pub fn synthetic_rule_specs(n: usize) -> Vec<RuleSpec> {
    (0..n)
        .map(|i| RuleSpec {
            name: format!("rule_{i}"),
            description: format!("true when rule {i} holds; false otherwise."),
        })
        .collect()
}

/// `n` synthetic rule names for the schema-build scaling group.
pub fn synthetic_rule_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("rule_{i}")).collect()
}

/// `n` judge verdicts for one rule. When `dissent`, the verdicts split so the
/// tally must take the fail branch and union + de-duplicate violations — the
/// work the all-agree happy path skips.
pub fn judge_verdicts(n: usize, dissent: bool) -> Vec<RuleVerdict> {
    (0..n)
        .map(|i| {
            let holds = if dissent { i % 2 == 0 } else { true };
            RuleVerdict {
                holds,
                violations: if holds {
                    vec![]
                } else {
                    vec![Violation {
                        file: Some(format!("src/file_{i}.rs")),
                        line: Some(i as u64 + 1),
                        end_line: None,
                        message: Some(format!("violation {i}")),
                    }]
                },
            }
        })
        .collect()
}

/// `n` rule outcomes (a pass / fail-with-violation / skip mix) for the report
/// formatter groups.
pub fn outcomes(n: usize) -> Vec<RuleOutcome> {
    (0..n)
        .map(|i| match i % 3 {
            0 => RuleOutcome {
                name: format!("rule_{i}"),
                outcome: Outcome::Pass,
                votes_total: 1,
                votes_hold: 1,
                violations: vec![],
            },
            1 => RuleOutcome {
                name: format!("rule_{i}"),
                outcome: Outcome::Fail,
                votes_total: 3,
                votes_hold: 1,
                violations: vec![Violation {
                    file: Some(format!("src/file_{i}.rs")),
                    line: Some(i as u64 + 1),
                    end_line: Some(i as u64 + 3),
                    message: Some(format!("problem {i}")),
                }],
            },
            _ => RuleOutcome::skipped(format!("rule_{i}")),
        })
        .collect()
}
