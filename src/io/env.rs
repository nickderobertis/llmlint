//! Process-environment overrides for the top-level (session) settings.
//!
//! Precedence for every session setting is uniform:
//!
//! ```text
//! CLI flag  >  LLMLINT_ env var  >  config file  >  built-in default
//! ```
//!
//! This module owns the **env layer** — the one that sits between the CLI and
//! the config file. It folds each set `LLMLINT_*` variable into the already
//! merged [`Config`] (so everything downstream reads a single effective config)
//! and records the override's provenance as `env:<VAR>`, so `llmlint config
//! --sources` and `llmlint where` stay honest about where a value came from.
//! Reading the environment (and, for `LLMLINT_PROMPT_TEMPLATE`, a file) is I/O,
//! so it lives here rather than in the pure config model. Applying it *before*
//! the CLI overlay keeps the CLI winning; applying it *after* the config merge
//! keeps env winning over the file.
//!
//! Env var names are the setting path uppercased with `.` → `_`, prefixed
//! `LLMLINT_` (e.g. `oneharness.model` → `LLMLINT_ONEHARNESS_MODEL`). The
//! **bool grammar** is `1`/`true`/`yes` (on) and `0`/`false`/`no` (off),
//! case-insensitive; anything else is an exit-2 error located to the variable.
//! Structured settings (`version`, `files`) are deliberately config-only.

use crate::domain::config::{Config, Provenance};
use crate::errors::{io_err, Error, Result};

/// The top-level settings that accept an `LLMLINT_` env override, each paired
/// with its variable name. This is the single source of truth for which
/// settings are env-overridable; the `covers_every_overridable_setting` test
/// pins it against [`SETTING_KEYS`](crate::domain::config::SETTING_KEYS) so a new
/// setting can't silently miss env support (or the two lists drift). `version`
/// and `files` are intentionally absent — a published version and a structured
/// include/exclude set are config-only (see issue #152).
pub const ENV_SETTINGS: &[(&str, &str)] = &[
    ("files.include", "LLMLINT_FILES_INCLUDE"),
    ("files.exclude", "LLMLINT_FILES_EXCLUDE"),
    ("oneharness.config", "LLMLINT_ONEHARNESS_CONFIG"),
    ("oneharness.bin", "LLMLINT_ONEHARNESS_BIN"),
    ("oneharness.model", "LLMLINT_ONEHARNESS_MODEL"),
    ("oneharness.timeout", "LLMLINT_ONEHARNESS_TIMEOUT"),
    (
        "oneharness.schema_max_retries",
        "LLMLINT_ONEHARNESS_SCHEMA_MAX_RETRIES",
    ),
    ("prompt_template", "LLMLINT_PROMPT_TEMPLATE"),
    ("rationales", "LLMLINT_RATIONALES"),
    ("diff_base", "LLMLINT_DIFF_BASE"),
    ("history.enabled", "LLMLINT_HISTORY_ENABLED"),
    ("history.max_runs", "LLMLINT_HISTORY_MAX_RUNS"),
    ("history.dir", "LLMLINT_HISTORY_DIR"),
];

/// Settings that are deliberately **not** env-overridable, kept as an explicit
/// list so the drift test can assert the split is intentional rather than an
/// omission. Only `version` remains — a published plugin version is meaningful
/// only in the file itself, never as a per-run env tweak.
#[cfg(test)]
const CONFIG_ONLY_SETTINGS: &[&str] = &["version"];

/// The separator for the list-valued `LLMLINT_FILES_*` env vars: the platform
/// `PATH`-list separator (`:` on Unix, `;` on Windows), the convention users
/// already know for multi-path env vars. Globs are forward-slash relative
/// patterns, so they never contain it (unlike a comma, which brace-expansion
/// `{a,b}` globs do).
#[cfg(windows)]
const LIST_SEP: char = ';';
#[cfg(not(windows))]
const LIST_SEP: char = ':';

/// Apply the `LLMLINT_*` env overrides to `config`, discarding provenance. Used
/// by the `lint`/`lint-config` engine, which reads only the effective config.
pub fn apply_overrides(config: &mut Config) -> Result<()> {
    let mut prov = Provenance::default();
    apply_overrides_prov(config, &mut prov)
}

