//! SPIKE (plan authoring:serde-yaml-replacement) — never lands.
//!
//! A differential harness: every YAML document llmlint reads is run through the
//! crate it uses today (`serde_yaml_ng` 0.10, through llmlint's own
//! `configfs::parse`) and through `serde-saphyr` 1.3 (and, for the schema-less
//! walks, `saphyr` 0.2's node tree), and every difference is printed — in what
//! is read into llmlint's own types, and in the error each gives.
//!
//! Usage: `saphyr-diff <repo-root> <e2e-dump-dir> [--time]` (see `run.sh`).

mod compat;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use llmlint::domain::config::Config;
use llmlint::domain::version::Version;
use serde::de::{self, Deserializer, IgnoredAny, Visitor};
use serde::Deserialize;
use serde_json::Value as Json;
use serde_saphyr::options::DuplicateKeyPolicy;
use serde_saphyr::Options;

// ---------------------------------------------------------------- corpus ----

/// One document and the kind it belongs to.
struct Doc {
    kind: &'static str,
    name: String,
    text: String,
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn find_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            find_named(&p, name, out);
        } else if p.file_name().is_some_and(|n| n == name) {
            out.push(p);
        }
    }
}

fn repo_configs(root: &Path) -> Vec<Doc> {
    let mut paths: Vec<PathBuf> = ["llmlint.yml", "assets/init.llmlint.yml", "assets/config_lint.yml"]
        .iter()
        .map(|p| root.join(p))
        .collect();
    find_named(&root.join("screenshots/fixture"), "llmlint.yml", &mut paths);
    paths
        .into_iter()
        .map(|p| Doc {
            kind: "config:repo",
            name: p.strip_prefix(root).unwrap().display().to_string(),
            text: read(&p).unwrap_or_else(|| panic!("missing {}", p.display())),
        })
        .collect()
}

fn e2e_configs(dump: &Path) -> Vec<Doc> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dump)
        .unwrap_or_else(|e| panic!("e2e dump {}: {e}", dump.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| Doc {
            kind: "config:e2e",
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            text: read(&p).unwrap(),
        })
        .collect()
}

/// The fleet's real configs — read only.
fn fleet_configs() -> Vec<Doc> {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let mut paths = vec![
        home.join("projects/ai-orchestrator/llmlint.yml"),
        home.join("projects/ai-orchestrator/config/plan-checklist.llmlint.yml"),
    ];
    if let Ok(rd) = fs::read_dir(home.join(".ai-orchestrator/repos")) {
        let mut repos: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("nickderobertis__"))
            })
            .collect();
        repos.sort();
        paths.extend(repos.into_iter().map(|r| r.join("llmlint.yml")));
    }
    paths
        .into_iter()
        .filter_map(|p| {
            let text = read(&p)?;
            Some(Doc {
                kind: "config:fleet",
                name: p.strip_prefix(&home).unwrap().display().to_string(),
                text,
            })
        })
        .collect()
}

