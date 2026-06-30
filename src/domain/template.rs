//! Render the judge's system prompt from a (user-customizable) template.
//!
//! Templates are [minijinja] (Jinja2-style). The context exposes `rules` (each
//! with `name`, `description`, and `rationale` — whether that rule wants a
//! justification), `files` (the target paths), `file_rules` (per-file which
//! rules apply — the apply- or skip-list, whichever is shorter), `diffs`
//! (per-file changed-line diffs, present only under `--diff`), and `rationales`
//! (whether any rule in this batch wants one, to gate the rationale guidance
//! block). The built-in default template lives in `assets/default_template.md`
//! and is embedded via [`crate::io::assets`].

use serde::Serialize;

use crate::domain::applicability;
use crate::errors::{Error, Result};

/// One rule as presented to the judge in the rendered prompt.
#[derive(Debug, Clone, Serialize)]
pub struct RuleSpec {
    pub name: String,
    pub description: String,
    /// Whether this rule requires a `rationale` in the judge's verdict.
    pub rationale: bool,
    /// The relevance condition the judge must decide before evaluating this rule,
    /// or `None` for an always-evaluated rule. Exposed to the template so it can
    /// show the condition and gate the verdict on a `relevant` decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relevance: Option<String>,
    /// Whether every violation of this rule must cite a concrete file + line.
    /// Exposed to the template so it can mark the rule and ask the judge to
    /// localize each violation.
    pub require_line_attribution: bool,
    /// The slash-relative files this rule applies to (a subset of the call's
    /// `files` union). Drives the per-file applicability context and the
    /// wrong-file validation.
    pub files: Vec<String>,
}

/// One target file's changed-line diff, shown to the judge under `--diff`. Kept
/// separate from `files` (a plain path list) so a custom template using
/// `{{ f }}` keeps working; the diff is also inlined per file in `file_rules`.
#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    /// The file path (forward-slash form), matching its entry in `files`.
    pub file: String,
    /// The unified diff text for that file.
    pub diff: String,
}

/// One target file as the prompt presents it: its applicability (which rules to
/// evaluate against it) and, when `--diff` surfaced a change, its unified `diff`
/// inlined so the judge sees a changed file's rules and diff together.
#[derive(Serialize)]
struct FileEntry<'a> {
    file: &'a str,
    mode: applicability::Mode,
    rules: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<&'a str>,
}

#[derive(Serialize)]
struct Context<'a> {
    rules: &'a [RuleSpec],
    files: &'a [String],
    /// Per-file presentation: for each target file, the rules that apply (or,
    /// when shorter, the rules to skip) and its inlined `diff` when changed. Lets
    /// the template tell the judge which rules to evaluate against each file and
    /// show the change right there.
    file_rules: &'a [FileEntry<'a>],
    /// Per-file diffs (only files with changes), present under `--diff` and empty
    /// otherwise. Kept for custom templates; the default template inlines them
    /// per file via `file_rules`.
    diffs: &'a [FileDiff],
    /// True when at least one rule in this batch wants a rationale, so the
    /// template can show (or omit) the rationale guidance.
    rationales: bool,
    /// True when at least one rule in this batch carries a relevance condition,
    /// so the template can show (or omit) the relevance guidance.
    relevance: bool,
    /// True when at least one rule in this batch requires line attribution, so
    /// the template can show (or omit) the line-attribution guidance.
    line_attribution: bool,
}