/// Apply the `LLMLINT_*` env overrides to `config`, recording each override's
/// source in `prov` as `env:<VAR>`. Used by `config`/`where`, whose whole job is
/// to report where a value comes from.
pub fn apply_overrides_prov(config: &mut Config, prov: &mut Provenance) -> Result<()> {
    apply_from(config, prov, non_empty_var)
}

/// The engine, with the environment lookup injected so it is testable without
/// touching (racy, process-global) real env vars. `get` returns the *non-empty*
/// value of a variable, or `None`. `LLMLINT_PROMPT_TEMPLATE` still reads a real
/// file (tests point it at a temp file), keeping the one genuine filesystem
/// dependency explicit.
fn apply_from(
    config: &mut Config,
    prov: &mut Provenance,
    get: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    if let Some(v) = get("LLMLINT_FILES_INCLUDE") {
        // The include set is a *selection*: the env layer replaces the config's
        // globs (env wins over config), mirroring how explicit CLI files replace
        // them. An empty (all-separators) value is rejected rather than silently
        // selecting nothing.
        let globs = parse_list("LLMLINT_FILES_INCLUDE", &v)?;
        config.files.include = globs;
        note(prov, "files.include", "LLMLINT_FILES_INCLUDE");
    }
    if let Some(v) = get("LLMLINT_FILES_EXCLUDE") {
        // The exclude set is a *denylist*: layers accumulate (config ∪ env ∪ CLI)
        // so an env exclude never silently drops a config safety exclude — the same
        // additive, always-wins semantics as the `--exclude` flag.
        let globs = parse_list("LLMLINT_FILES_EXCLUDE", &v)?;
        config.files.exclude.extend(globs);
        note(prov, "files.exclude", "LLMLINT_FILES_EXCLUDE");
    }
    if let Some(v) = get("LLMLINT_ONEHARNESS_CONFIG") {
        // A single path, matching the single-file `--oneharness-config` / config
        // `oneharness.config` semantics (`resolve_oneharness_config` still warns
        // on extras from other layers).
        config.oneharness.config = vec![v];
        note(prov, "oneharness.config", "LLMLINT_ONEHARNESS_CONFIG");
    }
    if let Some(v) = get("LLMLINT_ONEHARNESS_BIN") {
        config.oneharness.bin = Some(v);
        note(prov, "oneharness.bin", "LLMLINT_ONEHARNESS_BIN");
    }
    if let Some(v) = get("LLMLINT_ONEHARNESS_MODEL") {
        config.oneharness.model = Some(v);
        note(prov, "oneharness.model", "LLMLINT_ONEHARNESS_MODEL");
    }
    if let Some(v) = get("LLMLINT_ONEHARNESS_TIMEOUT") {
        config.oneharness.timeout = Some(parse_int("LLMLINT_ONEHARNESS_TIMEOUT", &v, 1)?);
        note(prov, "oneharness.timeout", "LLMLINT_ONEHARNESS_TIMEOUT");
    }
    if let Some(v) = get("LLMLINT_ONEHARNESS_SCHEMA_MAX_RETRIES") {
        config.oneharness.schema_max_retries =
            Some(parse_int("LLMLINT_ONEHARNESS_SCHEMA_MAX_RETRIES", &v, 0)? as u32);
        note(
            prov,
            "oneharness.schema_max_retries",
            "LLMLINT_ONEHARNESS_SCHEMA_MAX_RETRIES",
        );
    }
    if let Some(v) = get("LLMLINT_PROMPT_TEMPLATE") {
        // Like `--prompt-template`, the value is a path whose *contents* become
        // the template. A read failure is located to the variable.
        let text = std::fs::read_to_string(&v).map_err(|e| Error::Env {
            var: "LLMLINT_PROMPT_TEMPLATE".to_string(),
            message: io_err(format!("reading prompt template {v}"), e).to_string(),
        })?;
        config.prompt_template = Some(text);
        note(prov, "prompt_template", "LLMLINT_PROMPT_TEMPLATE");
    }
    if let Some(v) = get("LLMLINT_RATIONALES") {
        config.rationales = Some(parse_bool("LLMLINT_RATIONALES", &v)?);
        note(prov, "rationales", "LLMLINT_RATIONALES");
    }
    if let Some(v) = get("LLMLINT_DIFF_BASE") {
        config.diff_base = Some(v);
        note(prov, "diff_base", "LLMLINT_DIFF_BASE");
    }
    if let Some(v) = get("LLMLINT_HISTORY_ENABLED") {
        config.history.enabled = Some(parse_bool("LLMLINT_HISTORY_ENABLED", &v)?);
        note(prov, "history.enabled", "LLMLINT_HISTORY_ENABLED");
    }
    if let Some(v) = get("LLMLINT_HISTORY_MAX_RUNS") {
        config.history.max_runs = Some(parse_int("LLMLINT_HISTORY_MAX_RUNS", &v, 1)? as usize);
        note(prov, "history.max_runs", "LLMLINT_HISTORY_MAX_RUNS");
    }
    if let Some(v) = get("LLMLINT_HISTORY_DIR") {
        config.history.dir = Some(v);
        note(prov, "history.dir", "LLMLINT_HISTORY_DIR");
    }
    Ok(())
}