/// Hand-written cases for each feature the task names (anchors, merge keys,
/// stash keys, the include refusal, both relevance forms, version text) and the
/// error classes (unknown keys, malformed YAML), plus the YAML 1.1 scalars the
/// two crates resolve differently.
fn synthetic_configs() -> Vec<Doc> {
    let rule = |extra: &str| format!("rules:\n  - name: r\n    description: d\n{extra}");
    let mut cases: Vec<(&str, String)> = vec![
        // anchors and merge keys
        ("anchor-alias-scalar", "x-d: &d \"shared text\"\nrules:\n  - name: r\n    description: *d\n".into()),
        ("anchor-files-reuse", "x-f: &f {include: [\"src/**\"], exclude: [\"gen/**\"]}\nrules:\n  - {name: a, description: d, files: *f}\n  - {name: b, description: d, files: *f}\n".into()),
        ("merge-basic", "x-base: &base {description: base, judges: 3}\nrules:\n  - <<: *base\n    name: r\n".into()),
        ("merge-local-wins", "x-base: &base {description: base, judges: 3}\nrules:\n  - <<: *base\n    name: r\n    judges: 5\n".into()),
        ("merge-local-before-merge-key", "x-base: &base {description: base, judges: 3}\nrules:\n  - name: r\n    judges: 5\n    <<: *base\n".into()),
        ("merge-list-first-wins", "x-a: &a {description: from_a}\nx-b: &b {description: from_b, judges: 2}\nrules:\n  - <<: [*a, *b]\n    name: r\n".into()),
        ("merge-nested-merge", "x-a: &a {description: from_a}\nx-b: &b\n  <<: *a\n  judges: 2\nrules:\n  - <<: *b\n    name: r\n".into()),
        ("merge-top-level", "x-defaults: &defaults\n  rationales: true\n  diff_base: main\n<<: *defaults\nrules: []\n".into()),
        ("merge-nested-top-level", "x-a: &a {rationales: true}\nx-b: &b\n  <<: *a\n  diff_base: main\n<<: *b\nrules: []\n".into()),
        ("merge-nested-unused", "x-a: &a {description: d}\nx-b: &b\n  <<: *a\n  judges: 2\nrules:\n  - {name: r, description: d}\n".into()),
        ("merge-nested-source-also-aliased", "x-a: &a {include: [\"src/**\"]}\nx-f: &f\n  <<: *a\n  exclude: [\"gen/**\"]\nx-g:\n  <<: *f\nfiles: *f\nrules: []\n".into()),
        ("merge-nested-inline", "rules:\n  - <<: {<<: {description: d}, judges: 2}\n    name: r\n".into()),
        ("merge-double-quoted-key", "rules:\n  - \"<<\": {description: d}\n    name: r\n".into()),
        ("merge-empty-value", "rules:\n  - <<:\n    name: r\n    description: d\n".into()),
        ("merge-inline-mapping", "rules:\n  - <<: {description: inline}\n    name: r\n".into()),
        ("merge-scalar-error", "x-s: &s hello\nrules:\n  - <<: *s\n    name: r\n    description: d\n".into()),
        ("merge-seq-of-scalar-error", "rules:\n  - <<: [1, 2]\n    name: r\n    description: d\n".into()),
        ("merge-brings-unknown-key", "x-base: &base {description: d, bogus: 1}\nrules:\n  - <<: *base\n    name: r\n".into()),
        ("merge-null", "rules:\n  - <<: ~\n    name: r\n    description: d\n".into()),
        ("merge-quoted-key", "rules:\n  - '<<': {description: d}\n    name: r\n".into()),
        ("merge-explicit-tag", "x-base: &base {description: d}\nrules:\n  - !!merge <<: *base\n    name: r\n".into()),
        ("alias-undefined", "rules:\n  - name: r\n    description: *nope\n".into()),
        // stash keys and the include refusal
        ("stash-keys", "x-notes: [1, 2, {a: b}]\nx-anything: {deep: [true, ~]}\nrules: []\n".into()),
        ("unknown-top-level-key", "frobnicate: 1\nrules: []\n".into()),
        ("include-refused", "include: [\"other.yml\"]\nrules: []\n".into()),
        ("include-null-refused", "include:\nrules: []\n".into()),
        ("include-via-merge", "x-i: &i {include: [a.yml]}\n<<: *i\nrules: []\n".into()),
        ("files-include-not-refused", "files:\n  include: [\"src/**\"]\nrules: []\n".into()),
        // relevance
        ("relevance-true", rule("    relevance: true\n")),
        ("relevance-false", rule("    relevance: false\n")),
        ("relevance-string", rule("    relevance: when the change touches SQL\n")),
        ("relevance-quoted-true", rule("    relevance: \"true\"\n")),
        ("relevance-yes", rule("    relevance: yes\n")),
        ("relevance-on", rule("    relevance: on\n")),
        ("relevance-no", rule("    relevance: no\n")),
        ("relevance-True", rule("    relevance: True\n")),
        ("relevance-y", rule("    relevance: y\n")),
        ("relevance-n", rule("    relevance: n\n")),
        ("relevance-off", rule("    relevance: off\n")),
        ("relevance-NO", rule("    relevance: NO\n")),
        ("relevance-quoted-yes", rule("    relevance: \"yes\"\n")),
        ("relevance-number", rule("    relevance: 1\n")),
        ("relevance-null", rule("    relevance: ~\n")),
        ("relevance-list", rule("    relevance: [a]\n")),
        // YAML 1.1 / 1.2 scalars in typed fields
        ("bool-field-yes", rule("    rationale: yes\n")),
        ("bool-field-on", rule("    rationale: on\n")),
        ("bool-field-True", rule("    rationale: True\n")),
        ("bool-field-quoted", rule("    rationale: \"true\"\n")),
        ("string-field-yes", "rules:\n  - name: r\n    description: yes\n".into()),
        ("string-field-number", "rules:\n  - name: r\n    description: 1.10\n".into()),
        ("string-field-int", "rules:\n  - name: r\n    description: 42\n".into()),
        ("string-field-null", "rules:\n  - name: r\n    description: ~\n".into()),
        ("string-field-hex", "rules:\n  - name: r\n    description: 0x1F\n".into()),
        ("string-field-tagged-str", "rules:\n  - name: r\n    description: !!str 1.10\n".into()),
        ("string-field-custom-tag", "rules:\n  - name: r\n    description: !foo bar\n".into()),
        ("name-field-n", "rules:\n  - name: n\n    description: d\n".into()),
        ("stash-key-on", "x-on: on\non: 1\nrules: []\n".into()),
        ("bool-field-quoted-false", rule("    rationale: \"false\"\n")),
        ("bool-field-quoted-yes", rule("    rationale: \"yes\"\n")),
        ("name-field-on", "rules:\n  - name: on\n    description: d\n".into()),
        ("model-number", "agents:\n  default:\n    model: 1.10\nrules: []\n".into()),
        ("int-field-string", rule("    judges: \"3\"\n")),
        ("int-field-float", rule("    judges: 3.0\n")),
        ("int-field-octal-ish", rule("    judges: 010\n")),
        ("int-field-0o", rule("    judges: 0o10\n")),
        ("int-field-negative", rule("    judges: -1\n")),
        ("string-field-underscore", "rules:\n  - name: r\n    description: 1_000\n".into()),
        ("string-field-leading-zero", "rules:\n  - name: r\n    description: 007\n".into()),
        ("int-field-underscore", rule("    judges: 1_0\n")),
        ("stash-underscore-key", "x-1_0: 1\nrules: []\n".into()),
        ("timeout-underscore", "oneharness:\n  timeout: 1_000\nrules: []\n".into()),
        ("plugins-scalar", "plugins: a.yml\nrules: []\n".into()),
        // document shape
        ("empty-document", "".into()),
        ("comment-only", "# nothing here\n".into()),
        ("explicit-null-document", "~\n".into()),
        ("top-level-sequence", "- a\n- b\n".into()),
        ("top-level-scalar", "hello\n".into()),
        ("multi-document", "rules: []\n---\nrules: []\n".into()),
        ("document-start-marker", "---\nrules: []\n".into()),
        ("bom", "\u{feff}rules: []\n".into()),
        ("crlf", "version: 1\r\nrules:\r\n  - name: r\r\n    description: d\r\n".into()),
        ("non-string-key", "1: one\nrules: []\n".into()),
        ("bool-key", "true: one\nrules: []\n".into()),
        ("complex-key", "? [a, b]\n: one\nrules: []\n".into()),
        // errors
        ("unknown-rule-key", "rules:\n  - name: r\n    description: d\n    bogus: 1\n".into()),
        ("unknown-files-key", "files:\n  includes: [\"a\"]\nrules: []\n".into()),
        ("unknown-agent-key", "agents:\n  a:\n    files: [\"x\"]\nrules: []\n".into()),
        ("missing-name", "rules:\n  - description: d\n".into()),
        ("wrong-type-rules", "rules: 3\n".into()),
        ("duplicate-top-key", "rules: []\nrules: []\n".into()),
        ("duplicate-rule-key", "rules:\n  - name: r\n    name: s\n    description: d\n".into()),
        ("duplicate-stash-key", "x-a: 1\nx-a: 2\nrules: []\n".into()),
        ("malformed-colons", "version: : :\n  - oops\n".into()),
        ("malformed-unclosed-flow", "rules: [\n".into()),
        ("malformed-unterminated-quote", "rules:\n  - name: \"r\n".into()),
        ("malformed-bad-indent", "rules:\n  - name: r\n   description: d\n".into()),
        ("malformed-tab-indent", "rules:\n\t- name: r\n".into()),
        ("malformed-not-yaml", "{{ template }}\n".into()),
        ("malformed-mixed-block", "a: b\n- c\n".into()),
        ("deep-nesting", format!("x-deep: {}{}\nrules: []\n", "[".repeat(200), "]".repeat(200))),
    ];
    // `version:` text: number-or-string (version.rs `scalar_text`).
    for v in [
        "1", "1.2", "1.10", "\"1.10\"", "'1.10'", "1.0", "1.2.3", "\"1.2.3\"", "01", "010", "0x10", "1e2",
        "1_0", "+1", ".5", "true", "~", "[1, 2]", "{a: 1}", "\"\"", "1.2.3.4", "!!str 1.10",
    ] {
        cases.push(("version", format!("version: {v}\nrules: []\n")));
    }
    cases
        .into_iter()
        .map(|(name, text)| Doc {
            kind: "config:synthetic",
            name: if name == "version" {
                format!("version:{}", text.lines().next().unwrap().trim_start_matches("version: "))
            } else {
                name.to_string()
            },
            text,
        })
        .collect()
}

