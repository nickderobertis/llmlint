//! `llmlint fingerprint`: one stable SHA-256 digest of everything that decides a
//! `lint` verdict — llmlint's version, the effective merged config (remote
//! plugins as the versions actually fetched), the prompt template in force, and
//! the oneharness configuration and version the judge would run under — so a
//! cache can replay a verdict only for the judge that produced it.
//!
//! The material is resolved through the same code `lint` runs (config load, env
//! and CLI overlays, the oneharness config layers), then made checkout-free:
//! paths under the lint directory become relative, `HOME` becomes `~`, the
//! oneharness binary's path and every `source` annotation are dropped, and the
//! [`RECORDING_ONLY`] settings — which only record a run — are removed. The
//! oneharness configuration comes from `oneharness config` itself; llmlint never
//! re-implements oneharness's resolution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::cli::FingerprintArgs;
use crate::commands::{ignores, lint};
use crate::domain::config::validate;
use crate::errors::{Error, Result};
use crate::io::{assets, configfs, env, files, oneharness};

/// Whose configuration a [`RECORDING_ONLY`] setting belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// llmlint's effective config; the path is into its serialized form.
    Llmlint,
    /// `oneharness config`'s report; the path is a top-level field.
    Oneharness,
}

/// Settings that only record a run and cannot change a verdict, left out of the
/// digest so a per-run or per-host recording choice (an orchestrator's run labels,
/// a history directory outside the checkout) never changes it. llmlint's paths
/// also cover their `LLMLINT_HISTORY_*` env overrides, which fold into the config
/// (`LLMLINT_NO_HISTORY` never reaches it). The `fingerprint` help and the README
/// name the same set.
pub const RECORDING_ONLY: &[(Owner, &str)] = &[
    (Owner::Llmlint, "history.enabled"),
    (Owner::Llmlint, "history.max_runs"),
    (Owner::Llmlint, "history.dir"),
    (Owner::Oneharness, "history"),
    (Owner::Oneharness, "history_dir"),
    (Owner::Oneharness, "history_pointer_file"),
    (Owner::Oneharness, "history_labels"),
];

pub fn run(args: FingerprintArgs) -> Result<i32> {
    let digest = compute(args.into_lint_args())?;
    println!("{digest}");
    Ok(0)
}

