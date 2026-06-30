//! Validate inline `llmlint: ignore` directives embedded in target-file comments.
//!
//! A target file can suppress a specific rule at a specific place with a comment:
//!
//! ```text
//! // llmlint: ignore[rule_name, other_rule] why it is safe here
//! /* llmlint: ignore-file[rule_name] generated file, reviewed elsewhere */
//! // llmlint: ignore-block[rule_name] reason the block below is exempt
//! // llmlint: ignore-end[rule_name]
//! ```
//!
//! **llmlint validates only the *structure* of these directives** — the
//! suppression *behavior* is the judge's, specified in the default prompt
//! template (the judge reads the files and honors the comments it finds). The
//! structure is strict on purpose: a directive must name specific, configured
//! rule(s) and give a reason, so a typo fails the run loudly instead of silently
//! ignoring nothing (an unknown rule) — or being mistaken for a blanket ignore.
//! The exception is `ignore-end`, which merely closes an open `ignore-block` and
//! so carries no reason of its own.
//!
//! `ignore-block` / `ignore-end` come in matched pairs scoped per rule: a block
//! opens for the rules it names and stays open until an `ignore-end` naming the
//! same rule(s). Blocks track each rule independently, so two rules opened
//! together may be closed at different points. llmlint checks the pairing
//! deterministically — every opened block must close, an `ignore-end` must have a
//! matching open block, and a rule cannot be opened twice without closing.
//!
//! This module is pure: it scans text and reports problems; reading files lives
//! in [`crate::io::files`] and the wiring in [`crate::commands`].

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::domain::config::is_valid_rule_name;

/// The reserved marker that introduces a directive. Detection is case-sensitive
/// and only fires when the marker is followed by a recognized keyword
/// (`ignore` / `ignore-file` / `ignore-block` / `ignore-end`), so prose that
/// merely mentions `llmlint:` is left alone.
const MARKER: &str = "llmlint:";

/// A single malformed directive, located for a `file:line: message` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// 1-based line number of the offending directive within the file.
    pub line: usize,
    /// What is wrong and how to fix it.
    pub message: String,
}

/// Which directive a line carries, with the keyword used to build help text.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// `ignore` — line-scoped; needs a reason.
    Line,
    /// `ignore-file` — file-scoped; needs a reason.
    File,
    /// `ignore-block` — opens a block for the named rules; needs a reason.
    BlockStart,
    /// `ignore-end` — closes a block for the named rules; needs no reason.
    BlockEnd,
}

impl Kind {
    /// The keyword spelling, used to tailor help-text examples to the directive.
    fn keyword(self) -> &'static str {
        match self {
            Kind::Line => "ignore",
            Kind::File => "ignore-file",
            Kind::BlockStart => "ignore-block",
            Kind::BlockEnd => "ignore-end",
        }
    }

    /// Whether a reason is required after the bracketed rule list.
    fn needs_reason(self) -> bool {
        !matches!(self, Kind::BlockEnd)
    }
}

/// Scan `text` for `llmlint: ignore*` directives and return every structural
/// problem (an empty vec means all directives — if any — are well-formed).
/// `known_rules` is the set of configured rule names a directive may reference;
/// a directive naming anything else is a typo and an error. Only the first
/// marker on a line is treated as a directive. Problems are returned in line
/// order.
pub fn validate(text: &str, known_rules: &BTreeSet<&str>) -> Vec<Problem> {
    let mut problems = Vec::new();
    // Rules with an open `ignore-block`, mapped to the line that opened them, so
    // an unclosed block can be reported at its origin and a re-open can name the
    // earlier one. Each rule is tracked independently.
    let mut open: BTreeMap<&str, usize> = BTreeMap::new();

    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        let Some(pos) = raw.find(MARKER) else {
            continue;
        };
        let after = raw[pos + MARKER.len()..].trim_start();
        let Some((kind, body)) = classify(after) else {
            continue;
        };

        let (messages, rules) = check_body(body, known_rules, kind);
        for message in messages {
            problems.push(Problem { line, message });
        }

        match kind {
            Kind::Line | Kind::File => {}
            Kind::BlockStart => {
                for r in rules {
                    if let Some(prev) = open.get(r) {
                        problems.push(Problem {
                            line,
                            message: format!(
                                "rule {r:?} already has an open ignore-block at line {prev}; \
                                 close it with `ignore-end[{r}]` before opening another"
                            ),
                        });
                    } else {
                        open.insert(r, line);
                    }
                }
            }
            Kind::BlockEnd => {
                for r in rules {
                    if open.remove(r).is_none() {
                        problems.push(Problem {
                            line,
                            message: format!(
                                "ignore-end for rule {r:?} with no open ignore-block above it"
                            ),
                        });
                    }
                }
            }
        }
    }

    // Anything still open at end of file was never closed.
    for (r, opened) in open {
        problems.push(Problem {
            line: opened,
            message: format!(
                "unclosed ignore-block for rule {r:?}; add a matching `llmlint: ignore-end[{r}]`"
            ),
        });
    }

    // Keep the report in source order even though unclosed blocks are appended
    // last; the stable sort preserves per-line message order.
    problems.sort_by_key(|p| p.line);
    problems
}