/// Render `template` with the given rules, target file paths, and per-file
/// `diffs` (empty unless `--diff` is set). The per-file applicability
/// (`file_rules`) is derived from each rule's `files`. `rationales` gates the
/// rationale guidance (true when any rule in this batch wants one), `relevance`
/// gates the relevance guidance (true when any rule is conditional), and
/// `line_attribution` gates the line-attribution guidance (true when any rule
/// requires every violation to cite a file + line).
pub fn render(
    template: &str,
    rules: &[RuleSpec],
    files: &[String],
    diffs: &[FileDiff],
    rationales: bool,
    relevance: bool,
    line_attribution: bool,
) -> Result<String> {
    let pairs: Vec<(String, Vec<String>)> = rules
        .iter()
        .map(|r| (r.name.clone(), r.files.clone()))
        .collect();
    let applic = applicability::per_file(&pairs, files);
    // Pair each file's applicability with its diff (changed files only) so the
    // template can inline a changed file's rules and diff together.
    let file_rules: Vec<FileEntry> = applic
        .iter()
        .map(|fr| FileEntry {
            file: &fr.file,
            mode: fr.mode,
            rules: &fr.rules,
            diff: diffs
                .iter()
                .find(|d| d.file == fr.file)
                .map(|d| d.diff.as_str()),
        })
        .collect();
    let mut env = minijinja::Environment::new();
    env.set_keep_trailing_newline(true);
    let ctx = Context {
        rules,
        files,
        file_rules: &file_rules,
        diffs,
        rationales,
        relevance,
        line_attribution,
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
                relevance: None,
                require_line_attribution: false,
                files: vec!["src/a.rs".into(), "src/b.rs".into()],
            },
            RuleSpec {
                name: "layered".into(),
                description: "true when layered.".into(),
                rationale: true,
                relevance: None,
                require_line_attribution: false,
                files: vec!["src/a.rs".into(), "src/b.rs".into()],
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
            &[],
            true,
            false,
            false,
        )
        .unwrap();
        assert!(out.contains("- src/a.rs"));
        assert!(out.contains("- src/b.rs"));
        assert!(out.contains("* no_inline_sql: true when no SQL is inline"));
        assert!(out.contains("* layered: true when layered."));
    }

    #[test]
    fn diffs_block_is_gated_and_renders_per_file() {
        // The `diffs` context block stays available for custom templates.
        let tmpl = "{% if diffs %}CHANGED\n{% for d in diffs %}{{ d.file }}:\n{{ d.diff }}\
                    {% endfor %}{% else %}WHOLE{% endif %}";
        // No diffs (the default): the gate is off.
        let off = render(
            tmpl,
            &rules(),
            &["src/a.rs".into()],
            &[],
            true,
            false,
            false,
        )
        .unwrap();
        assert!(off.contains("WHOLE"), "got: {off}");
        // With a diff: the block renders the file path and its diff text.
        let diffs = vec![FileDiff {
            file: "src/a.rs".into(),
            diff: "@@ -1 +1 @@\n-old\n+new\n".into(),
        }];
        let on = render(
            tmpl,
            &rules(),
            &["src/a.rs".into()],
            &diffs,
            true,
            false,
            false,
        )
        .unwrap();
        assert!(on.contains("CHANGED"), "got: {on}");
        assert!(on.contains("src/a.rs:"), "got: {on}");
        assert!(on.contains("+new"), "got: {on}");
    }

    #[test]
    fn file_rules_inline_the_diff_for_a_changed_file_only() {
        // Each file_rules entry carries its diff when changed, so the template can
        // show a changed file's rules and diff together; an unchanged file has none.
        let tmpl = "{% for fr in file_rules %}{{ fr.file }}\
                    {% if fr.diff %} DIFF[{{ fr.diff }}]{% endif %}\n{% endfor %}";
        let diffs = vec![FileDiff {
            file: "src/a.rs".into(),
            diff: "@@ -1 +1 @@\n+new\n".into(),
        }];
        let out = render(
            tmpl,
            &rules(),
            &["src/a.rs".into(), "src/b.rs".into()],
            &diffs,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(
            out.contains("src/a.rs DIFF[@@ -1 +1 @@\n+new\n]"),
            "out:\n{out}"
        );
        // The unchanged file gets no inlined diff.
        assert!(out.contains("src/b.rs\n"), "out:\n{out}");
        assert!(!out.contains("src/b.rs DIFF"), "out:\n{out}");
    }

    #[test]
    fn rationales_flag_and_per_rule_rationale_are_in_scope() {
        let tmpl = "{% if rationales %}WANT{% else %}SKIP{% endif %}\n\
                    {% for r in rules %}{{ r.name }}={{ r.rationale }}\n{% endfor %}";
        let on = render(tmpl, &rules(), &[], &[], true, false, false).unwrap();
        assert!(on.contains("WANT"));
        assert!(on.contains("no_inline_sql=true"));
        let off = render(tmpl, &rules(), &[], &[], false, false, false).unwrap();
        assert!(off.contains("SKIP"));
    }

    #[test]
    fn relevance_flag_and_per_rule_condition_are_in_scope() {
        let tmpl = "{% if relevance %}GATE{% else %}NOGATE{% endif %}\n\
                    {% for r in rules %}{% if r.relevance %}{{ r.name }}: {{ r.relevance }}\n\
                    {% endif %}{% endfor %}";
        let mut rs = rules();
        rs[0].relevance = Some("the change touches SQL".into());
        let on = render(tmpl, &rs, &[], &[], true, true, false).unwrap();
        assert!(on.contains("GATE"));
        assert!(on.contains("no_inline_sql: the change touches SQL"));
        // The always-evaluated rule renders no condition line.
        assert!(!on.contains("layered:"));
        let off = render(tmpl, &rules(), &[], &[], true, false, false).unwrap();
        assert!(off.contains("NOGATE"));
    }

    #[test]
    fn line_attribution_flag_and_per_rule_marker_are_in_scope() {
        let tmpl = "{% if line_attribution %}LOCALIZE{% else %}ANYWHERE{% endif %}\n\
                    {% for r in rules %}{% if r.require_line_attribution %}{{ r.name }} pinned\n\
                    {% endif %}{% endfor %}";
        let mut rs = rules();
        rs[0].require_line_attribution = true;
        let on = render(tmpl, &rs, &[], &[], true, false, true).unwrap();
        assert!(on.contains("LOCALIZE"));
        assert!(on.contains("no_inline_sql pinned"));
        // The rule that doesn't require attribution renders no marker line.
        assert!(!on.contains("layered pinned"));
        let off = render(tmpl, &rules(), &[], &[], true, false, false).unwrap();
        assert!(off.contains("ANYWHERE"));
    }

    #[test]
    fn file_rules_expose_per_file_applicability() {
        // Two rules with different file scopes: a.rs gets only_a, b.rs gets only_b.
        let rs = vec![
            RuleSpec {
                name: "only_a".into(),
                description: "d".into(),
                rationale: false,
                relevance: None,
                require_line_attribution: false,
                files: vec!["src/a.rs".into()],
            },
            RuleSpec {
                name: "only_b".into(),
                description: "d".into(),
                rationale: false,
                relevance: None,
                require_line_attribution: false,
                files: vec!["src/b.rs".into()],
            },
        ];
        let tmpl = "{% for fr in file_rules %}{{ fr.file }}:{{ fr.mode }}:\
                    {% for r in fr.rules %}{{ r }} {% endfor %}\n{% endfor %}";
        let out = render(
            tmpl,
            &rs,
            &["src/a.rs".into(), "src/b.rs".into()],
            &[],
            false,
            false,
            false,
        )
        .unwrap();
        assert!(out.contains("src/a.rs:include:only_a"), "out:\n{out}");
        assert!(out.contains("src/b.rs:include:only_b"), "out:\n{out}");
    }

    #[test]
    fn invalid_template_is_a_template_error() {
        let err = render("{% for x in %}", &rules(), &[], &[], true, false, false).unwrap_err();
        assert!(matches!(err, Error::Template(_)));
    }
}
