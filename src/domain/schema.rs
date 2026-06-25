//! Generate the JSON Schema that constrains a judge's structured output.
//!
//! oneharness (`run --schema`) enforces and validates this schema itself, so
//! llmlint only has to describe the shape: an object keyed by rule name, each
//! value an object whose fields are presented in a fixed order — `name`, then an
//! optional `rationale`, then the verdict (`holds` + `violations`). The order is
//! deliberate: the judge echoes the rule's `name` first to anchor on the right
//! rule, reasons in the `rationale`, and only then commits to `holds`, so
//! next-token prediction keeps each verdict consistent and on-target. Whether a
//! rule carries a `rationale` is decided per rule (the session default, with a
//! per-rule override). The key ordering survives serialization because
//! `serde_json` is built with `preserve_order`.

use serde_json::{json, Map, Value};

/// One rule's slot in the structured-output schema: its name and whether the
/// judge must supply a `rationale` for it.
#[derive(Debug, Clone, Copy)]
pub struct SchemaRule<'a> {
    pub name: &'a str,
    pub rationale: bool,
}

/// Build the structured-output schema for a batch of rules.
pub fn build(rules: &[SchemaRule]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::with_capacity(rules.len());
    for rule in rules {
        properties.insert(rule.name.to_string(), rule_schema(rule));
        required.push(rule.name);
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

fn rule_schema(rule: &SchemaRule) -> Value {
    // Insertion order is the emission order we want: name -> [rationale] ->
    // holds -> violations. `required` lists the same fields in the same order.
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();

    // 1. Echo the rule name, pinned to the exact value so the judge can't drift
    //    onto the wrong rule.
    properties.insert(
        "name".to_string(),
        json!({ "type": "string", "const": rule.name }),
    );
    required.push(json!("name"));

    // 2. The rationale (only when this rule wants one): a terse justification
    //    written before the verdict.
    if rule.rationale {
        properties.insert(
            "rationale".to_string(),
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Terse, evidence-citing justification for `holds`, written before the verdict.",
            }),
        );
        required.push(json!("rationale"));
    }

    // 3. The verdict itself.
    properties.insert("holds".to_string(), json!({ "type": "boolean" }));
    required.push(json!("holds"));
    properties.insert(
        "violations".to_string(),
        json!({
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
        }),
    );

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": Value::Array(required),
        "properties": Value::Object(properties),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(name: &str, rationale: bool) -> SchemaRule<'_> {
        SchemaRule { name, rationale }
    }

    /// The order of keys in a `properties` object, as they will serialize (relies
    /// on `serde_json`'s `preserve_order`).
    fn key_order(props: &Value) -> Vec<String> {
        props
            .as_object()
            .unwrap()
            .keys()
            .map(String::from)
            .collect()
    }

    #[test]
    fn schema_keys_each_rule_and_requires_holds() {
        let s = build(&[with("a_rule", true), with("b_rule", true)]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], json!(["a_rule", "b_rule"]));
        let a = &s["properties"]["a_rule"];
        assert_eq!(a["properties"]["holds"]["type"], "boolean");
        assert_eq!(a["properties"]["violations"]["type"], "array");
    }

    #[test]
    fn rule_fields_are_ordered_name_rationale_holds() {
        let s = build(&[with("only", true)]);
        let rule = &s["properties"]["only"];
        // Both the property map and the `required` list are in emission order.
        assert_eq!(
            key_order(&rule["properties"]),
            ["name", "rationale", "holds", "violations"]
        );
        assert_eq!(rule["required"], json!(["name", "rationale", "holds"]));
        // The name is pinned to the exact rule so the judge can't mislabel it.
        assert_eq!(rule["properties"]["name"]["const"], "only");
        assert_eq!(rule["properties"]["rationale"]["minLength"], 1);
    }

    #[test]
    fn rationale_off_drops_the_field_but_keeps_order() {
        let s = build(&[with("only", false)]);
        let rule = &s["properties"]["only"];
        assert_eq!(
            key_order(&rule["properties"]),
            ["name", "holds", "violations"]
        );
        assert_eq!(rule["required"], json!(["name", "holds"]));
        assert!(rule["properties"].get("rationale").is_none());
    }

    #[test]
    fn rationale_is_decided_per_rule() {
        // A batch can mix rationale-on and rationale-off rules (per-rule override).
        let s = build(&[with("on", true), with("off", false)]);
        assert_eq!(
            s["properties"]["on"]["required"],
            json!(["name", "rationale", "holds"])
        );
        assert_eq!(s["properties"]["off"]["required"], json!(["name", "holds"]));
    }

    #[test]
    fn rule_order_is_preserved_in_top_level_properties() {
        let s = build(&[with("zed", true), with("alpha", true)]);
        // Not alphabetized — emission order matches the batch order.
        assert_eq!(key_order(&s["properties"]), ["zed", "alpha"]);
    }

    #[test]
    fn empty_rule_set_is_a_well_formed_object_schema() {
        let s = build(&[]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!([]));
        assert!(s["properties"].as_object().unwrap().is_empty());
    }
}