/// The inline ignores active in one file, as line spans per rule, so a verdict's
/// violations can be suppressed deterministically (rather than relying on the
/// judge to honor the comments). Built by [`suppressions`] from text that has
/// already passed [`validate`], so only well-formed, configured directives count.
#[derive(Debug, Clone, Default)]
pub struct Suppressions {
    /// Rules suppressed for the whole file (`ignore-file`).
    file_scoped: BTreeSet<String>,
    /// Per-rule inclusive 1-based line ranges covered by line/block directives.
    ranges: BTreeMap<String, Vec<(usize, usize)>>,
}

impl Suppressions {
    /// Whether an inline ignore covers a violation of `rule` at `line` (1-based).
    /// A file-scoped ignore covers every line — including an unlocated (`None`)
    /// violation; a line/block ignore covers only the lines it spans, so it never
    /// matches an unlocated violation.
    pub fn covers(&self, rule: &str, line: Option<u64>) -> bool {
        if self.file_scoped.contains(rule) {
            return true;
        }
        let Some(line) = line else {
            return false;
        };
        let line = line as usize;
        self.ranges
            .get(rule)
            .is_some_and(|rs| rs.iter().any(|(s, e)| *s <= line && line <= *e))
    }

    /// True when the file carries no honor-able ignore directives.
    pub fn is_empty(&self) -> bool {
        self.file_scoped.is_empty() && self.ranges.is_empty()
    }
}

/// Parse `text` into the inline-ignore [`Suppressions`] it declares. Mirrors
/// [`validate`]'s scan but keeps the line spans rather than the structural
/// problems: a line-scoped `ignore` covers its own line and the one below it (a
/// trailing comment vs. a comment on its own line), `ignore-file` covers the
/// whole file, and an `ignore-block` … `ignore-end` pair covers every line from
/// the open to the close. Only configured rules (`known`) and well-formed
/// directives count — the caller validates structure first, so a malformed
/// directive has already failed the run before this is reached.
pub fn suppressions(text: &str, known: &BTreeSet<&str>) -> Suppressions {
    let mut out = Suppressions::default();
    let mut open: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_line = 0usize;

    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        last_line = line;
        let Some(pos) = raw.find(MARKER) else {
            continue;
        };
        let after = raw[pos + MARKER.len()..].trim_start();
        let Some((kind, body)) = classify(after) else {
            continue;
        };
        let (_problems, rules) = check_body(body, known, kind);

        match kind {
            Kind::Line => {
                for r in rules {
                    // Own line and the line immediately below it.
                    out.ranges
                        .entry(r.to_string())
                        .or_default()
                        .push((line, line + 1));
                }
            }
            Kind::File => {
                for r in rules {
                    out.file_scoped.insert(r.to_string());
                }
            }
            Kind::BlockStart => {
                for r in rules {
                    open.entry(r.to_string()).or_insert(line);
                }
            }
            Kind::BlockEnd => {
                for r in rules {
                    if let Some(start) = open.remove(r) {
                        out.ranges
                            .entry(r.to_string())
                            .or_default()
                            .push((start, line));
                    }
                }
            }
        }
    }
    // An unclosed block (validate would have rejected it) defensively covers to
    // end of file so its region is never under-suppressed.
    for (r, start) in open {
        out.ranges
            .entry(r)
            .or_default()
            .push((start, last_line.max(start)));
    }
    out
}

