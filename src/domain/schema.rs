//! Generate the JSON Schema that constrains a judge's structured output.
//!
//! oneharness (`run --schema`) enforces and validates this schema itself, so
//! llmlint only has to describe the shape: an object keyed by rule name, each
//! value an object with a boolean `holds` and an optional `violations` array.

use serde_json::{json, Map, Value};

/// Build the structured-output schema for a batch of rule names.
pub fn build(rule_names: &[&str]) -> Value {
    let mut properties = Map::new();
    for &name in rule_names {
        properties.insert(name.to_string(), rule_schema());
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": rule_names,
        "properties": properties,
    })
}

fn rule_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["holds"],
        "properties": {
            "holds": { "type": "boolean" },
            "violations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "file": { "type": "string" },
                        "line": { "type": "integer", "minimum": 1 },
                        "end_line": { "type": "integer", "minimum": 1 },
                        "message": { "type": "string" }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_keys_each_rule_and_requires_holds() {
        let s = build(&["a_rule", "b_rule"]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], json!(["a_rule", "b_rule"]));
        let a = &s["properties"]["a_rule"];
        assert_eq!(a["required"], json!(["holds"]));
        assert_eq!(a["properties"]["holds"]["type"], "boolean");
        assert_eq!(a["properties"]["violations"]["type"], "array");
    }

    #[test]
    fn empty_rule_set_is_a_well_formed_object_schema() {
        let s = build(&[]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!([]));
        assert!(s["properties"].as_object().unwrap().is_empty());
    }
}
