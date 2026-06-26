//! Validate inline `llmlint: ignore` directives embedded in target-file comments.
//!
//! A target file can suppress a specific rule at a specific place with a comment:
//!
//! ```text
//! // llmlint: ignore[rule_name, other_rule] why it is safe here
//! /* llmlint: ignore-file[rule_name] generated file, reviewed elsewhere */
//! ```
//!
//! **llmlint validates only the *structure* of these directives** — the
//! suppression *behavior* is the judge's, specified in the default prompt
//! template (the judge reads the files and honors the comments it finds). The
//! structure is strict on purpose: a directive must name specific, configured
//! rule(s) and give a reason, so a typo fails the run loudly instead of silently
//! ignoring nothing (an unknown rule) — or being mistaken for a blanket ignore.
//!
//! This module is pure: it scans text and reports problems; reading files lives
//! in [`crate::io::files`] and the wiring in [`crate::commands`].

use std::collections::BTreeSet;

use crate::domain::config::is_valid_rule_name;

/// The reserved marker that introduces a directive. Detection is case-sensitive
/// and only fires when the marker is followed by an `ignore` / `ignore-file`
/// keyword, so prose that merely mentions `llmlint:` is left alone.
const MARKER: &str = "llmlint:";

/// A single malformed directive, located for a `file:line: message` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// 1-based line number of the offending directive within the file.
    pub line: usize,
    /// What is wrong and how to fix it.
    pub message: String,
}

/// Scan `text` for `llmlint: ignore` / `llmlint: ignore-file` directives and
/// return every structural problem (an empty vec means all directives — if any —
/// are well-formed). `known_rules` is the set of configured rule names a
/// directive may reference; a directive naming anything else is a typo and an
/// error. Only the first marker on a line is treated as a directive.
pub fn validate(text: &str, known_rules: &BTreeSet<&str>) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let Some(pos) = raw.find(MARKER) else {
            continue;
        };
        let after = raw[pos + MARKER.len()..].trim_start();
        // Only `ignore` / `ignore-file` are directives; `ignore-file` is tried
        // first so its `-file` suffix isn't swallowed by the `ignore` branch.
        // Anything else after `llmlint:` is treated as prose and skipped.
        let body = strip_keyword(after, "ignore-file").or_else(|| strip_keyword(after, "ignore"));
        let Some(body) = body else {
            continue;
        };
        for message in check_body(body, known_rules) {
            problems.push(Problem {
                line: idx + 1,
                message,
            });
        }
    }
    problems
}

/// If `s` starts with `keyword` followed by a boundary (end of line, whitespace,
/// or the opening `[`), return the remainder after the keyword. The boundary
/// check keeps `ignored`/`ignore-file` from matching the `ignore` keyword.
fn strip_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() || c == '[' => Some(rest),
        _ => None,
    }
}

/// Validate the `[rules] reason` body of a recognized directive, returning a
/// message per problem (empty = well-formed). Collects every problem so one run
/// surfaces all fixes for the directive at once.
fn check_body(body: &str, known_rules: &BTreeSet<&str>) -> Vec<String> {
    let mut issues = Vec::new();
    let body = body.trim_start();

    if !body.starts_with('[') {
        issues.push(
            "name the rule(s) to ignore in brackets, e.g. `ignore[rule_name] <reason>`".into(),
        );
        return issues;
    }
    let Some(close) = body.find(']') else {
        issues.push("unterminated rule list: add a closing `]` after the rule name(s)".into());
        return issues;
    };

    let inside = &body[1..close];
    let rules: Vec<&str> = inside
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if rules.is_empty() {
        issues.push("name at least one rule to ignore inside the brackets".into());
    }
    for r in &rules {
        if !is_valid_rule_name(r) {
            issues.push(format!(
                "{r:?} is not a valid rule name (letters, digits, underscore; \
                 must start with a letter)"
            ));
        } else if !known_rules.contains(r) {
            issues.push(format!(
                "unknown rule {r:?}; configured rules: {}",
                available(known_rules)
            ));
        }
    }

    if reason_of(&body[close + 1..]).is_empty() {
        issues
            .push("give a reason after the brackets explaining why the rule(s) are ignored".into());
    }
    issues
}

/// The reason text following the `]`, with trailing whitespace and the common
/// block-comment terminators (`*/`, `-->`) stripped so a bare
/// `/* llmlint: ignore[r] */` is correctly seen as having *no* reason.
fn reason_of(after_bracket: &str) -> &str {
    let mut reason = after_bracket.trim();
    loop {
        let stripped = reason
            .strip_suffix("*/")
            .or_else(|| reason.strip_suffix("-->"))
            .map(str::trim_end);
        match stripped {
            Some(s) if s.len() != reason.len() => reason = s,
            _ => break,
        }
    }
    reason
}