// ------------------------------------------------------------ the readers ----

/// Today: llmlint's own `configfs::parse` (serde_yaml_ng `Value` → `apply_merge`
/// → the `include` refusal → `from_value`).
fn current_parse(text: &str) -> Result<Config, String> {
    llmlint::io::configfs::parse(text, "doc").map_err(|e| e.to_string())
}

/// True when the field is present at all, whatever its value (`include:` with a
/// null value is refused today, since `contains_key` sees it).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d).map(|_| true)
}

#[derive(Deserialize)]
struct IncludeProbe {
    #[serde(default, deserialize_with = "present")]
    include: bool,
}

fn cfg_err(message: impl std::fmt::Display) -> String {
    llmlint::errors::Error::ConfigParse { path: "doc".into(), message: message.to_string() }.to_string()
}

const INCLUDE_MSG: &str = "top-level `include` was renamed to `plugins` (it pulls in other \
                           configs; `files.include` is the file glob). Rename the key to `plugins`.";

/// Candidate A: serde-saphyr straight into `Config`, after a probe for the
/// top-level `include` key (a probe that fails — e.g. a non-mapping document —
/// leaves the error to the typed read).
fn saphyr_typed(text: &str, opts: &Options) -> Result<Config, String> {
    if let Ok(p) = serde_saphyr::from_str_with_options::<IncludeProbe>(text, opts.clone()) {
        if p.include {
            return Err(cfg_err(INCLUDE_MSG));
        }
    }
    serde_saphyr::from_str_with_options::<Config>(text, opts.clone())
        .map_err(cfg_err)
}

/// Candidate B: serde-saphyr into `serde_json::Value` (the closest stand-in for
/// today's `Value` step), the refusal on the map, then `from_value`.
fn saphyr_via_json(text: &str, opts: &Options) -> Result<Config, String> {
    let v: Json = serde_saphyr::from_str_with_options(text, opts.clone())
        .map_err(cfg_err)?;
    if v.as_object().is_some_and(|m| m.contains_key("include")) {
        return Err(cfg_err(INCLUDE_MSG));
    }
    serde_json::from_value(v).map_err(cfg_err)
}

/// The candidate compatibility pre-passes (compat.rs), in order, then the
/// typed read. `emulate` picks the nested-merge pre-pass: quote the inner `<<`
/// (today's literal-key result) rather than refuse.
fn compat_prepass(text: &str, emulate: bool) -> Result<String, String> {
    let t = compat::unquote_merge_keys(text);
    compat::refuse_null_merge(&t).map_err(cfg_err)?;
    let t = if emulate {
        compat::quote_nested_merge_keys(&t)
    } else {
        compat::refuse_nested_merge(&t).map_err(cfg_err)?;
        t
    };
    Ok(compat::quote_yaml11_numbers(&t))
}
fn compat_refuse(text: &str, opts: &Options) -> Result<Config, String> {
    saphyr_typed(&compat_prepass(text, false)?, opts)
}
fn compat_emulate(text: &str, opts: &Options) -> Result<Config, String> {
    saphyr_typed(&compat_prepass(text, true)?, opts)
}

/// How many documents of a kind each pre-pass changes (rewrites) or refuses.
fn report_prepass_reach(kind: &str, docs: &[&Doc]) {
    let (mut q, mut u, mut n, mut e, mut r) = (0, 0, 0, 0, 0);
    let mut touched = Vec::new();
    for d in docs {
        let t = &d.text;
        let mut hit = |c: &mut usize, b: bool, what: &str| {
            if b {
                *c += 1;
                touched.push(format!("{what}:{}", d.name));
            }
        };
        hit(&mut q, compat::quote_yaml11_numbers(t) != *t, "quote_yaml11_numbers");
        hit(&mut u, compat::unquote_merge_keys(t) != *t, "unquote_merge_keys");
        hit(&mut n, compat::refuse_null_merge(t).is_err(), "refuse_null_merge");
        hit(&mut e, compat::quote_nested_merge_keys(t) != *t, "quote_nested_merge_keys");
        hit(&mut r, compat::refuse_nested_merge(t).is_err(), "refuse_nested_merge");
    }
    println!(
        "PREPASS [{kind}] {} docs: quote_yaml11_numbers rewrites {q}, unquote_merge_keys rewrites {u}, refuse_null_merge refuses {n}, quote_nested_merge_keys rewrites {e}, refuse_nested_merge refuses {r}",
        docs.len()
    );
    if kind != "config:synthetic" {
        for t in touched {
            println!("PREPASS-HIT [{kind}] {t}");
        }
    }
}

/// The plugin `VersionProbe` (src/io/plugins.rs), restated: it is private.
#[derive(Deserialize, Debug, PartialEq)]
struct VersionProbe {
    #[serde(default)]
    version: Option<Version>,
}

/// What a deserializer hands a visitor for a scalar, for `version:` text.
#[derive(Debug)]
struct Trace(String);

