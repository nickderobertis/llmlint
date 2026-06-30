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

/// One rule's slot in the structured-output schema: its name, whether the judge
/// must supply a `rationale` for it, and whether the judge must first decide the
/// rule's `relevance` to the change.
#[derive(Debug, Clone, Copy)]
pub struct SchemaRule<'a> {
    pub name: &'a str,
    pub rationale: bool,
    /// When true, the judge gates the verdict on a `relevant` boolean: it may
    /// report `relevant=false` (no `holds`), or `relevant=true` then the verdict.
    pub relevance: bool,
    /// When true, every violation must carry a concrete `file` and `line`: the
    /// violation item schema marks both **required**, so oneharness re-prompts
    /// the judge (in one batched turn) until every violation is localized.
    pub require_line_attribution: bool,
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
    // [relevant] -> holds -> violations. `required` lists the same fields in the
    // same order.
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
    //    written before the verdict. With relevance gating, it also explains why
    //    a rule is (not) relevant.
    if rule.rationale {
        let description = if rule.relevance {
            "Terse, evidence-citing justification, written before `relevant`/`holds`: why the rule is (or isn't) relevant, and — when relevant — the verdict."
        } else {
            "Terse, evidence-citing justification for `holds`, written before the verdict."
        };
        properties.insert(
            "rationale".to_string(),
            json!({ "type": "string", "minLength": 1, "description": description }),
        );
        required.push(json!("rationale"));
    }

    // 3. The relevance gate (only when the judge decides relevance): decided
    //    before the verdict. `holds` is required only when `relevant` is true
    //    (enforced by the if/then below); an irrelevant rule stops here.
    if rule.relevance {
        properties.insert(
            "relevant".to_string(),
            json!({
                "type": "boolean",
                "description": "Whether this rule applies to the change. Decide before the verdict; when false, omit `holds`/`violations`.",
            }),
        );
        required.push(json!("relevant"));
    }

    // 4. The verdict itself. `holds` is unconditionally required only when the
    //    judge does not decide relevance; otherwise it is gated on `relevant`.
    properties.insert("holds".to_string(), json!({ "type": "boolean" }));
    if !rule.relevance {
        required.push(json!("holds"));
    }
    // The violation item. When the rule requires line attribution, `file` and
    // `line` are marked required so oneharness re-prompts the judge until every
    // violation is localized; otherwise they stay optional (some findings can't
    // be pinned to one source line).
    let mut item = Map::new();
    item.insert("type".to_string(), json!("object"));
    item.insert("additionalProperties".to_string(), json!(false));
    item.insert(
        "properties".to_string(),
        json!({
            "file": { "type": "string" },
            "line": { "type": "integer", "minimum": 1 },
            "end_line": { "type": "integer", "minimum": 1 },
            "message": { "type": "string" }
        }),
    );
    if rule.require_line_attribution {
        item.insert("required".to_string(), json!(["file", "line"]));
    }
    properties.insert(
        "violations".to_string(),
        json!({ "type": "array", "items": Value::Object(item) }),
    );

    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    obj.insert("additionalProperties".to_string(), json!(false));
    obj.insert("required".to_string(), Value::Array(required));
    obj.insert("properties".to_string(), Value::Object(properties));
    if rule.relevance {
        // Require the verdict only once the rule is judged relevant, so an
        // irrelevant verdict legitimately ends after `relevant`.
        obj.insert(
            "if".to_string(),
            json!({ "properties": { "relevant": { "const": true } }, "required": ["relevant"] }),
        );
        obj.insert("then".to_string(), json!({ "required": ["holds"] }));
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(name: &str, rationale: bool) -> SchemaRule<'_> {
        SchemaRule {
            name,
            rationale,
            relevance: false,
            require_line_attribution: false,
        }
    }

    fn with_relevance(name: &str, rationale: bool) -> SchemaRule<'_> {
        SchemaRule {
            name,
            rationale,
            relevance: true,
            require_line_attribution: false,
        }
    }

    fn with_attribution(name: &str) -> SchemaRule<'_> {
        SchemaRule {
            name,
            rationale: false,
            relevance: false,
            require_line_attribution: true,
        }
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
    fn relevance_inserts_a_gate_before_holds_and_requires_holds_only_when_relevant() {
        let s = build(&[with_relevance("gated", true)]);
        let rule = &s["properties"]["gated"];
        // The relevant gate sits between the rationale and the verdict.
        assert_eq!(
            key_order(&rule["properties"]),
            ["name", "rationale", "relevant", "holds", "violations"]
        );
        // `holds` is NOT unconditionally required — only `relevant` is.
        assert_eq!(rule["required"], json!(["name", "rationale", "relevant"]));
        assert_eq!(rule["properties"]["relevant"]["type"], "boolean");
        // ...but an if/then makes `holds` required once the rule is relevant.
        assert_eq!(rule["if"]["properties"]["relevant"]["const"], true);
        assert_eq!(rule["then"]["required"], json!(["holds"]));
    }

    #[test]
    fn relevance_without_rationale_still_gates_holds() {
        let s = build(&[with_relevance("gated", false)]);
        let rule = &s["properties"]["gated"];
        assert_eq!(
            key_order(&rule["properties"]),
            ["name", "relevant", "holds", "violations"]
        );
        assert_eq!(rule["required"], json!(["name", "relevant"]));
        assert_eq!(rule["then"]["required"], json!(["holds"]));
    }

    #[test]
    fn non_relevance_rules_have_no_if_then_gate() {
        let s = build(&[with("plain", true)]);
        let rule = &s["properties"]["plain"];
        assert!(rule.get("if").is_none());
        assert!(rule.get("then").is_none());
        assert!(rule["properties"].get("relevant").is_none());
    }

    #[test]
    fn require_line_attribution_marks_file_and_line_required_in_violations() {
        let s = build(&[with_attribution("located"), with("free", false)]);
        // The attribution rule's violation items require both file and line...
        let located = &s["properties"]["located"]["properties"]["violations"]["items"];
        assert_eq!(located["required"], json!(["file", "line"]));
        assert_eq!(located["properties"]["file"]["type"], "string");
        assert_eq!(located["properties"]["line"]["minimum"], 1);
        // ...while a rule without the flag keeps every field optional.
        let free = &s["properties"]["free"]["properties"]["violations"]["items"];
        assert!(free.get("required").is_none());
    }

    #[test]
    fn empty_rule_set_is_a_well_formed_object_schema() {
        let s = build(&[]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!([]));
        assert!(s["properties"].as_object().unwrap().is_empty());
    }
}
