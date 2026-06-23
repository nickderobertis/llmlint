//! Render the judge's system prompt from a (user-customizable) template.
//!
//! Templates are [minijinja] (Jinja2-style). The context exposes `rules` (each
//! with `name` and `description`) and `files` (the target paths). The built-in
//! default template lives in `assets/default_template.md` and is embedded via
//! [`crate::io::assets`].

use serde::Serialize;

use crate::errors::{Error, Result};

/// One rule as presented to the judge in the rendered prompt.
#[derive(Debug, Clone, Serialize)]
pub struct RuleSpec {
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
struct Context<'a> {
    rules: &'a [RuleSpec],
    files: &'a [String],
}

/// Render `template` with the given rules and target file paths.
pub fn render(template: &str, rules: &[RuleSpec], files: &[String]) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_keep_trailing_newline(true);
    let ctx = Context { rules, files };
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
                description: "TRUE when no SQL is inline; FALSE otherwise.".into(),
            },
            RuleSpec {
                name: "layered".into(),
                description: "TRUE when layered.".into(),
            },
        ]
    }

    #[test]
    fn renders_rules_and_files() {
        let tmpl = "Files:\n{% for f in files %}- {{ f }}\n{% endfor %}\
                    Rules:\n{% for r in rules %}* {{ r.name }}: {{ r.description }}\n{% endfor %}";
        let out = render(tmpl, &rules(), &["src/a.rs".into(), "src/b.rs".into()]).unwrap();
        assert!(out.contains("- src/a.rs"));
        assert!(out.contains("- src/b.rs"));
        assert!(out.contains("* no_inline_sql: TRUE when no SQL is inline"));
        assert!(out.contains("* layered: TRUE when layered."));
    }

    #[test]
    fn invalid_template_is_a_template_error() {
        let err = render("{% for x in %}", &rules(), &[]).unwrap_err();
        assert!(matches!(err, Error::Template(_)));
    }
}