struct TraceVisitor;
impl<'de> Visitor<'de> for TraceVisitor {
    type Value = Trace;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("anything")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Trace, E> {
        Ok(Trace(format!("bool({v})")))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Trace, E> {
        Ok(Trace(format!("i64({v})")))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Trace, E> {
        Ok(Trace(format!("u64({v})")))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Trace, E> {
        Ok(Trace(format!("f64({v})")))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Trace, E> {
        Ok(Trace(format!("str({v:?})")))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Trace, E> {
        Ok(Trace("unit".into()))
    }
    fn visit_none<E: de::Error>(self) -> Result<Trace, E> {
        Ok(Trace("none".into()))
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Trace, D::Error> {
        d.deserialize_any(TraceVisitor).map(|t| Trace(format!("some({})", t.0)))
    }
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut a: A) -> Result<Trace, A::Error> {
        while a.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Trace("seq".into()))
    }
    fn visit_map<A: de::MapAccess<'de>>(self, mut a: A) -> Result<Trace, A::Error> {
        while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Trace("map".into()))
    }
}
struct AnyTrace(Trace);
impl<'de> Deserialize<'de> for AnyTrace {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(TraceVisitor).map(AnyTrace)
    }
}
struct StrTrace(Trace);
impl<'de> Deserialize<'de> for StrTrace {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(TraceVisitor).map(StrTrace)
    }
}
#[derive(Deserialize)]
struct VAny {
    version: AnyTrace,
}
#[derive(Deserialize)]
struct VStr {
    version: StrTrace,
}

/// Candidate for version.rs `scalar_text` with no `serde_yaml_ng::Value`: the
/// same `deserialize_any`, with numbers formatted the way serde_yaml_ng's
/// `Number` displays them (ryu for finite floats, `.inf`/`.nan` otherwise).
struct CandidateText(String);
struct CandidateVisitor;
impl<'de> Visitor<'de> for CandidateVisitor {
    type Value = CandidateText;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a number or string like 1, 1.2, or \"1.2.3\"")
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<CandidateText, E> {
        Ok(CandidateText(v.to_string()))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<CandidateText, E> {
        Ok(CandidateText(v.to_string()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<CandidateText, E> {
        Ok(CandidateText(match serde_json::Number::from_f64(v) {
            Some(n) => n.to_string(),
            None if v.is_nan() => ".nan".into(),
            None if v > 0.0 => ".inf".into(),
            None => "-.inf".into(),
        }))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<CandidateText, E> {
        Ok(CandidateText(v.to_string()))
    }
}
#[derive(Deserialize)]
struct CandidateProbe {
    #[serde(default, deserialize_with = "candidate_version")]
    version: Option<String>,
}
impl<'de> Deserialize<'de> for CandidateText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(CandidateVisitor)
    }
}
/// As `Option<Version>` reads it: null is `None` before the visitor runs.
fn candidate_version<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Option::<CandidateText>::deserialize(d)? {
        None => Ok(None),
        Some(t) => Version::parse(&t.0).map(|v| Some(v.to_string())).map_err(de::Error::custom),
    }
}

/// Candidate code change for the `Option<bool>` / integer fields: read through
/// `deserialize_any`, so a quoted scalar arrives as a string and is refused,
/// as serde_yaml_ng's `Value` → `from_value` path refuses it today.
struct StrictBool(bool);
struct StrictBoolVisitor;
impl<'de> Visitor<'de> for StrictBoolVisitor {
    type Value = StrictBool;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a boolean")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<StrictBool, E> {
        Ok(StrictBool(v))
    }
}
impl<'de> Deserialize<'de> for StrictBool {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictBoolVisitor)
    }
}
struct StrictU32(u32);
struct StrictU32Visitor;
impl<'de> Visitor<'de> for StrictU32Visitor {
    type Value = StrictU32;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("u32")
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<StrictU32, E> {
        u32::try_from(v).map(StrictU32).map_err(|_| E::invalid_value(de::Unexpected::Unsigned(v), &self))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<StrictU32, E> {
        u32::try_from(v).map(StrictU32).map_err(|_| E::invalid_value(de::Unexpected::Signed(v), &self))
    }
}
impl<'de> Deserialize<'de> for StrictU32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictU32Visitor)
    }
}
#[derive(Deserialize)]
struct PlainFields {
    #[serde(default)]
    b: Option<bool>,
    #[serde(default)]
    n: Option<u32>,
}
#[derive(Deserialize)]
struct StrictFields {
    #[serde(default)]
    b: Option<StrictBool>,
    #[serde(default)]
    n: Option<StrictU32>,
}

fn report_code_changes() {
    println!("\n=== candidate code change: Option<bool>/u32 fields read through deserialize_any ===");
    let fields = |p: PlainFields| format!("b={:?} n={:?}", p.b, p.n);
    let strict = |p: StrictFields| format!("b={:?} n={:?}", p.b.map(|x| x.0), p.n.map(|x| x.0));
    for case in [
        "b: true", "b: True", "b: \"true\"", "b: \"yes\"", "b: yes", "b: on", "b: n", "b: ~",
        "n: 3", "n: \"3\"", "n: 3.0", "n: 010", "n: 0o10", "n: 0x10", "n: 1_000", "n: 1_0", "n: -1", "n: ~",
    ] {
        let today = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(case)
            .map_err(|e| e.to_string())
            .and_then(|v| serde_yaml_ng::from_value::<PlainFields>(v).map_err(|e| e.to_string()))
            .map(fields);
        let mut o = recommended();
        o.with_snippet = false;
        let plain = serde_saphyr::from_str_with_options::<PlainFields>(case, o.clone()).map(fields).map_err(|e| e.to_string());
        let changed = serde_saphyr::from_str_with_options::<StrictFields>(case, o.clone()).map(strict).map_err(|e| e.to_string());
        let quoted = compat::quote_yaml11_numbers(case);
        let both = serde_saphyr::from_str_with_options::<StrictFields>(&quoted, o).map(strict).map_err(|e| e.to_string());
        let same = |r: &Result<String, String>| match (&today, r) {
            (Ok(a), Ok(b)) => a == b,
            (Err(_), Err(_)) => true,
            _ => false,
        };
        println!(
            "CODE {:<12} today={today:?}\n      serde-saphyr recommended, plain fields: {} {plain:?}\n      serde-saphyr recommended, deserialize_any fields: {} {changed:?}\n      quote_yaml11_numbers pre-pass + deserialize_any fields: {} {both:?}",
            case,
            if same(&plain) { "SAME" } else { "DIFF" },
            if same(&changed) { "SAME" } else { "DIFF" },
            if same(&both) { "SAME" } else { "DIFF" },
        );
    }
}

// ------------------------------------------------------------- diffing ----