/// Render the configured rule names for an error message (already sorted by the
/// `BTreeSet`), or `(none)` when there are none.
fn available(known_rules: &BTreeSet<&str>) -> String {
    if known_rules.is_empty() {
        "(none)".to_string()
    } else {
        known_rules.iter().copied().collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known<'a>(names: &[&'a str]) -> BTreeSet<&'a str> {
        names.iter().copied().collect()
    }

    fn messages(text: &str, names: &[&str]) -> Vec<String> {
        validate(text, &known(names))
            .into_iter()
            .map(|p| format!("{}:{}", p.line, p.message))
            .collect()
    }

    #[test]
    fn well_formed_line_and_file_directives_have_no_problems() {
        let text = "// llmlint: ignore[no_todo] tracked in JIRA-1\n\
                    /* llmlint: ignore-file[no_todo, no_sql] generated */\n\
                    # llmlint: ignore[no_sql] one-off migration script\n";
        assert!(validate(text, &known(&["no_todo", "no_sql"])).is_empty());
    }

    #[test]
    fn prose_mentioning_the_marker_is_not_a_directive() {
        // `llmlint:` not followed by a keyword, and `ignored`/`ignore-foo` near
        // misses, are all prose — never directives, so never errors.
        let text = "// see llmlint: docs for the ignore feature\n\
                    // we llmlint: ignored this once (prose)\n\
                    // llmlint: ignore-foo[x] not a keyword\n";
        assert!(validate(text, &known(&["x"])).is_empty());
    }

    #[test]
    fn bare_ignore_without_brackets_is_rejected() {
        let msgs = messages("// llmlint: ignore please\n", &["r"]);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].starts_with("1:"));
        assert!(msgs[0].contains("brackets"), "got: {msgs:?}");
    }

    #[test]
    fn empty_bracket_list_is_rejected() {
        let msgs = messages("// llmlint: ignore[] reason\n", &["r"]);
        assert!(msgs.iter().any(|m| m.contains("at least one rule")));
    }

    #[test]
    fn unknown_rule_is_rejected_and_lists_configured() {
        let msgs = messages("// llmlint: ignore[typo] reason\n", &["alpha", "beta"]);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("unknown rule"), "got: {msgs:?}");
        assert!(msgs[0].contains("alpha, beta"), "got: {msgs:?}");
    }

    #[test]
    fn invalid_rule_name_is_rejected() {
        let msgs = messages("// llmlint: ignore[bad-name] reason\n", &["bad"]);
        assert!(msgs.iter().any(|m| m.contains("not a valid rule name")));
    }

    #[test]
    fn missing_reason_is_rejected_including_block_comment_close() {
        // No reason at all, and a block comment whose only trailing text is the
        // terminator — both must be flagged as missing a reason.
        let msgs = messages("// llmlint: ignore[r]\n/* llmlint: ignore[r] */\n", &["r"]);
        assert_eq!(msgs.len(), 2, "got: {msgs:?}");
        assert!(msgs.iter().all(|m| m.contains("give a reason")));
        assert!(msgs[0].starts_with("1:"));
        assert!(msgs[1].starts_with("2:"));
    }

    #[test]
    fn reason_before_block_comment_terminator_is_accepted() {
        let text = "/* llmlint: ignore[r] legacy shim, see #42 */\n\
                    <!-- llmlint: ignore-file[r] vendored doc -->\n";
        assert!(validate(text, &known(&["r"])).is_empty());
    }

    #[test]
    fn unterminated_bracket_list_is_rejected() {
        let msgs = messages("// llmlint: ignore[r reason\n", &["r"]);
        assert!(msgs.iter().any(|m| m.contains("unterminated")));
    }

    #[test]
    fn multiple_problems_on_one_directive_are_all_reported() {
        // Unknown rule AND no reason: both surface for the one line.
        let msgs = messages("// llmlint: ignore[ghost]\n", &["real"]);
        assert_eq!(msgs.len(), 2, "got: {msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("unknown rule")));
        assert!(msgs.iter().any(|m| m.contains("give a reason")));
    }

    #[test]
    fn available_says_none_when_no_rules_configured() {
        let msgs = messages("// llmlint: ignore[x] reason\n", &[]);
        assert!(msgs[0].contains("(none)"), "got: {msgs:?}");
    }

    #[test]
    fn trailing_directive_after_real_code_is_validated() {
        // A trailing comment on a code line is still found and validated.
        let msgs = messages("let x = todo();  // llmlint: ignore[r]\n", &["r"]);
        assert!(msgs.iter().any(|m| m.contains("give a reason")));
    }
}