/// Resolve the judge a `lint` with `args` would run and digest it.
fn compute(args: crate::cli::LintArgs) -> Result<String> {
    let cwd = lint::resolve_cwd(&args.cwd)?;
    let loaded = configfs::load_with_targets(&args.config, &cwd, &[])?;
    let mut scopes = loaded.scopes;
    let mut config = loaded.config;
    validate(&config)?;
    let pre_files = config.files.clone();
    env::apply_overrides(&mut config)?;
    lint::apply_cli_overrides(&mut config, &args)?;
    if config.files != pre_files {
        ignores::retarget_session_scopes(&mut scopes, &cwd, &config.files);
    }

    // The oneharness side: the binary resolved exactly as `lint` resolves it,
    // its version, and the configuration it would resolve for the judge run.
    let oh_files = lint::resolve_oneharness_config(&args, &config);
    for file in &oh_files {
        std::fs::File::open(file).map_err(|e| {
            Error::HarnessConfig(format!(
                "cannot read the oneharness config {}: {e}; fix or remove it (it is named \
                 by `oneharness.config`, LLMLINT_ONEHARNESS_CONFIG or --oneharness-config)",
                file.display()
            ))
        })?;
    }
    let bin = args
        .oneharness_bin
        .clone()
        .or_else(|| config.oneharness.bin.clone());
    let client = oneharness::Client::new(bin.as_deref());
    // The same floor `lint` enforces: a oneharness `lint` would refuse has no
    // judge to describe.
    let oh_version = client.check_min_version()?;
    oneharness::Client::check_layered_config_version(&oh_version, oh_files.len())?;
    let oh_config = client.resolved_config(&cwd, &oh_files)?;

    // The llmlint side: the effective config, with each default made explicit
    // (so a changed built-in default reads as a change, and a setting spelled
    // out at its default is the same judge as one left unset) and each setting
    // that has already been resolved hashed as its result rather than as how it
    // was spelled: the plugin specs are their merged rules plus the resolved
    // versions below, and the oneharness config layers are the one ordered list
    // `lint` forwards — whether a layer came from the config, the env or a flag.
    config.rationales = Some(config.rationales_default());
    config.plugins.clear();
    config.oneharness.config.clear();
    let layers: Vec<String> = oh_files.iter().map(|f| files::to_slash(f)).collect();
    config.prompt_template = Some(
        config
            .prompt_template
            .clone()
            .unwrap_or_else(|| assets::DEFAULT_TEMPLATE.to_string()),
    );
    config.max_prompt_chars = std::num::NonZeroU64::new(config.max_prompt_chars() as u64);
    config.oneharness.bin = None;
    let mut config_value =
        serde_json::to_value(&config).map_err(|e| Error::Io(format!("serializing config: {e}")))?;
    let mut oh_value = without_sources(oh_config);
    if let Value::Object(map) = &mut oh_value {
        map.remove("config_files");
    }
    for (owner, path) in RECORDING_ONLY {
        match owner {
            Owner::Llmlint => remove_path(&mut config_value, path),
            Owner::Oneharness => remove_path(&mut oh_value, path),
        }
    }

    let scopes: BTreeMap<String, Value> = scopes
        .iter()
        .map(|(rule, scope)| {
            (
                rule.clone(),
                json!({
                    "dir": files::to_slash(scope.dir.strip_prefix(&cwd).unwrap_or(&scope.dir)),
                    "files": scope.files,
                }),
            )
        })
        .collect();
    let plugins: Vec<Value> = loaded
        .plugins
        .iter()
        .map(|p| {
            json!({
                "url": p.url,
                "pin": p.resolved.pin().map(ToString::to_string),
                "version": p.resolved.version().map(ToString::to_string),
            })
        })
        .collect();

    let mut material = json!({
        "llmlint": env!("CARGO_PKG_VERSION"),
        "config": config_value,
        "scopes": scopes,
        "plugins": plugins,
        "oneharness": { "version": oh_version, "layers": layers, "config": oh_value },
    });
    relativize(&mut material, &replacements(&cwd));
    let bytes = serde_json::to_vec(&canonical(material))
        .map_err(|e| Error::Io(format!("serializing fingerprint material: {e}")))?;
    Ok(hex(
        ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()
    ))
}

/// Collapse oneharness's `{value, source}` annotations to their values: where a
/// setting came from is a path on this host, not part of the judge.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the `{value, source}` shape is the released oneharness's `config` report, which publishes no schema; `Client::resolved_config` admits only reports of this shape, and `fingerprint_reads_the_real_harness_configuration` (the real-oneharness tier, `just test-oneharness`) reconciles both against the released binary
fn without_sources(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            if map.len() == 2 && map.contains_key("value") && map.contains_key("source") {
                return without_sources(map["value"].clone());
            }
            Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, without_sources(v)))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(without_sources).collect()),
        other => other,
    }
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// `value` with every object's keys in sorted order, so the bytes hashed never
/// depend on the order a setting was inserted, merged or removed in (objects keep
/// insertion order here, and removing a key reorders the rest).
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Remove the value at a dotted `path` from a JSON object, if present.
/// A parent object the removal leaves empty is removed too, so a setting that
/// was never set and one whose every field was removed hash the same.
fn remove_path(value: &mut Value, path: &str) {
    let Value::Object(map) = value else { return };
    match path.split_once('.') {
        None => {
            map.remove(path);
        }
        Some((head, rest)) => {
            let Some(child) = map.get_mut(head) else {
                return;
            };
            remove_path(child, rest);
            if child.as_object().is_some_and(Map::is_empty) {
                map.remove(head);
            }
        }
    }
}