/// Record `key`'s effective source as the env variable `var`, overwriting any
/// config source (env wins over the file, so it is the source to report).
fn note(prov: &mut Provenance, key: &str, var: &str) {
    prov.settings.insert(key.to_string(), format!("env:{var}"));
}

/// Read `name` from the environment, treating an unset *or empty* value as
/// absent (an exported-but-empty var is the shell's "unset", not a request to
/// override a setting with the empty string).
fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Parse a non-negative integer with a minimum, erroring (located to `var`) on a
/// non-number or an out-of-range value. Returns `u64`; callers narrow as needed.
fn parse_int(var: &str, val: &str, min: u64) -> Result<u64> {
    let n: u64 = val.trim().parse().map_err(|_| Error::Env {
        var: var.to_string(),
        message: format!("expected a whole number, got {val:?}"),
    })?;
    if n < min {
        return Err(Error::Env {
            var: var.to_string(),
            message: format!("must be >= {min}, got {n}"),
        });
    }
    Ok(n)
}

/// Split a `LIST_SEP`-separated glob list, trimming whitespace and dropping empty
/// entries. Erroring (located to `var`) when nothing usable remains, so a
/// set-but-empty `LLMLINT_FILES_*` is a clear boundary fault rather than a
/// silent "select/exclude nothing".
fn parse_list(var: &str, val: &str) -> Result<Vec<String>> {
    let globs: Vec<String> = val
        .split(LIST_SEP)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if globs.is_empty() {
        return Err(Error::Env {
            var: var.to_string(),
            message: format!("expected one or more {LIST_SEP:?}-separated globs, got {val:?}"),
        });
    }
    Ok(globs)
}

