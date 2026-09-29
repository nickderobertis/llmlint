//! Caller-supplied run labels: the one grammar for `lint --label`,
//! `LLMLINT_LABELS`, and the `history --label` filter.
//!
//! A label is `KEY=VALUE`, split on the first `=`. The grammar mirrors
//! oneharness's history labels (`oneharness-core`'s `domain::history`
//! `parse_label` / `validate_label`, behind `--history-label` /
//! `ONEHARNESS_HISTORY_LABELS`), so one convention covers the whole stack:
//!
//! - **key:** 1–64 ASCII letters, digits, `.`, `_`, or `-`, beginning with a
//!   letter or digit;
//! - **value:** 1–256 Unicode code points, none of them a control character
//!   (Unicode `Cc`: U+0000–U+001F and U+007F–U+009F).
//!
//! A repeated key takes the later value, and `--label` overlays
//! `LLMLINT_LABELS` key by key. Every entry point parses through
//! [`parse_entry`], so the lint path and the history filter can never disagree
//! about what a label is. Reading the environment is the caller's job; this
//! module only interprets text.

use std::collections::BTreeMap;

use crate::errors::{Error, Result};

/// A validated label set. A `BTreeMap` so keys are always in sorted order — the
/// order the history record, listings, and results pointer all promise.
pub type Labels = BTreeMap<String, String>;

/// The environment variable carrying a run's labels (the lower layer).
pub const ENV_VAR: &str = "LLMLINT_LABELS";

/// The CLI flag carrying labels (the upper layer on `lint`; a filter on
/// `history`).
pub const FLAG: &str = "--label";

/// Longest permitted key, in characters.
pub const KEY_MAX: usize = 64;

/// Longest permitted value, in Unicode code points (not bytes).
pub const VALUE_MAX: usize = 256;

/// Parse one `KEY=VALUE` entry, naming `origin` (`--label` or `LLMLINT_LABELS`)
/// in the error so a caller can tell which input to fix.
pub fn parse_entry(entry: &str, origin: &str) -> Result<(String, String)> {
    parse_label(entry).map_err(|message| Error::Label {
        origin: origin.to_string(),
        entry: entry.to_string(),
        message,
    })
}

/// The label grammar itself, with a plain message for the failure.
fn parse_label(entry: &str) -> std::result::Result<(String, String), String> {
    let Some((key, value)) = entry.split_once('=') else {
        return Err("expected KEY=VALUE".to_string());
    };
    let valid_key = !key.is_empty()
        && key.chars().count() <= KEY_MAX
        && key
            .chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphanumeric() || (i > 0 && matches!(c, '.' | '_' | '-')));
    if !valid_key {
        return Err(format!(
            "key must be 1-{KEY_MAX} ASCII letters, digits, `.`, `_`, or `-`, \
             beginning with a letter or digit"
        ));
    }
    if value.is_empty() {
        return Err("value must not be empty".to_string());
    }
    if value.chars().count() > VALUE_MAX {
        return Err(format!("value exceeds {VALUE_MAX} characters"));
    }
    // `char::is_control` is exactly Unicode `Cc` (C0, DEL, and C1).
    if value.chars().any(char::is_control) {
        return Err("value must not contain control characters".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

/// Parse repeated `--label` entries in order (a later duplicate key wins when
/// they are folded into [`Labels`]).
pub fn parse_flags(entries: &[String]) -> Result<Vec<(String, String)>> {
    entries.iter().map(|e| parse_entry(e, FLAG)).collect()
}

/// Parse an `LLMLINT_LABELS` value: comma-separated entries, each trimmed. An
/// empty (or all-whitespace) value means no labels.
pub fn parse_env(text: &str) -> Result<Vec<(String, String)>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|e| parse_entry(e.trim(), ENV_VAR))
        .collect()
}

/// The effective label set for a run: the environment's labels, overlaid key by
/// key by the `--label` flags (a later duplicate winning within each layer).
pub fn resolve(env: Option<&str>, flags: &[String]) -> Result<Labels> {
    let mut labels = Labels::new();
    if let Some(text) = env {
        labels.extend(parse_env(text)?);
    }
    labels.extend(parse_flags(flags)?);
    Ok(labels)
}

/// Whether `labels` contains every `filter` pair with an equal value.
pub fn matches(labels: &Labels, filter: &[(String, String)]) -> bool {
    filter.iter().all(|(k, v)| labels.get(k) == Some(v))
}