/// Every path at which two JSON trees differ, as `path: before -> after`.
fn json_diff(path: &str, a: &Json, b: &Json, out: &mut Vec<String>) {
    match (a, b) {
        (Json::Object(x), Json::Object(y)) => {
            let xk: Vec<_> = x.keys().collect();
            let yk: Vec<_> = y.keys().collect();
            if xk != yk && xk.iter().collect::<std::collections::BTreeSet<_>>() == yk.iter().collect() {
                out.push(format!("{path}: key order {xk:?} -> {yk:?}"));
            }
            for (k, v) in x {
                let p = format!("{path}.{k}");
                match y.get(k) {
                    Some(w) => json_diff(&p, v, w, out),
                    None => out.push(format!("{p}: {} -> <absent>", short(v))),
                }
            }
            for (k, w) in y {
                if !x.contains_key(k) {
                    out.push(format!("{path}.{k}: <absent> -> {}", short(w)));
                }
            }
        }
        (Json::Array(x), Json::Array(y)) if x.len() == y.len() => {
            for (i, (v, w)) in x.iter().zip(y).enumerate() {
                json_diff(&format!("{path}[{i}]"), v, w, out);
            }
        }
        _ if a == b => {}
        _ => out.push(format!("{path}: {} -> {}", short(a), short(b))),
    }
}

fn short(v: &Json) -> String {
    let s = v.to_string();
    if s.chars().count() > 160 {
        format!("{}…", s.chars().take(160).collect::<String>())
    } else {
        s
    }
}

fn cfg_json(c: &Config) -> Json {
    serde_json::to_value(c).unwrap()
}

/// Compare two read results; `None` when they agree.
fn compare(before: &Result<Config, String>, after: &Result<Config, String>) -> Option<String> {
    match (before, after) {
        (Ok(a), Ok(b)) if a == b => None,
        (Ok(a), Ok(b)) => {
            let mut d = Vec::new();
            json_diff("$", &cfg_json(a), &cfg_json(b), &mut d);
            if d.is_empty() {
                d.push("(PartialEq differs; JSON identical)".into());
            }
            Some(format!("both read, values differ:\n        {}", d.join("\n        ")))
        }
        (Ok(_), Err(e)) => Some(format!("today reads it; serde-saphyr errors:\n        after:  {}", one_line(e))),
        (Err(e), Ok(b)) => Some(format!(
            "today errors; serde-saphyr reads it:\n        before: {}\n        after:  {}",
            one_line(e),
            short(&cfg_json(b))
        )),
        (Err(_), Err(_)) => None, // both refuse: error *text* is reported separately
    }
}

fn one_line(e: &str) -> String {
    e.replace('\n', "⏎ ")
}

/// The option set this spike recommends for the port (see the report).
fn recommended() -> Options {
    serde_saphyr::options! { strict_booleans: true, no_schema: true }
}

/// The option sets tried, by name, when the default disagrees with today.
fn option_variants() -> Vec<(&'static str, Options)> {
    vec![
        ("strict_booleans", serde_saphyr::options! { strict_booleans: true }),
        ("no_schema", serde_saphyr::options! { no_schema: true }),
        ("legacy_octal_numbers", serde_saphyr::options! { legacy_octal_numbers: true }),
        ("duplicate_keys=LastWins", serde_saphyr::options! { duplicate_keys: DuplicateKeyPolicy::LastWins }),
        ("duplicate_keys=FirstWins", serde_saphyr::options! { duplicate_keys: DuplicateKeyPolicy::FirstWins }),
        ("strict_booleans+no_schema", recommended()),
    ]
}

// ---------------------------------------------------------- the reports ----

#[derive(Default)]
struct Tally {
    docs: usize,
    agree_ok: usize,
    agree_err: usize,
    differ: usize,
}

fn report_configs(kind: &str, docs: &[&Doc], pass: &str, dflt: Options) {
    println!("\n=== {kind} — configfs::parse vs serde-saphyr, {pass} options ({} documents) ===", docs.len());
    let variants = option_variants();
    let (mut typed, mut via, mut cref, mut cemu) = (Tally::default(), Tally::default(), Tally::default(), Tally::default());
    let mut errors: Vec<String> = Vec::new();
    for d in docs {
        let before = current_parse(&d.text);
        for (label, f, tally) in [
            ("typed", saphyr_typed as fn(&str, &Options) -> Result<Config, String>, &mut typed),
            ("via-json", saphyr_via_json, &mut via),
            ("compat-refuse", compat_refuse, &mut cref),
            ("compat-emulate", compat_emulate, &mut cemu),
        ] {
            tally.docs += 1;
            let after = f(&d.text, &dflt);
            match compare(&before, &after) {
                None if before.is_ok() => tally.agree_ok += 1,
                None => {
                    tally.agree_err += 1;
                    if label == "typed" && pass == "default" {
                        let mut plain = dflt.clone();
                        plain.with_snippet = false;
                        errors.push(format!(
                            "  {}\n      before: {}\n      after:  {}\n      after (with_snippet=false): {}",
                            d.name,
                            one_line(before.as_ref().unwrap_err()),
                            one_line(after.as_ref().unwrap_err()),
                            one_line(&f(&d.text, &plain).unwrap_err())
                        ));
                    }
                }
                Some(diff) => {
                    tally.differ += 1;
                    let fixes: Vec<&str> = variants
                        .iter()
                        .filter(|(_, o)| {
                            let a = f(&d.text, o);
                            compare(&before, &a).is_none() && before.is_ok() == a.is_ok()
                        })
                        .map(|(n, _)| *n)
                        .collect();
                    println!(
                        "DIFF [{kind}] [{pass}] [{label}] {}\n      input: {}\n      {diff}\n      reproduced by option: {}",
                        d.name,
                        short(&Json::String(d.text.clone())),
                        if fixes.is_empty() { "none".to_string() } else { fixes.join(", ") }
                    );
                }
            }
        }
    }
    for (label, t) in [("typed", &typed), ("via-json", &via), ("compat-refuse", &cref), ("compat-emulate", &cemu)] {
        println!(
            "SUMMARY [{kind}] [{pass}] [{label}] {} docs: {} read identically, {} refused by both, {} differ",
            t.docs, t.agree_ok, t.agree_err, t.differ
        );
    }
    if !errors.is_empty() {
        println!("ERRORS [{kind}] [{pass}] refused by both — today's message vs serde-saphyr's (typed):");
        for e in errors {
            println!("{e}");
        }
    }
}

type ProbeRead = Result<Option<String>, String>;

