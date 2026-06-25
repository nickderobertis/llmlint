//! Generate the public JSON Schema for an llmlint config file.
//!
//! This schema describes the config model in [`crate::domain::config`] and is
//! published at [`SCHEMA_URL`] (the bundled copy ships as
//! `assets/llmlint.schema.json`). `llmlint init` writes a
//! `# yaml-language-server: $schema=…` modeline pointing at that URL, so editors
//! with the YAML language server give completion and validation for free.
//!
//! The schema is *derived* from the [`Config`] model itself (via `schemars`), so
//! structure and field docs come straight from the Rust types — add a field and
//! it appears automatically. The committed asset
//! is pinned to this output by `committed_asset_matches_generated_schema`, so it
//! can never drift: change the model, regenerate the asset
//! (`LLMLINT_UPDATE_SCHEMA=1 cargo test -p llmlint config_schema`), and the test
//! goes green again.

use serde_json::Value;

use crate::domain::config::Config;

/// Canonical public URL of the config JSON Schema. Served from the repo's
/// `assets/` over raw.githubusercontent, exactly like the bundled config-lint
/// plugin. Used as the schema's `$id` and as the `$schema=` target written into
/// configs by `llmlint init`.
pub const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/nickderobertis/llmlint/main/assets/llmlint.schema.json";

/// The `# yaml-language-server` modeline that points an editor at the published
/// schema. `llmlint init` writes this as the first line of a generated config.
pub fn modeline() -> String {
    format!("# yaml-language-server: $schema={SCHEMA_URL}\n")
}

/// Build the JSON Schema (2020-12) for an llmlint config, derived from the
/// [`Config`] model and published with [`SCHEMA_URL`] as its `$id`.
pub fn build() -> Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Config)).expect("config schema serializes");
    // schemars doesn't assign an `$id`; set it to the canonical published URL so
    // the schema is self-describing and tools can dereference it.
    if let Value::Object(map) = &mut schema {
        map.insert("$id".to_string(), Value::String(SCHEMA_URL.to_string()));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::assets;

    #[test]
    fn schema_describes_the_top_level_config_shape() {
        let s = build();
        assert_eq!(s["$id"], SCHEMA_URL);
        assert_eq!(s["type"], "object");
        // Every modeled top-level key is described.
        let props = s["properties"].as_object().unwrap();
        for key in [
            "version",
            "prompt_template",
            "files",
            "oneharness",
            "plugins",
            "agents",
            "rules",
        ] {
            assert!(props.contains_key(key), "missing property {key}");
        }
        // A rule requires name + description, like the Rust model. Resolve the
        // `rules` item `$ref` rather than hardcoding the schemars-derived def name.
        let rule_ref = s["properties"]["rules"]["items"]["$ref"]
            .as_str()
            .expect("rules.items is a $ref");
        let rule_def = rule_ref.rsplit('/').next().unwrap();
        let required = s["$defs"][rule_def]["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
        assert!(required.iter().any(|v| v == "description"));
    }

    #[test]
    fn modeline_targets_the_published_schema() {
        let line = modeline();
        assert!(line.starts_with("# yaml-language-server: $schema="));
        assert!(line.contains(SCHEMA_URL));
        assert!(line.ends_with('\n'));
    }

    /// The committed `assets/llmlint.schema.json` is what gets published and
    /// referenced by init; pin it to the generator so the two never drift. Set
    /// `LLMLINT_UPDATE_SCHEMA=1` to rewrite the asset from the generator.
    #[test]
    fn committed_asset_matches_generated_schema() {
        let generated = build();
        let committed: Value =
            serde_json::from_str(assets::CONFIG_SCHEMA).expect("asset is valid JSON");
        if committed != generated {
            if std::env::var_os("LLMLINT_UPDATE_SCHEMA").is_some() {
                let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/llmlint.schema.json");
                let mut body = serde_json::to_string_pretty(&generated).unwrap();
                body.push('\n');
                std::fs::write(path, body).unwrap();
                return;
            }
            panic!(
                "assets/llmlint.schema.json is out of sync with config_schema::build(); \
                 regenerate with `LLMLINT_UPDATE_SCHEMA=1 cargo test -p llmlint config_schema`"
            );
        }
    }
}
