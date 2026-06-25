//! Generate the public JSON Schema for an llmlint config file.
//!
//! This schema describes the config model in [`crate::domain::config`] and is
//! published at [`SCHEMA_URL`] (the bundled copy ships as
//! `assets/llmlint.schema.json`). `llmlint init` writes a
//! `# yaml-language-server: $schema=…` modeline pointing at that URL, so editors
//! with the YAML language server give completion and validation for free.
//!
//! The generator here is the single source of truth: the committed asset is
//! pinned to this output by `committed_asset_matches_generated_schema`, so the
//! schema can never drift from the Rust model — change the model, regenerate the
//! asset (`LLMLINT_UPDATE_SCHEMA=1 cargo test -p llmlint config_schema`), and the
//! test goes green again.

use serde_json::{json, Value};

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

/// Build the JSON Schema (draft 2020-12) describing an llmlint config file.
pub fn build() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SCHEMA_URL,
        "title": "llmlint configuration",
        "description":
            "Configuration for llmlint, an LLM-as-judge linter for code-quality \
             checks deterministic linters can't express. \
             Docs: https://github.com/nickderobertis/llmlint",
        "type": "object",
        // Unknown top-level keys are allowed so YAML anchors can be parked in a
        // throwaway key (e.g. `x-prompts:`); nested objects reject extras.
        "additionalProperties": true,
        "properties": {
            "version": {
                "description":
                    "This config's own published version (1, 1.2, or \"1.2.3\"). It \
                     matters when the config is consumed elsewhere as a plugin: that \
                     consumer pins a desired version with an `@` suffix on the URL.",
                "type": ["integer", "number", "string"]
            },
            "prompt_template": {
                "description":
                    "Master minijinja prompt template, rendered with `rules` (each \
                     with name + description) and `files` (the target paths). \
                     Overrides the built-in template.",
                "type": "string"
            },
            "files": {
                "description":
                    "Default include/exclude globs selecting target files when none \
                     are passed on the CLI.",
                "$ref": "#/$defs/fileFilter"
            },
            "oneharness": {
                "description": "Defaults for how llmlint invokes the oneharness subprocess.",
                "$ref": "#/$defs/oneharness"
            },
            "plugins": {
                "description":
                    "Shared rule sets merged in, one entry each: a local file path or \
                     a URL (http(s)://, file://), the URL optionally pinned to a \
                     version with an `@` suffix (e.g. `…/rules.yml@1.2`).",
                "type": "array",
                "items": { "type": "string" }
            },
            "agents": {
                "description":
                    "Named agents that group rules and share harness/model/batch \
                     config. A rule with no `agent` uses the `default` agent.",
                "type": "object",
                "additionalProperties": { "$ref": "#/$defs/agent" }
            },
            "rules": {
                "description":
                    "The lint rules. Each is a positive invariant judged true/false \
                     about the target files (holds=true passes; holds=false is a \
                     violation).",
                "type": "array",
                "items": { "$ref": "#/$defs/rule" }
            }
        },
        "$defs": {
            "fileFilter": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "include": {
                        "description": "Globs selecting files to lint.",
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "exclude": {
                        "description": "Globs subtracted from the included set.",
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "oneharness": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "config": {
                        "description":
                            "oneharness config file(s) to forward via `--config` \
                             (single-file today; extras are warned and dropped).",
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bin": {
                        "description": "Override the oneharness binary path.",
                        "type": "string"
                    },
                    "model": {
                        "description":
                            "Default model for every judge (an agent's `model` \
                             overrides it).",
                        "type": "string"
                    },
                    "timeout": {
                        "description": "Per-judge timeout in seconds (default 120).",
                        "type": "integer",
                        "minimum": 1
                    },
                    "schema_max_retries": {
                        "description":
                            "Schema-validation re-prompt budget passed to oneharness \
                             `--schema-max-retries`.",
                        "type": "integer",
                        "minimum": 0
                    }
                }
            },
            "agent": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "harness": {
                        "description":
                            "Harness id from `oneharness list`. Omit to let oneharness \
                             pick its own configured default harness.",
                        "type": "string"
                    },
                    "model": {
                        "description": "Model override for this agent's judges.",
                        "type": "string"
                    },
                    "batch_size": {
                        "description": "Max rules per judge run (default 20).",
                        "type": "integer",
                        "minimum": 1
                    },
                    "prompt_template": {
                        "description":
                            "Extra prompt text appended to the master template before \
                             rendering.",
                        "type": "string"
                    },
                    "files": {
                        "description": "Override the target files for this agent's rules.",
                        "$ref": "#/$defs/fileFilter"
                    }
                }
            },
            "rule": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "description"],
                "properties": {
                    "name": {
                        "description":
                            "Terse snake_case identifier: an ASCII letter followed by \
                             letters, digits, or underscores. Used as a JSON Schema key.",
                        "type": "string",
                        "pattern": "^[A-Za-z][A-Za-z0-9_]*$"
                    },
                    "description": {
                        "description":
                            "The invariant the judge evaluates. State clearly what is \
                             TRUE (passes) and what is FALSE (a violation).",
                        "type": "string",
                        "minLength": 1
                    },
                    "agent": {
                        "description":
                            "Name of the agent (under `agents`) this rule runs on. \
                             Defaults to the `default` agent.",
                        "type": "string"
                    },
                    "judges": {
                        "description":
                            "Independent judges to run; the majority verdict wins \
                             (default 1). Must be odd so the vote can't tie.",
                        "type": "integer",
                        "minimum": 1
                    },
                    "files": {
                        "description": "Override the target files for this rule.",
                        "$ref": "#/$defs/fileFilter"
                    }
                }
            }
        }
    })
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
        // A rule requires name + description, like the Rust model.
        assert_eq!(
            s["$defs"]["rule"]["required"],
            json!(["name", "description"])
        );
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