/// Render pairs as `k1=v1, k2=v2` in the given order (a [`Labels`] iterates
/// sorted by key) — the spelling of the results pointer and the human views.
pub fn render<'a>(pairs: impl IntoIterator<Item = (&'a String, &'a String)>) -> String {
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    //! Boundary vectors for the grammar, mirroring oneharness's
    //! `domain::history` label tests (`labels_validate_and_later_values_win`,
    //! `label_lengths_are_bounded_in_characters_not_bytes`) — the convention this
    //! module follows.
    use super::*;

    fn ok(entry: &str) -> (String, String) {
        parse_label(entry).unwrap_or_else(|e| panic!("{entry:?} should parse: {e}"))
    }

    fn err(entry: &str) -> String {
        parse_label(entry).expect_err(&format!("{entry:?} should be rejected"))
    }

    #[test]
    fn splits_on_the_first_equals() {
        assert_eq!(ok("session=abc"), ("session".into(), "abc".into()));
        assert_eq!(ok("url=a=b=c"), ("url".into(), "a=b=c".into()));
    }

    #[test]
    fn key_boundaries() {
        assert!(err("=value").contains("key must be"));
        for bad_start in [".k=v", "_k=v", "-k=v"] {
            assert!(err(bad_start).contains("beginning with a letter or digit"));
        }
        assert!(err("a b=v").contains("key must be"));
        assert!(err("ké=v").contains("key must be"));
        // Allowed punctuation after the first character, and a digit may lead.
        ok("0.a_b-c=v");
        ok(&format!("{}=v", "k".repeat(KEY_MAX)));
        assert!(err(&format!("{}=v", "k".repeat(KEY_MAX + 1))).contains("key must be"));
    }

    #[test]
    fn value_boundaries_count_code_points_not_bytes() {
        assert!(err("k=").contains("must not be empty"));
        // A four-byte astral character counts once.
        let astral = "𝄞".repeat(VALUE_MAX);
        ok(&format!("k={astral}"));
        let over = "x".repeat(VALUE_MAX + 1);
        assert!(err(&format!("k={over}")).contains("exceeds 256"));
    }

    #[test]
    fn value_rejects_c0_and_c1_controls() {
        for c in ['\u{0}', '\n', '\u{1f}', '\u{7f}', '\u{80}', '\u{9f}'] {
            assert!(
                err(&format!("k=a{c}b")).contains("control characters"),
                "{c:?}"
            );
        }
        // Just past each range is fine.
        ok("k=a\u{20}b");
        ok("k=a\u{a0}b");
    }

    #[test]
    fn missing_equals_is_rejected() {
        assert!(err("session").contains("expected KEY=VALUE"));
        assert!(err("").contains("expected KEY=VALUE"));
    }

    #[test]
    fn errors_name_the_entry_and_origin() {
        let e = parse_entry("bad", FLAG).unwrap_err().to_string();
        assert!(e.contains("\"bad\"") && e.contains("--label"), "{e}");
        let e = resolve(Some("ok=1, -x=2"), &[]).unwrap_err().to_string();
        assert!(
            e.contains("\"-x=2\"") && e.contains("LLMLINT_LABELS"),
            "{e}"
        );
    }

    #[test]
    fn env_is_trimmed_comma_separated_and_empty_means_none() {
        assert!(parse_env("").unwrap().is_empty());
        assert!(parse_env("   ").unwrap().is_empty());
        assert_eq!(
            parse_env(" session=abc , turn=3 ").unwrap(),
            vec![
                ("session".into(), "abc".into()),
                ("turn".into(), "3".into())
            ]
        );
        // An empty entry between commas has no `=`.
        assert!(parse_env("a=1,,b=2").is_err());
    }

    #[test]
    fn later_duplicates_win_and_flags_overlay_env() {
        let labels = resolve(
            Some("session=env,turn=1,turn=2"),
            &["judge=j".into(), "session=cli".into(), "judge=k".into()],
        )
        .unwrap();
        let got: Vec<(&str, &str)> = labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(got, vec![("judge", "k"), ("session", "cli"), ("turn", "2")]);
        assert!(resolve(None, &[]).unwrap().is_empty());
    }

    #[test]
    fn matches_requires_every_pair_equal() {
        let labels = resolve(Some("session=abc,turn=1"), &[]).unwrap();
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert!(matches(&labels, &[]));
        assert!(matches(&labels, &[pair("session", "abc")]));
        assert!(matches(
            &labels,
            &[pair("session", "abc"), pair("turn", "1")]
        ));
        assert!(!matches(&labels, &[pair("turn", "2")]));
        assert!(!matches(&labels, &[pair("judge", "abc")]));
    }

    #[test]
    fn render_joins_in_iteration_order() {
        let labels = resolve(None, &["z=1".into(), "a=2".into()]).unwrap();
        assert_eq!(render(&labels), "a=2, z=1");
    }
}