fn report_version_probe(docs: &[&Doc]) {
    let readers: Vec<(&str, Box<dyn Fn(&str) -> ProbeRead>)> = vec![
        ("serde-saphyr default", Box::new(|t: &str| {
            serde_saphyr::from_str::<VersionProbe>(t)
                .map(|p| p.version.map(|v| v.to_string()))
                .map_err(|e| e.to_string())
        })),
        ("serde-saphyr recommended", Box::new(|t: &str| {
            serde_saphyr::from_str_with_options::<VersionProbe>(t, recommended())
                .map(|p| p.version.map(|v| v.to_string()))
                .map_err(|e| e.to_string())
        })),
        ("serde-saphyr recommended + duplicate_keys=LastWins", Box::new(|t: &str| {
            let mut o = recommended();
            o.duplicate_keys = DuplicateKeyPolicy::LastWins;
            serde_saphyr::from_str_with_options::<VersionProbe>(t, o)
                .map(|p| p.version.map(|v| v.to_string()))
                .map_err(|e| e.to_string())
        })),
        ("candidate visitor, serde-saphyr recommended", Box::new(|t: &str| {
            serde_saphyr::from_str_with_options::<CandidateProbe>(t, recommended())
                .map(|p| p.version)
                .map_err(|e| e.to_string())
        })),
        ("candidate visitor + quote_yaml11_numbers pre-pass, serde-saphyr recommended", Box::new(|t: &str| {
            serde_saphyr::from_str_with_options::<CandidateProbe>(&compat::quote_yaml11_numbers(t), recommended())
                .map(|p| p.version)
                .map_err(|e| e.to_string())
        })),
    ];
    for (label, read) in &readers {
        println!(
            "\n=== plugin VersionProbe — serde_yaml_ng::from_str vs {label} ({} documents) ===",
            docs.len()
        );
        let (mut same, mut differ) = (0, 0);
        for d in docs {
            let a: ProbeRead = serde_yaml_ng::from_str::<VersionProbe>(&d.text)
                .map(|p| p.version.map(|v| v.to_string()))
                .map_err(|e| e.to_string());
            let b = read(&d.text);
            let agree = match (&a, &b) {
                (Ok(x), Ok(y)) => x == y,
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if agree {
                same += 1;
                if let (Err(x), Err(y)) = (&a, &b) {
                    if d.kind != "config:e2e" && *label == "serde-saphyr default" {
                        println!("ERR  [probe] {}\n      before: {}\n      after:  {}", d.name, one_line(x), one_line(y));
                    }
                }
            } else {
                differ += 1;
                println!(
                    "DIFF [probe] [{label}] [{}] {}\n      before: {:?}\n      after:  {:?}",
                    d.kind,
                    d.name,
                    a.as_ref().map_err(|e| one_line(e)),
                    b.as_ref().map_err(|e| one_line(e)),
                );
            }
        }
        println!("SUMMARY [probe] [{label}] {} docs: {same} agree, {differ} differ", docs.len());
    }
}

/// What each crate hands `version.rs`'s `scalar_text` (it asks for
/// `deserialize_any`, via a `Value`) and what a string-first read would get.
fn report_version_scalars() {
    println!("\n=== version text — the scalar each crate hands version.rs ===");
    for v in [
        "1", "1.2", "1.10", "\"1.10\"", "1.0", "1.2.3", "01", "1e2", "true", "~", ".inf", "-1",
        "[1, 2]", "1_0", "0x10",
    ] {
        let text = format!("version: {v}\n");
        let ng_any = serde_yaml_ng::from_str::<VAny>(&text).map(|x| x.version.0 .0).map_err(|e| e.to_string());
        let sa_any = serde_saphyr::from_str::<VAny>(&text).map(|x| x.version.0 .0).map_err(|e| one_line(&e.to_string()));
        let ng_str = serde_yaml_ng::from_str::<VStr>(&text).map(|x| x.version.0 .0).map_err(|e| e.to_string());
        let sa_str = serde_saphyr::from_str::<VStr>(&text).map(|x| x.version.0 .0).map_err(|e| one_line(&e.to_string()));
        let ng_ver = current_parse(&text).map(|c| c.version.map(|v| v.to_string()));
        let sa_ver = saphyr_typed(&text, &Options::default()).map(|c| c.version.map(|v| v.to_string()));
        let sa_json = saphyr_via_json(&text, &Options::default()).map(|c| c.version.map(|v| v.to_string()));
        let cand = serde_saphyr::from_str_with_options::<CandidateProbe>(&text, recommended())
            .map(|c| c.version)
            .map_err(|e| {
                let mut o = recommended();
                o.with_snippet = false;
                let _ = e;
                serde_saphyr::from_str_with_options::<CandidateProbe>(&text, o).err().unwrap().to_string()
            });
        let today = ng_ver.as_ref().map(|v| v.clone()).map_err(|e| e.clone());
        let verdict = match (&today, &cand) {
            (Ok(a), Ok(b)) if a == b => "SAME",
            (Err(_), Err(_)) => "SAME(both refuse)",
            _ => "DIFF",
        };
        println!("VERSION-CANDIDATE {verdict} {v:<8} today={today:?} candidate(saphyr+recommended)={cand:?}");
        println!(
            "VERSION {v:<8} deserialize_any: ng={ng_any:?} saphyr={sa_any:?}\n                 deserialize_str: ng={ng_str:?} saphyr={sa_str:?}\n                 Config.version:  today={ng_ver:?} saphyr-typed={sa_ver:?} saphyr-via-json={sa_json:?}"
        );
    }
}

/// serde_yaml_ng's tree as JSON, recording non-string keys and tags.
fn ng_to_json(v: &serde_yaml_ng::Value, notes: &mut Vec<String>, path: &str) -> Json {
    use serde_yaml_ng::Value as V;
    match v {
        V::Null => Json::Null,
        V::Bool(b) => Json::Bool(*b),
        V::Number(n) => serde_json::to_value(n).unwrap(),
        V::String(s) => Json::String(s.clone()),
        V::Sequence(s) => Json::Array(
            s.iter().enumerate().map(|(i, x)| ng_to_json(x, notes, &format!("{path}[{i}]"))).collect(),
        ),
        V::Mapping(m) => {
            let mut o = serde_json::Map::new();
            for (k, x) in m {
                let key = match k {
                    V::String(s) => s.clone(),
                    other => {
                        let k = serde_yaml_ng::to_string(other).unwrap().trim().to_string();
                        notes.push(format!("{path}: non-string key {k}"));
                        k
                    }
                };
                let p = format!("{path}.{key}");
                o.insert(key, ng_to_json(x, notes, &p));
            }
            Json::Object(o)
        }
        V::Tagged(t) => {
            notes.push(format!("{path}: tag {}", t.tag));
            ng_to_json(&t.value, notes, path)
        }
    }
}

/// saphyr's node tree as JSON, recording non-string keys, tags, aliases.
fn saphyr_to_json(v: &saphyr::Yaml, notes: &mut Vec<String>, path: &str) -> Json {
    use saphyr::{Scalar, Yaml as Y};
    let scalar = |s: &Scalar| match s {
        Scalar::Null => Json::Null,
        Scalar::Boolean(b) => Json::Bool(*b),
        Scalar::Integer(i) => Json::from(*i),
        Scalar::FloatingPoint(f) => serde_json::Number::from_f64(f.into_inner()).map(Json::Number).unwrap_or(Json::Null),
        Scalar::String(s) => Json::String(s.to_string()),
    };
    match v {
        Y::Value(s) => scalar(s),
        Y::Representation(s, _, _) => {
            notes.push(format!("{path}: unresolved representation {s:?}"));
            Json::String(s.to_string())
        }
        Y::Sequence(s) => Json::Array(
            s.iter().enumerate().map(|(i, x)| saphyr_to_json(x, notes, &format!("{path}[{i}]"))).collect(),
        ),
        Y::Mapping(m) => {
            let mut o = serde_json::Map::new();
            for (k, x) in m {
                let key = match k {
                    Y::Value(Scalar::String(s)) => s.to_string(),
                    Y::Value(other) => {
                        let k = scalar(other).to_string();
                        notes.push(format!("{path}: non-string key {k}"));
                        k
                    }
                    other => {
                        notes.push(format!("{path}: complex key {other:?}"));
                        format!("{other:?}")
                    }
                };
                let p = format!("{path}.{key}");
                o.insert(key, saphyr_to_json(x, notes, &p));
            }
            Json::Object(o)
        }
        Y::Tagged(t, inner) => {
            notes.push(format!("{path}: tag {t}"));
            saphyr_to_json(inner, notes, path)
        }
        Y::Alias(n) => {
            notes.push(format!("{path}: unresolved alias {n}"));
            Json::Null
        }
        Y::BadValue => {
            notes.push(format!("{path}: bad value"));
            Json::Null
        }
    }
}

fn top_keys(v: &Json) -> String {
    v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default()
}

fn report_workflows(root: &Path) {
    let dir = root.join(".github/workflows");
    let mut paths: Vec<_> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    paths.sort();
    println!("\n=== workflows — schema-less walk (doc[\"on\"], jobs, needs, matrix) ({} documents) ===", paths.len());
    let strict = serde_saphyr::options! { strict_booleans: true };
    let mut differ = 0;
    for p in &paths {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(p).unwrap();
        let mut ng_notes = Vec::new();
        let ng: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).unwrap();
        let ng = ng_to_json(&ng, &mut ng_notes, "$");
        let mut tree_notes = Vec::new();
        let tree = saphyr::Yaml::load_from_str(&text).map(|docs| saphyr_to_json(&docs[0], &mut tree_notes, "$"));
        for (label, after) in [
            ("serde-saphyr→serde_json::Value (default)", serde_saphyr::from_str::<Json>(&text).map_err(|e| one_line(&e.to_string()))),
            ("serde-saphyr→serde_json::Value (strict_booleans)", serde_saphyr::from_str_with_options::<Json>(&text, strict.clone()).map_err(|e| one_line(&e.to_string()))),
            ("saphyr::Yaml tree", tree.clone().map_err(|e| e.to_string())),
        ] {
            match after {
                Err(e) => {
                    differ += 1;
                    println!("DIFF [workflow] {name} [{label}] today reads it; this errors: {e}");
                }
                Ok(a) => {
                    let mut d = Vec::new();
                    json_diff("$", &ng, &a, &mut d);
                    if !d.is_empty() {
                        differ += 1;
                        println!("DIFF [workflow] {name} [{label}] top-level keys today={} after={}", top_keys(&ng), top_keys(&a));
                        for line in d {
                            println!("        {line}");
                        }
                    }
                }
            }
        }
        if !ng_notes.is_empty() || !tree_notes.is_empty() {
            println!("NOTE [workflow] {name}: serde_yaml_ng {ng_notes:?}; saphyr tree {tree_notes:?}");
        }
    }
    println!("SUMMARY [workflow] {} documents × 3 readers: {differ} reader/document pairs differ from today", paths.len());

    // The same walk over hand-written trigger/value shapes, so a clean result
    // above is known not to be vacuous: these are where the readers part ways.
    for case in [
        "on: push\n",
        "on: [push, pull_request]\n",
        "on:\n  pull_request:\n    branches: [main]\n",
        "true: 1\nn: 2\n",
        "a: y\nb: n\nc: yes\nd: off\ne: \"on\"\n",
        "a: 1.10\nb: 010\nc: 0o10\nd: 1_000\n",
        "e: .inf\n",
        "a: !!str 1\nb: !custom x\n",
    ] {
        let mut notes = Vec::new();
        let ng = ng_to_json(&serde_yaml_ng::from_str(case).unwrap(), &mut notes, "$");
        let mut tnotes = Vec::new();
        let tree = saphyr_to_json(&saphyr::Yaml::load_from_str(case).unwrap()[0], &mut tnotes, "$");
        let rows: [(&str, Result<Json, String>); 3] = [
            ("serde-saphyr default", serde_saphyr::from_str::<Json>(case).map_err(|e| one_line(&e.to_string()))),
            ("serde-saphyr strict_booleans", serde_saphyr::from_str_with_options::<Json>(case, strict.clone()).map_err(|e| one_line(&e.to_string()))),
            ("saphyr tree", Ok(tree)),
        ];
        println!("WALK-CASE {:?} today={} notes={notes:?}", case, ng);
        for (label, r) in rows {
            let verdict = match &r {
                Ok(v) if *v == ng => "SAME".to_string(),
                Ok(v) => {
                    let mut d = Vec::new();
                    json_diff("$", &ng, v, &mut d);
                    format!("DIFF {}", d.join("; "))
                }
                Err(e) => format!("ERR {e}"),
            };
            println!("      {label}: {verdict}");
        }
        if !tnotes.is_empty() {
            println!("      saphyr tree notes: {tnotes:?}");
        }
    }

    // ci_gate.rs reads a flow sequence out of ci-gate.sh as `Vec<String>`.
    let gate = fs::read_to_string(root.join(".github/scripts/ci-gate.sh")).unwrap();
    let line = gate
        .lines()
        .find_map(|l| l.strip_prefix("readonly SWEEP_JOBS='"))
        .and_then(|r| r.strip_suffix('\''))
        .unwrap();
    let a = serde_yaml_ng::from_str::<Vec<String>>(line).map_err(|e| e.to_string());
    let b = serde_saphyr::from_str::<Vec<String>>(line).map_err(|e| e.to_string());
    println!(
        "{} [ci-gate SWEEP_JOBS] {line}\n      before: {a:?}\n      after:  {b:?}",
        if a == b { "SAME" } else { "DIFF" }
    );
}

