// llmlint: ignore-file[contracts_have_one_source_or_a_drift_gate] the grammar mirrors oneharness's history labels by a cited convention the plan chose (issue #204), not across a seam: llmlint's labels are validated, stored, and filtered by llmlint alone and never handed to oneharness, so the two drifting would make one tool accept a label the other rejects, never corrupt shared data; a reconciling drift gate is a tracked follow-up
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

use serde::Serialize;

use crate::errors::{Error, Result};

/// The environment variable carrying a run's labels (the lower layer).
pub const ENV_VAR: &str = "LLMLINT_LABELS";

/// The CLI flag carrying labels (the upper layer on `lint`; a filter on
/// `history`).
pub const FLAG: &str = "--label";

/// Longest permitted key, in characters.
pub const KEY_MAX: usize = 64;

/// Longest permitted value, in Unicode code points (not bytes).
pub const VALUE_MAX: usize = 256;

/// One label that satisfies the grammar. Its fields are private and the only
/// constructor is the grammar, so an invalid label cannot exist past a parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    key: String,
    value: String,
}

impl Label {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.key, self.value)
    }
}

/// A set of valid labels, one value per key. Keys iterate (and serialize, as a
/// JSON object) in sorted order — the order the history record, listings, and
/// results pointer all promise. Built only from [`Label`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Labels(BTreeMap<String, String>);

impl Labels {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Add `label`, replacing any earlier value for its key (a later label wins).
    pub fn insert(&mut self, label: Label) {
        self.0.insert(label.key, label.value);
    }

    /// Whether this set contains every `filter` label with an equal value.
    pub fn matches(&self, filter: &[Label]) -> bool {
        filter
            .iter()
            .all(|l| self.0.get(&l.key).is_some_and(|v| *v == l.value))
    }

    /// Rebuild a set read back from storage, keeping only the pairs that still
    /// satisfy the grammar — a hand-edited or foreign record can never smuggle an
    /// invalid label into a filter match or a rendered view.
    pub fn from_stored<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Labels {
        pairs
            .into_iter()
            .filter_map(|(k, v)| validate(k, v).ok())
            .collect()
    }

    /// `k1=v1, k2=v2` in sorted key order — the spelling of the results pointer
    /// and the human views.
    pub fn render(&self) -> String {
        self.0
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl FromIterator<Label> for Labels {
    fn from_iter<I: IntoIterator<Item = Label>>(iter: I) -> Self {
        let mut labels = Labels::default();
        for label in iter {
            labels.insert(label);
        }
        labels
    }
}

/// Parse one `KEY=VALUE` entry, naming `origin` (`--label` or `LLMLINT_LABELS`)
/// in the error so a caller can tell which input to fix.
pub fn parse_entry(entry: &str, origin: &str) -> Result<Label> {
    parse_label(entry).map_err(|message| Error::Label {
        origin: origin.to_string(),
        entry: entry.to_string(),
        message,
    })
}

/// Split an entry on its first `=` and hold both halves to the grammar.
fn parse_label(entry: &str) -> std::result::Result<Label, String> {
    let Some((key, value)) = entry.split_once('=') else {
        return Err("expected KEY=VALUE".to_string());
    };
    validate(key, value)
}

/// The label grammar itself, with a plain message for the failure.
fn validate(key: &str, value: &str) -> std::result::Result<Label, String> {
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
    Ok(Label {
        key: key.to_string(),
        value: value.to_string(),
    })
}

/// Parse repeated `--label` entries in order (a later duplicate key wins when
/// they are folded into [`Labels`]).
pub fn parse_flags(entries: &[String]) -> Result<Vec<Label>> {
    entries.iter().map(|e| parse_entry(e, FLAG)).collect()
}

/// Parse an `LLMLINT_LABELS` value: comma-separated entries, each trimmed. An
/// empty (or all-whitespace) value means no labels.
pub fn parse_env(text: &str) -> Result<Vec<Label>> {
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
    let env = match env {
        Some(text) => parse_env(text)?,
        None => Vec::new(),
    };
    let flags = parse_flags(flags)?;
    Ok(env.into_iter().chain(flags).collect())
}

#[cfg(test)]
mod tests {
    //! Boundary vectors for the grammar, mirroring oneharness's
    //! `domain::history` label tests (`labels_validate_and_later_values_win`,
    //! `label_lengths_are_bounded_in_characters_not_bytes`) — the convention this
    //! module follows.
    use super::*;

    fn ok(entry: &str) -> (String, String) {
        let label = parse_label(entry).unwrap_or_else(|e| panic!("{entry:?} should parse: {e}"));
        (label.key().to_string(), label.value().to_string())
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
        let parsed: Vec<String> = parse_env(" session=abc , turn=3 ")
            .unwrap()
            .iter()
            .map(Label::to_string)
            .collect();
        assert_eq!(parsed, vec!["session=abc", "turn=3"]);
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
        assert_eq!(labels.render(), "judge=k, session=cli, turn=2");
        assert!(resolve(None, &[]).unwrap().is_empty());
    }

    #[test]
    fn matches_requires_every_pair_equal() {
        let labels = resolve(Some("session=abc,turn=1"), &[]).unwrap();
        let filter = |entries: &[&str]| {
            let entries: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
            parse_flags(&entries).unwrap()
        };
        assert!(labels.matches(&[]));
        assert!(labels.matches(&filter(&["session=abc"])));
        assert!(labels.matches(&filter(&["session=abc", "turn=1"])));
        assert!(!labels.matches(&filter(&["turn=2"])));
        assert!(!labels.matches(&filter(&["judge=abc"])));
    }

    #[test]
    fn render_and_serialize_in_sorted_key_order() {
        let labels = resolve(None, &["z=1".into(), "a=2".into()]).unwrap();
        assert_eq!(labels.render(), "a=2, z=1");
        assert_eq!(
            serde_json::to_string(&labels).unwrap(),
            r#"{"a":"2","z":"1"}"#
        );
    }

    #[test]
    fn stored_labels_keep_only_grammatical_pairs() {
        let long = "v".repeat(VALUE_MAX + 1);
        let labels = Labels::from_stored([
            ("session", "abc"),
            ("-bad", "x"),
            ("empty", ""),
            ("ctl", "a\nb"),
            ("long", long.as_str()),
        ]);
        assert_eq!(labels.render(), "session=abc");
    }
}