/// Parse a boolean using the documented grammar (`1`/`true`/`yes` vs
/// `0`/`false`/`no`, case-insensitive), erroring (located to `var`) otherwise.
fn parse_bool(var: &str, val: &str) -> Result<bool> {
    match val.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(Error::Env {
            var: var.to_string(),
            message: format!("expected a boolean (1/true/yes or 0/false/no), got {val:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::SETTING_KEYS;
    use std::collections::HashMap;

    /// Build a lookup closure over a fixed map, so the engine is exercised with
    /// no process-global env mutation.
    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn apply(config: &mut Config, prov: &mut Provenance, pairs: &[(&str, &str)]) -> Result<()> {
        apply_from(config, prov, lookup(pairs))
    }

    #[test]
    fn covers_every_overridable_setting() {
        // Every SETTING_KEYS entry is either env-overridable or explicitly
        // config-only — no setting silently lacks env support, and the two env
        // lists don't drift from the provenance key list.
        use std::collections::BTreeSet;
        let env: BTreeSet<&str> = ENV_SETTINGS.iter().map(|(k, _)| *k).collect();
        let config_only: BTreeSet<&str> = CONFIG_ONLY_SETTINGS.iter().copied().collect();
        let all: BTreeSet<&str> = SETTING_KEYS.iter().copied().collect();
        assert!(env.is_disjoint(&config_only), "a setting can't be both");
        let union: BTreeSet<&str> = env.union(&config_only).copied().collect();
        assert_eq!(
            union, all,
            "every setting must be env-overridable or config-only"
        );
    }

    #[test]
    fn env_var_names_match_the_setting_path() {
        // The naming convention is mechanical: LLMLINT_ + path uppercased, `.`→`_`.
        for (key, var) in ENV_SETTINGS {
            let expected = format!("LLMLINT_{}", key.to_uppercase().replace('.', "_"));
            assert_eq!(*var, expected, "env var for {key:?}");
        }
    }

    #[test]
    fn folds_scalar_settings_and_records_env_provenance() {
        let mut config = Config::default();
        let mut prov = Provenance::default();
        apply(
            &mut config,
            &mut prov,
            &[
                ("LLMLINT_ONEHARNESS_MODEL", "env-model"),
                ("LLMLINT_ONEHARNESS_TIMEOUT", "42"),
                ("LLMLINT_ONEHARNESS_SCHEMA_MAX_RETRIES", "0"),
                ("LLMLINT_ONEHARNESS_CONFIG", "oh.toml"),
                ("LLMLINT_ONEHARNESS_BIN", "/opt/oneharness"),
                ("LLMLINT_RATIONALES", "no"),
                ("LLMLINT_DIFF_BASE", "main"),
                ("LLMLINT_HISTORY_ENABLED", "false"),
                ("LLMLINT_HISTORY_MAX_RUNS", "7"),
                ("LLMLINT_HISTORY_DIR", "/tmp/h"),
            ],
        )
        .unwrap();
        assert_eq!(config.oneharness.model.as_deref(), Some("env-model"));
        assert_eq!(config.oneharness.timeout, Some(42));
        assert_eq!(config.oneharness.schema_max_retries, Some(0));
        assert_eq!(config.oneharness.config, vec!["oh.toml".to_string()]);
        assert_eq!(config.oneharness.bin.as_deref(), Some("/opt/oneharness"));
        assert_eq!(config.rationales, Some(false));
        assert_eq!(config.diff_base.as_deref(), Some("main"));
        assert_eq!(config.history.enabled, Some(false));
        assert_eq!(config.history.max_runs, Some(7));
        assert_eq!(config.history.dir.as_deref(), Some("/tmp/h"));

        assert_eq!(
            prov.settings["oneharness.model"],
            "env:LLMLINT_ONEHARNESS_MODEL"
        );
        assert_eq!(prov.settings["rationales"], "env:LLMLINT_RATIONALES");
        assert_eq!(
            prov.settings["history.max_runs"],
            "env:LLMLINT_HISTORY_MAX_RUNS"
        );
    }

    #[test]
    fn files_include_replaces_and_exclude_unions() {
        let sep = LIST_SEP;
        let mut config = Config {
            files: crate::domain::config::FileFilter {
                include: vec!["config/**".to_string()],
                exclude: vec!["**/config-ex.rs".to_string()],
            },
            ..Config::default()
        };
        let mut prov = Provenance::default();
        apply(
            &mut config,
            &mut prov,
            &[
                ("LLMLINT_FILES_INCLUDE", &format!("a/**{sep}b/**")),
                ("LLMLINT_FILES_EXCLUDE", "**/env-ex.rs"),
            ],
        )
        .unwrap();
        // include replaces the config globs; exclude unions onto them.
        assert_eq!(config.files.include, vec!["a/**", "b/**"]);
        assert_eq!(
            config.files.exclude,
            vec!["**/config-ex.rs", "**/env-ex.rs"]
        );
        assert_eq!(prov.settings["files.include"], "env:LLMLINT_FILES_INCLUDE");
        assert_eq!(prov.settings["files.exclude"], "env:LLMLINT_FILES_EXCLUDE");
    }

    #[test]
    fn empty_files_list_is_a_boundary_error() {
        let mut config = Config::default();
        let mut prov = Provenance::default();
        let err = apply(&mut config, &mut prov, &[("LLMLINT_FILES_INCLUDE", "   ")]).unwrap_err();
        match err {
            Error::Env { var, .. } => assert_eq!(var, "LLMLINT_FILES_INCLUDE"),
            other => panic!("expected Error::Env, got {other:?}"),
        }
    }

    #[test]
    fn env_overrides_a_config_value_and_replaces_its_provenance() {
        let mut config = Config::default();
        config.oneharness.model = Some("config-model".to_string());
        let mut prov = Provenance::default();
        prov.settings
            .insert("oneharness.model".to_string(), "llmlint.yml".to_string());

        apply(
            &mut config,
            &mut prov,
            &[("LLMLINT_ONEHARNESS_MODEL", "env-model")],
        )
        .unwrap();
        // Env wins over the config value, and provenance points at the env var —
        // not the file it superseded.
        assert_eq!(config.oneharness.model.as_deref(), Some("env-model"));
        assert_eq!(
            prov.settings["oneharness.model"],
            "env:LLMLINT_ONEHARNESS_MODEL"
        );
    }

    #[test]
    fn unset_env_leaves_config_and_provenance_untouched() {
        let mut config = Config {
            diff_base: Some("develop".to_string()),
            ..Config::default()
        };
        let mut prov = Provenance::default();
        prov.settings
            .insert("diff_base".to_string(), "llmlint.yml".to_string());
        apply(&mut config, &mut prov, &[]).unwrap();
        assert_eq!(config.diff_base.as_deref(), Some("develop"));
        assert_eq!(prov.settings["diff_base"], "llmlint.yml");
    }

    #[test]
    fn bool_grammar_accepts_documented_spellings() {
        for on in ["1", "true", "TRUE", "Yes", " yes "] {
            assert!(parse_bool("V", on).unwrap(), "{on:?} should be true");
        }
        for off in ["0", "false", "No", " NO "] {
            assert!(!parse_bool("V", off).unwrap(), "{off:?} should be false");
        }
    }

    #[test]
    fn malformed_bool_is_located_to_the_variable() {
        let mut config = Config::default();
        let mut prov = Provenance::default();
        let err = apply(&mut config, &mut prov, &[("LLMLINT_RATIONALES", "maybe")]).unwrap_err();
        match err {
            Error::Env { var, message } => {
                assert_eq!(var, "LLMLINT_RATIONALES");
                assert!(message.contains("boolean"), "got: {message}");
            }
            other => panic!("expected Error::Env, got {other:?}"),
        }
    }

    #[test]
    fn non_numeric_and_below_minimum_are_errors() {
        let mut config = Config::default();
        let mut prov = Provenance::default();
        assert!(matches!(
            apply(
                &mut config,
                &mut prov,
                &[("LLMLINT_ONEHARNESS_TIMEOUT", "abc")]
            ),
            Err(Error::Env { .. })
        ));
        // max_runs must be >= 1.
        let err = apply(&mut config, &mut prov, &[("LLMLINT_HISTORY_MAX_RUNS", "0")]).unwrap_err();
        match err {
            Error::Env { var, message } => {
                assert_eq!(var, "LLMLINT_HISTORY_MAX_RUNS");
                assert!(message.contains(">= 1"), "got: {message}");
            }
            other => panic!("expected Error::Env, got {other:?}"),
        }
    }

    #[test]
    fn prompt_template_reads_the_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tmpl.txt");
        std::fs::write(&path, "custom template body").unwrap();
        let mut config = Config::default();
        let mut prov = Provenance::default();
        apply(
            &mut config,
            &mut prov,
            &[("LLMLINT_PROMPT_TEMPLATE", path.to_str().unwrap())],
        )
        .unwrap();
        assert_eq!(
            config.prompt_template.as_deref(),
            Some("custom template body")
        );
        assert_eq!(
            prov.settings["prompt_template"],
            "env:LLMLINT_PROMPT_TEMPLATE"
        );
    }

    #[test]
    fn prompt_template_missing_file_is_located_to_the_variable() {
        let mut config = Config::default();
        let mut prov = Provenance::default();
        let err = apply(
            &mut config,
            &mut prov,
            &[("LLMLINT_PROMPT_TEMPLATE", "/no/such/template.txt")],
        )
        .unwrap_err();
        match err {
            Error::Env { var, .. } => assert_eq!(var, "LLMLINT_PROMPT_TEMPLATE"),
            other => panic!("expected Error::Env, got {other:?}"),
        }
    }
}