// --------------------------------------------------------------- timing ----

fn median_micros(mut f: impl FnMut()) -> f64 {
    for _ in 0..20 {
        f();
    }
    let mut samples: Vec<f64> = (0..200)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn report_timing(real: &[&Doc], root: &Path) {
    let biggest_cfg = real.iter().max_by_key(|d| d.text.len()).unwrap();
    let mut workflows: Vec<(String, String)> = fs::read_dir(root.join(".github/workflows"))
        .unwrap()
        .flatten()
        .map(|e| (e.path().file_name().unwrap().to_string_lossy().into_owned(), fs::read_to_string(e.path()).unwrap()))
        .collect();
    workflows.sort_by_key(|w| std::cmp::Reverse(w.1.len()));
    let (wname, wtext) = &workflows[0];
    println!("\n=== parse time (median of 200, after 20 warm-up; {}) ===", if cfg!(debug_assertions) { "DEBUG build" } else { "release build" });
    let t = &biggest_cfg.text;
    let a = median_micros(|| {
        current_parse(t).unwrap();
    });
    let b = median_micros(|| {
        saphyr_typed(t, &Options::default()).unwrap();
    });
    let c = median_micros(|| {
        saphyr_via_json(t, &Options::default()).unwrap();
    });
    println!(
        "TIME largest config [{}] {} ({} bytes): configfs::parse {a:.1} µs | serde-saphyr typed (incl. include probe) {b:.1} µs | serde-saphyr via serde_json::Value {c:.1} µs",
        biggest_cfg.kind,
        biggest_cfg.name,
        t.len()
    );
    let pre = median_micros(|| {
        compat_prepass(t, false).unwrap();
    });
    let compat_all = median_micros(|| {
        compat_refuse(t, &recommended()).unwrap();
    });
    println!("TIME   compat pre-passes alone {pre:.1} µs | pre-passes + include probe + serde-saphyr→Config (recommended) {compat_all:.1} µs");
    let probe = median_micros(|| {
        serde_saphyr::from_str::<IncludeProbe>(t).unwrap();
    });
    let typed_only = median_micros(|| {
        serde_saphyr::from_str::<Config>(t).unwrap();
    });
    let typed_rec = median_micros(|| {
        serde_saphyr::from_str_with_options::<Config>(t, recommended()).unwrap();
    });
    let json_only = median_micros(|| {
        serde_saphyr::from_str::<Json>(t).unwrap();
    });
    let ng_value = median_micros(|| {
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t).unwrap();
    });
    let ng_typed = median_micros(|| {
        serde_yaml_ng::from_str::<Config>(t).unwrap();
    });
    let tree = median_micros(|| {
        saphyr::Yaml::load_from_str(t).unwrap();
    });
    println!(
        "TIME   breakdown, same document: include probe alone {probe:.1} µs | serde-saphyr→Config alone {typed_only:.1} µs | serde-saphyr→Config (recommended) {typed_rec:.1} µs | serde-saphyr→serde_json::Value {json_only:.1} µs | serde_yaml_ng→Value {ng_value:.1} µs | serde_yaml_ng→Config direct {ng_typed:.1} µs | saphyr::Yaml tree {tree:.1} µs"
    );
    let a = median_micros(|| {
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(wtext).unwrap();
    });
    let b = median_micros(|| {
        serde_saphyr::from_str::<Json>(wtext).unwrap();
    });
    let c = median_micros(|| {
        saphyr::Yaml::load_from_str(wtext).unwrap();
    });
    println!(
        "TIME largest workflow {wname} ({} bytes): serde_yaml_ng::Value {a:.1} µs | serde-saphyr→serde_json::Value {b:.1} µs | saphyr::Yaml tree {c:.1} µs",
        wtext.len()
    );
}