/// The host-specific prefixes to rewrite, longest first: the lint directory
/// (as given and canonical, in native and forward-slash spelling) becomes `.`,
/// and the home directory becomes `~`.
fn replacements(cwd: &Path) -> Vec<(String, String)> {
    let mut dirs: Vec<(PathBuf, &str)> = Vec::new();
    for dir in [
        Some(cwd.to_path_buf()),
        std::path::absolute(cwd).ok(),
        cwd.canonicalize().ok(),
    ]
    .into_iter()
    .flatten()
    {
        dirs.push((dir, "."));
    }
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(var).filter(|h| !h.is_empty()) {
            let home = PathBuf::from(home);
            if let Ok(canonical) = home.canonicalize() {
                dirs.push((canonical, "~"));
            }
            dirs.push((home, "~"));
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (dir, to) in dirs {
        let native = dir
            .to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_string();
        // A relative or root path is no prefix worth rewriting.
        if native.len() <= 1 || !dir.is_absolute() {
            continue;
        }
        for from in [native.clone(), native.replace('\\', "/")] {
            if !out.iter().any(|(f, _)| *f == from) {
                out.push((from, to.to_string()));
            }
        }
    }
    out.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    out
}

/// Rewrite every string in `value` that contains a host-specific prefix.
fn relativize(value: &mut Value, prefixes: &[(String, String)]) {
    match value {
        Value::String(s) => {
            for (from, to) in prefixes {
                if s.contains(from.as_str()) {
                    *s = replace_dir(s, from, to);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| relativize(v, prefixes)),
        Value::Object(map) => map.values_mut().for_each(|v| relativize(v, prefixes)),
        _ => {}
    }
}

/// Replace each occurrence of the directory `from` in `s` with `to`, but only
/// where it is a whole path prefix — followed by a separator or the end — so a
/// sibling like `/work/treeish` is not taken for `/work/tree`.
fn replace_dir(s: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(from) {
        let after = &rest[i + from.len()..];
        out.push_str(&rest[..i]);
        if after.is_empty() || after.starts_with(['/', '\\']) {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_collapse_to_values_at_any_depth() {
        let v = json!({
            "model": {"value": "m", "source": "/abs/oneharness.toml"},
            "harness": {"codex": {"model": {"value": "x", "source": "default"}}},
            "env": {},
        });
        assert_eq!(
            without_sources(v),
            json!({"model": "m", "harness": {"codex": {"model": "x"}}, "env": {}})
        );
    }

    #[test]
    fn dotted_paths_are_removed_and_absent_ones_ignored() {
        let mut v = json!({"history": {"dir": "d", "enabled": true}, "rules": []});
        remove_path(&mut v, "history.dir");
        remove_path(&mut v, "nope.deeper");
        remove_path(&mut v, "history_labels");
        assert_eq!(v, json!({"history": {"enabled": true}, "rules": []}));
        // Emptied by the removal, the parent goes too.
        remove_path(&mut v, "history.enabled");
        assert_eq!(v, json!({"rules": []}));
    }

    #[test]
    fn host_prefixes_become_relative() {
        let mut v = json!({"a": "/work/tree/oneharness.toml", "b": ["file:///work/tree/p.yml"], "c": "/work/treeish"});
        relativize(&mut v, &[("/work/tree".to_string(), ".".to_string())]);
        assert_eq!(v["a"], "./oneharness.toml");
        assert_eq!(v["b"][0], "file://./p.yml");
        assert_eq!(v["c"], "/work/treeish");
    }

    #[test]
    fn the_help_and_readme_name_every_recording_only_setting() {
        use clap::CommandFactory;
        let mut cmd = crate::cli::Cli::command();
        let help = cmd
            .find_subcommand_mut("fingerprint")
            .unwrap()
            .render_long_help()
            .to_string();
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        let readme = include_str!("../../README.md");
        for (_, path) in RECORDING_ONLY {
            assert!(
                help.contains(&format!("`{path}`")),
                "help omits {path}:\n{help}"
            );
            assert!(readme.contains(&format!("`{path}`")), "README omits {path}");
        }
    }

    #[test]
    fn canonical_json_is_independent_of_key_order() {
        let a = canonical(json!({"b": 1, "a": {"y": [{"q": 1, "p": 2}], "x": 0}}));
        let b = canonical(json!({"a": {"x": 0, "y": [{"p": 2, "q": 1}]}, "b": 1}));
        assert_eq!(a.to_string(), b.to_string());
        assert_eq!(a.to_string(), r#"{"a":{"x":0,"y":[{"p":2,"q":1}]},"b":1}"#);
    }

    #[test]
    fn hex_is_lowercase_and_two_digits_per_byte() {
        assert_eq!(hex(&[0, 0xab, 0x0f]), "00ab0f");
    }
}