/// Classify the text after the `llmlint:` marker into a directive kind plus the
/// `[rules] …` body. The `-file` / `-block` / `-end` variants are tried before
/// bare `ignore` so their suffix isn't swallowed by the `ignore` branch.
/// Anything else after `llmlint:` is prose and yields `None`.
fn classify(after: &str) -> Option<(Kind, &str)> {
    for (keyword, kind) in [
        ("ignore-file", Kind::File),
        ("ignore-block", Kind::BlockStart),
        ("ignore-end", Kind::BlockEnd),
        ("ignore", Kind::Line),
    ] {
        if let Some(body) = strip_keyword(after, keyword) {
            return Some((kind, body));
        }
    }
    None
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

/// Validate the `[rules] reason` body of a recognized directive. Returns a
/// message per structural problem (empty = well-formed) plus the valid, configured
/// rule names parsed from the brackets — the latter feeds block pairing, so it
/// excludes any rule that is itself malformed or unknown (those surface as their
/// own problems). Collects every problem so one run surfaces all fixes at once.
fn check_body<'a>(
    body: &'a str,
    known_rules: &BTreeSet<&str>,
    kind: Kind,
) -> (Vec<String>, Vec<&'a str>) {
    let mut issues = Vec::new();
    let mut valid = Vec::new();
    let body = body.trim_start();
    let keyword = kind.keyword();
    let example = if kind.needs_reason() {
        format!("`{keyword}[rule_name] <reason>`")
    } else {
        format!("`{keyword}[rule_name]`")
    };

    if !body.starts_with('[') {
        issues.push(format!("name the rule(s) in brackets, e.g. {example}"));
        return (issues, valid);
    }
    let Some(close) = body.find(']') else {
        issues.push("unterminated rule list: add a closing `]` after the rule name(s)".into());
        return (issues, valid);
    };

    let inside = &body[1..close];
    let rules: Vec<&str> = inside
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if rules.is_empty() {
        issues.push("name at least one rule inside the brackets".into());
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
        } else {
            valid.push(*r);
        }
    }

    if kind.needs_reason() && reason_of(&body[close + 1..]).is_empty() {
        issues
            .push("give a reason after the brackets explaining why the rule(s) are ignored".into());
    }
    (issues, valid)
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

    #[test]
    fn matched_block_open_and_close_is_accepted() {
        let text = "// llmlint: ignore-block[r] legacy region, see #7\n\
                    fn f() {}\n\
                    // llmlint: ignore-end[r]\n";
        assert!(validate(text, &known(&["r"])).is_empty());
    }

    #[test]
    fn block_open_without_reason_is_rejected() {
        let msgs = messages(
            "// llmlint: ignore-block[r]\n// llmlint: ignore-end[r]\n",
            &["r"],
        );
        assert_eq!(msgs.len(), 1, "got: {msgs:?}");
        assert!(msgs[0].contains("give a reason"), "got: {msgs:?}");
    }

    #[test]
    fn block_end_needs_no_reason() {
        // A bare `ignore-end` (no trailing text) is well-formed.
        let text = "// llmlint: ignore-block[r] reason\n// llmlint: ignore-end[r]\n";
        assert!(validate(text, &known(&["r"])).is_empty());
    }

    #[test]
    fn unclosed_block_is_reported_at_its_opening_line() {
        let msgs = messages(
            "// code\n// llmlint: ignore-block[r] never closed\nmore code\n",
            &["r"],
        );
        assert_eq!(msgs.len(), 1, "got: {msgs:?}");
        assert!(msgs[0].starts_with("2:"), "got: {msgs:?}");
        assert!(msgs[0].contains("unclosed ignore-block"), "got: {msgs:?}");
    }

    #[test]
    fn block_end_without_a_matching_open_is_rejected() {
        let msgs = messages("// llmlint: ignore-end[r]\n", &["r"]);
        assert_eq!(msgs.len(), 1, "got: {msgs:?}");
        assert!(msgs[0].starts_with("1:"));
        assert!(msgs[0].contains("no open ignore-block"), "got: {msgs:?}");
    }

    #[test]
    fn reopening_an_open_block_for_the_same_rule_is_rejected() {
        let msgs = messages(
            "// llmlint: ignore-block[r] first\n\
             // llmlint: ignore-block[r] second\n\
             // llmlint: ignore-end[r]\n",
            &["r"],
        );
        // The second open is rejected; the first open is still closed by the end.
        assert_eq!(msgs.len(), 1, "got: {msgs:?}");
        assert!(msgs[0].starts_with("2:"), "got: {msgs:?}");
        assert!(
            msgs[0].contains("already has an open ignore-block at line 1"),
            "got: {msgs:?}"
        );
    }

    #[test]
    fn two_rules_opened_together_can_close_at_different_lines() {
        let text = "// llmlint: ignore-block[a, b] both exempt here\n\
                    fn f() {}\n\
                    // llmlint: ignore-end[a]\n\
                    fn g() {}\n\
                    // llmlint: ignore-end[b]\n";
        assert!(validate(text, &known(&["a", "b"])).is_empty());
    }

    #[test]
    fn one_of_two_opened_rules_left_unclosed_is_reported() {
        let text = "// llmlint: ignore-block[a, b] reason\n\
                    // llmlint: ignore-end[a]\n";
        let msgs = messages(text, &["a", "b"]);
        assert_eq!(msgs.len(), 1, "got: {msgs:?}");
        assert!(msgs[0].starts_with("1:"), "got: {msgs:?}");
        assert!(msgs[0].contains("unclosed ignore-block"), "got: {msgs:?}");
        assert!(msgs[0].contains("\"b\""), "got: {msgs:?}");
    }

    #[test]
    fn overlapping_blocks_for_distinct_rules_are_accepted() {
        // a opens, b opens inside it, a closes, then b closes — independent
        // per-rule tracking allows the interleave.
        let text = "// llmlint: ignore-block[a] outer\n\
                    // llmlint: ignore-block[b] inner\n\
                    // llmlint: ignore-end[a]\n\
                    // llmlint: ignore-end[b]\n";
        assert!(validate(text, &known(&["a", "b"])).is_empty());
    }

    #[test]
    fn block_directives_naming_unknown_rules_are_rejected() {
        // An unknown rule in a block directive surfaces as the unknown-rule error
        // and does not also produce spurious pairing errors.
        let msgs = messages(
            "// llmlint: ignore-block[ghost] reason\n// llmlint: ignore-end[ghost]\n",
            &["real"],
        );
        assert_eq!(msgs.len(), 2, "got: {msgs:?}");
        assert!(
            msgs.iter().all(|m| m.contains("unknown rule")),
            "got: {msgs:?}"
        );
    }

    #[test]
    fn block_problems_are_reported_in_line_order() {
        // An end-without-open on line 1 and an unclosed open on line 2: the
        // report is ordered by line even though the unclosed one is found last.
        let msgs = messages(
            "// llmlint: ignore-end[a]\n// llmlint: ignore-block[b] reason\n",
            &["a", "b"],
        );
        assert_eq!(msgs.len(), 2, "got: {msgs:?}");
        assert!(msgs[0].starts_with("1:"), "got: {msgs:?}");
        assert!(msgs[1].starts_with("2:"), "got: {msgs:?}");
    }

    #[test]
    fn suppressions_line_covers_own_line_and_the_one_below() {
        let s = suppressions("// llmlint: ignore[r] reason\ncode\nmore\n", &known(&["r"]));
        assert!(s.covers("r", Some(1)));
        assert!(s.covers("r", Some(2)));
        assert!(!s.covers("r", Some(3)));
        // An unlocated violation isn't matched by a line-scoped ignore.
        assert!(!s.covers("r", None));
        // A rule the directive doesn't name is never covered.
        assert!(!s.covers("other", Some(1)));
    }

    #[test]
    fn suppressions_file_scope_covers_every_line_including_unlocated() {
        let s = suppressions("/* llmlint: ignore-file[r] generated */\n", &known(&["r"]));
        assert!(s.covers("r", Some(999)));
        assert!(s.covers("r", None));
        assert!(!s.covers("nope", Some(1)));
    }

    #[test]
    fn suppressions_block_covers_open_through_close_inclusive() {
        let text = "// llmlint: ignore-block[r] reason\ncode\ncode\n// llmlint: ignore-end[r]\n";
        let s = suppressions(text, &known(&["r"]));
        for l in 1..=4 {
            assert!(s.covers("r", Some(l)), "line {l} should be covered");
        }
        assert!(!s.covers("r", Some(5)));
    }

    #[test]
    fn suppressions_independent_per_rule_blocks() {
        let text = "// llmlint: ignore-block[a, b] both\ncode\n// llmlint: ignore-end[a]\n\
                    code\n// llmlint: ignore-end[b]\n";
        let s = suppressions(text, &known(&["a", "b"]));
        assert!(s.covers("a", Some(2)));
        assert!(!s.covers("a", Some(4)));
        assert!(s.covers("b", Some(4)));
    }

    #[test]
    fn suppressions_empty_when_no_directives() {
        assert!(suppressions("just code\n", &known(&["r"])).is_empty());
    }
}