// ----------------------------------------------------------------- main ----

use saphyr::LoadableYamlNode as _;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).expect("usage: saphyr-diff <repo-root> <e2e-dump-dir> [--time]"));
    let dump = PathBuf::from(args.get(2).expect("usage: saphyr-diff <repo-root> <e2e-dump-dir> [--time]"));
    let time = args.iter().any(|a| a == "--time");

    let repo = repo_configs(&root);
    let e2e = e2e_configs(&dump);
    let fleet = fleet_configs();
    let synth = synthetic_configs();
    let mut counts = String::new();
    for (k, v) in [("config:repo", &repo), ("config:e2e", &e2e), ("config:fleet", &fleet), ("config:synthetic", &synth)] {
        let _ = write!(counts, "{k}={} ", v.len());
    }
    println!(
        "saphyr-diff: serde_yaml_ng 0.10 (today) vs serde-saphyr 1.3.0 / saphyr 0.2.1 — corpus {counts}"
    );

    for docs in [&repo, &e2e, &fleet, &synth] {
        let refs: Vec<&Doc> = docs.iter().collect();
        report_configs(refs[0].kind, &refs, "default", Options::default());
        report_configs(refs[0].kind, &refs, "recommended(strict_booleans+no_schema)", recommended());
        report_prepass_reach(refs[0].kind, &refs);
    }
    let all: Vec<&Doc> = repo.iter().chain(&e2e).chain(&fleet).chain(&synth).collect();
    report_version_probe(&all);
    report_version_scalars();
    report_code_changes();
    report_workflows(&root);

    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &all {
        *by_kind.entry(d.kind).or_default() += 1;
    }
    if time {
        let real: Vec<&Doc> = repo.iter().chain(&fleet).collect();
        report_timing(&real, &root);
    }
}
