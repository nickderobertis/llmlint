//! Render the judge's system prompt from a (user-customizable) template.
//!
//! Templates are [minijinja] (Jinja2-style). The context exposes `rules` (each
//! with `name`, `description`, and `rationale` — whether that rule wants a
//! justification), `files` (the target paths), and `rationales` (whether any
//! rule in this batch wants one, to gate the rationale guidance block). The
//! built-in default template lives in `assets/default_template.md` and is
//! embedded via [`crate::io::assets`].

use serde::Serialize;

use crate::errors::{Error, Result};

/// One rule as presented to the judge in the rendered prompt.
#[derive(Debug, Clone, Serialize)]
pub struct RuleSpec {
    pub name: String,
    pub description: String,
    /// Whether this rule requires a `rationale` in the judge's verdict.
    pub rationale: bool,
}

#[derive(Serialize)]
struct Context<'a> {
    rules: &'a [RuleSpec],
    files: &'a [String],
    /// True when at least one rule in this batch wants a rationale, so the
    /// template can show (or omit) the rationale guidance.
    rationales: bool,
}

/// Render `template` with the given rules and target file paths. `rationales`
/// gates the rationale guidance and is true when any rule in this batch wants one.
pub fn render(
    template: &str,
    rules: &[RuleSpec],
    files: &[String],
    rationales: bool,
) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_keep_trailing_newline(true);
    let ctx = Context {
        rules,
        files,
        rationales,
    };
    env.render_str(template, ctx)
        .map_err(|e| Error::Template(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<RuleSpec> {
        vec![
            RuleSpec {
                name: "no_inline_sql".into(),
                description: "true when no SQL is inline; false otherwise.".into(),
                rationale: true,
            },
            RuleSpec {
                name: "layered".into(),
                description: "true when layered.".into(),
                rationale: true,
            },
        ]
    }

    #[test]
    fn renders_rules_and_files() {
        let tmpl = "Files:\n{% for f in files %}- {{ f }}\n{% endfor %}\
                    Rules:\n{% for r in rules %}* {{ r.name }}: {{ r.description }}\n{% endfor %}";
        let out = render(
            tmpl,
            &rules(),
            &["src/a.rs".into(), "src/b.rs".into()],
            true,
        )
        .unwrap();
        assert!(out.contains("- src/a.rs"));
        assert!(out.contains("- src/b.rs"));
        assert!(out.contains("* no_inline_sql: true when no SQL is inline"));
        assert!(out.contains("* layered: true when layered."));
    }

    #[test]
    fn rationales_flag_and_per_rule_rationale_are_in_scope() {
        let tmpl = "{% if rationales %}WANT{% else %}SKIP{% endif %}\n\
                    {% for r in rules %}{{ r.name }}={{ r.rationale }}\n{% endfor %}";
        let on = render(tmpl, &rules(), &[], true).unwrap();
        assert!(on.contains("WANT"));
        assert!(on.contains("no_inline_sql=true"));
        let off = render(tmpl, &rules(), &[], false).unwrap();
        assert!(off.contains("SKIP"));
    }

    #[test]
    fn invalid_template_is_a_template_error() {
        let err = render("{% for x in %}", &rules(), &[], true).unwrap_err();
        assert!(matches!(err, Error::Template(_)));
    }
}
