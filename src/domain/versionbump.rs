//! Deterministic "a changed versioned config must bump its version" check.
//!
//! A config file that declares a top-level `version:` is a **published plugin**:
//! consumers pin it with an `@` suffix (`url@1`) and pick up any `1.x` under that
//! pin with no pin change (see [`crate::domain::version`]). So if such a file's
//! content changes but its `version` stays the same, every consumer silently gets
//! new behavior under an unchanged version — the exact drift a version is meant to
//! signal. This check makes that a hard error: a versioned config that changed vs
//! a base must also change its `version`.
//!
//! It is a static, model-free complement to the LLM-as-judge config lint — like
//! `check-ignores`, it belongs in the fast deterministic loop next to fmt/clippy.
//! This module is **pure**: it decides from a file's own text (does it declare a
//! version?) and its unified diff (did the top-level `version:` line change?)
//! alone. The I/O — resolving the config files and computing their diffs — lives
//! in [`crate::commands::version_bump`].

/// Does `text` declare a top-level `version:` key? Only an **unindented**
/// `version:` line counts — a nested `version:` (indented, so it belongs to some
/// other mapping) is not the config's own published version and is ignored.
pub fn declares_version(text: &str) -> bool {
    text.lines().any(|l| top_level_version_value(l).is_some())
}

/// The normalized value of a top-level `version:` line (e.g. `"1.1"`), or `None`
/// when `line` is not an unindented `version:` assignment. The value is trimmed
/// and its surrounding YAML quotes stripped, so `version: 1`, `version: "1"`, and
/// `version: '1'` all normalize to `1` — reformatting the same version is not a
/// bump. No version *parsing* happens (the caller only compares two values for
/// (in)equality), so a malformed version never turns this check into a parse
/// error; two textually-different values (`1` vs `1.1`) are simply a bump.
fn top_level_version_value(line: &str) -> Option<&str> {
    // Top-level means column 0: any leading whitespace makes it a nested key.
    if line.starts_with([' ', '\t']) {
        return None;
    }
    let rest = line.strip_prefix("version")?;
    // Allow `version:` and the rarer `version :`; reject `versioning:` etc.
    let rest = rest.trim_start_matches([' ', '\t']);
    let value = rest.strip_prefix(':')?.trim();
    Some(unquote(value))
}

/// Strip one pair of matching surrounding quotes from a YAML scalar, so a quoted
/// and unquoted spelling of the same version compare equal.
fn unquote(value: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

/// The top-level `version:` value on an **added** (`+`) diff line, if any. The
/// unified-diff `+++ b/<file>` header is not an added content line (after one `+`
/// it reads `++ b/<file>`, which is not a `version:` assignment), so it never
/// matches.
fn added_version(diff: &str) -> Option<&str> {
    diff.lines()
        .filter_map(|l| l.strip_prefix('+'))
        .find_map(top_level_version_value)
}

/// The top-level `version:` value on a **removed** (`-`) diff line, if any. The
/// `--- a/<file>` header is excluded for the same reason as [`added_version`].
fn removed_version(diff: &str) -> Option<&str> {
    diff.lines()
        .filter_map(|l| l.strip_prefix('-'))
        .find_map(top_level_version_value)
}

/// Given a **non-empty** unified `diff` for a versioned config, did the version
/// actually get bumped? True when the top-level `version:` line was changed to a
/// different value (an added version line whose value differs from the removed
/// one), or newly introduced (an added version with none removed — a brand-new
/// file). A change that leaves the `version:` line untouched — or only reformats
/// it to the same value — is **not** a bump.
pub fn is_bumped(diff: &str) -> bool {
    match added_version(diff) {
        // The version line was not among the added lines: it was not touched.
        None => false,
        // A new file (nothing removed) counts as setting the version; otherwise
        // the value must actually differ from what was there before.
        Some(added) => removed_version(diff).is_none_or(|removed| added != removed),
    }
}

/// The verdict for one versioned config: it fails only when it **changed** (has a
/// non-empty diff) but was **not** bumped. An unchanged file (`diff` is `None`,
/// or an empty/whitespace-only diff) passes trivially — there is nothing to bump.
pub fn changed_without_bump(diff: Option<&str>) -> bool {
    matches!(diff, Some(d) if !d.trim().is_empty() && !is_bumped(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_version_only_for_a_top_level_key() {
        assert!(declares_version("version: 1\nrules: []\n"));
        assert!(declares_version("rules: []\nversion: 1.2.3\n"));
        assert!(declares_version("version : 2\n")); // space before colon
                                                    // A nested/indented `version:` is some other mapping's key, not the
                                                    // config's own published version.
        assert!(!declares_version("agents:\n  a:\n    version: 1\n"));
        assert!(!declares_version("rules: []\n"));
        // A key that merely starts with "version" is not `version`.
        assert!(!declares_version("versioning: on\n"));
    }

    #[test]
    fn top_level_version_value_trims_and_ignores_indent() {
        assert_eq!(top_level_version_value("version: 1.1"), Some("1.1"));
        assert_eq!(top_level_version_value("version:1.1"), Some("1.1"));
        assert_eq!(top_level_version_value("version:   2  "), Some("2"));
        assert_eq!(top_level_version_value("  version: 1"), None);
        assert_eq!(top_level_version_value("versionfoo: 1"), None);
    }

    fn diff(body: &str) -> String {
        // A realistic per-file unified diff, headers included, so the parsing is
        // exercised against the exact shape `git diff` emits.
        format!("diff --git a/x.yml b/x.yml\nindex abc..def 100644\n--- a/x.yml\n+++ b/x.yml\n@@ -1,3 +1,3 @@\n{body}")
    }

    #[test]
    fn bumped_when_the_version_value_changes() {
        let d = diff(" rules: []\n-version: 1\n+version: 2\n");
        assert!(is_bumped(&d));
        assert!(!changed_without_bump(Some(&d)));
    }

    #[test]
    fn bumped_on_a_minor_component_change() {
        let d = diff("-version: 1\n+version: 1.1\n rules: []\n");
        assert!(is_bumped(&d));
    }

    #[test]
    fn not_bumped_when_content_changes_but_version_untouched() {
        let d = diff(" version: 1\n-  - name: a\n+  - name: b\n");
        assert!(!is_bumped(&d));
        assert!(changed_without_bump(Some(&d)));
    }

    #[test]
    fn not_bumped_when_the_version_line_is_only_reformatted_to_the_same_value() {
        // `1` -> `"1"` is the same version; reformatting it is not a bump.
        let d = diff("-version: 1\n+version: \"1\"\n rules: []\n");
        assert!(!is_bumped(&d));
    }

    #[test]
    fn a_new_file_counts_as_setting_the_version() {
        // A brand-new versioned config is all `+` lines with nothing removed.
        let d = "diff --git a/x.yml b/x.yml\nnew file mode 100644\n--- /dev/null\n+++ b/x.yml\n@@ -0,0 +1,2 @@\n+version: 1\n+rules: []\n";
        assert!(is_bumped(d));
        assert!(!changed_without_bump(Some(d)));
    }

    #[test]
    fn the_diff_headers_never_count_as_a_version_change() {
        // No `version:` content line anywhere: the `+++`/`---` headers must not be
        // mistaken for an added/removed version.
        let d = diff("-a: 1\n+a: 2\n");
        assert!(!is_bumped(&d));
        assert!(changed_without_bump(Some(&d)));
    }

    #[test]
    fn unchanged_files_pass_trivially() {
        assert!(!changed_without_bump(None));
        assert!(!changed_without_bump(Some("")));
        assert!(!changed_without_bump(Some("   \n")));
    }
}
