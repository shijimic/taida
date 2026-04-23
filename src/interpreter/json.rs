/// std/json — JSON serialization/deserialization for Taida Lang.
///
/// Converts between Taida values and JSON strings.
/// Uses serde_json for parsing/generation.
///
/// JSON Molten Iron: JSON is an opaque primitive ("molten iron").
/// To use JSON data in Taida, it must be cast through a schema (TypeDef)
/// using `JSON[raw, Schema]()`. Direct manipulation of JSON values is prohibited.
use serde_json;

use crate::interpreter::value::Value;
use crate::parser::FieldDef;

/// Convert a serde_json::Value to a Taida Value (deep conversion).
/// Used only in tests; production code uses JSON[raw, Schema]() schema matching.
#[cfg(test)]
pub fn json_to_taida_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Str(String::new()), // No null in Taida
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Int(0)
            }
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(arr) => Value::list(arr.iter().map(json_to_taida_value).collect()),
        serde_json::Value::Object(obj) => {
            let fields: Vec<(String, Value)> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_taida_value(v)))
                .collect();
            Value::BuchiPack(fields)
        }
    }
}

/// Convert a Taida Value to a serde_json::Value.
///
/// C18-2 contract: Enum values (`Value::EnumVal(enum_name, ordinal)`) are
/// emitted as the variant name Str (e.g. `"Running"`). This makes
/// `jsonEncode` symmetric with the C16 `JSON[raw, Schema]()` decoder,
/// which accepts the variant-name Str wire format. `Value::Int` values
/// that are not tagged as EnumVal continue to emit as JSON numbers.
pub fn taida_value_to_json(val: &Value) -> serde_json::Value {
    match val {
        Value::Int(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::Float(n) => {
            if let Some(num) = serde_json::Number::from_f64(*n) {
                serde_json::Value::Number(num)
            } else {
                serde_json::Value::Null
            }
        }
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(taida_value_to_json).collect())
        }
        Value::BuchiPack(fields) => {
            let mut map = serde_json::Map::new();
            for (field_name, field_val) in fields {
                // Skip __type field — it's internal metadata, not user data
                if field_name == "__type" {
                    continue;
                }
                map.insert(field_name.clone(), taida_value_to_json(field_val));
            }
            serde_json::Value::Object(map)
        }
        Value::Unit => serde_json::Value::Object(serde_json::Map::new()),
        Value::Json(j) => j.clone(),
        // C18-2: Emit Enum as variant name Str. If the enum_name is not
        // registered (shouldn't happen — EnumVal is only produced by the
        // evaluator after enum_defs registration), fall back to the ordinal
        // Int for safety.
        Value::EnumVal(enum_name, ordinal) => {
            // The shared enum defs aren't accessible from this free function,
            // so this fallback path relies on the caller to pass an enriched
            // variant. See `taida_value_to_json_with_enum_defs` for the
            // lookup-aware version (used by stdlib_json_encode through the
            // `Interpreter`).
            let _ = enum_name;
            serde_json::Value::Number(serde_json::Number::from(*ordinal))
        }
        _ => serde_json::Value::Null,
    }
}

/// C18-2: Enrich `taida_value_to_json` with an `enum_defs` registry so
/// Enum values can be emitted as their declared variant-name Str. This is
/// the wire form used by `jsonEncode` and symmetric with the C16
/// `JSON[raw, Schema]()` decoder. See `src/interpreter/prelude.rs`
/// for the `jsonEncode` dispatch that routes through this function.
pub fn taida_value_to_json_with_enum_defs(
    val: &Value,
    enum_defs: &std::collections::HashMap<String, Vec<String>>,
) -> serde_json::Value {
    match val {
        Value::Int(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::Float(n) => {
            if let Some(num) = serde_json::Number::from_f64(*n) {
                serde_json::Value::Number(num)
            } else {
                serde_json::Value::Null
            }
        }
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::List(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| taida_value_to_json_with_enum_defs(item, enum_defs))
                .collect(),
        ),
        Value::BuchiPack(fields) => {
            let mut map = serde_json::Map::new();
            for (field_name, field_val) in fields {
                if field_name == "__type" {
                    continue;
                }
                map.insert(
                    field_name.clone(),
                    taida_value_to_json_with_enum_defs(field_val, enum_defs),
                );
            }
            serde_json::Value::Object(map)
        }
        Value::Unit => serde_json::Value::Object(serde_json::Map::new()),
        Value::Json(j) => j.clone(),
        Value::EnumVal(enum_name, ordinal) => {
            if let Some(variants) = enum_defs.get(enum_name)
                && let Some(variant_name) = variants.get(*ordinal as usize)
            {
                serde_json::Value::String(variant_name.clone())
            } else {
                // Enum not found in registry: defensive fallback to ordinal
                // Int. This path should only hit for values fabricated via
                // Rust-level Value construction outside the evaluator.
                serde_json::Value::Number(serde_json::Number::from(*ordinal))
            }
        }
        _ => serde_json::Value::Null,
    }
}

// ── JSON Molten Iron: Schema-based casting ─────────────────────────

/// Schema for JSON casting.
/// Describes the target type that JSON data should be cast into.
#[derive(Debug, Clone)]
pub enum JsonSchema {
    /// Primitive type: Int, Str, Float, Bool
    Primitive(PrimitiveType),
    /// TypeDef: named type with fields
    TypeDef(String, Vec<SchemaField>),
    /// List of a schema type: @[Schema]
    List(Box<JsonSchema>),
    /// Enum type: (enum_name, variant_names in ordinal order).
    /// JSON wire format is the variant name Str (e.g. `"Active"`).
    /// On match the variant's ordinal is returned as `Value::Int(ordinal)`.
    /// On mismatch/missing the caller is responsible for wrapping in `Lax[Enum]`.
    Enum(String, Vec<String>),
}

/// Primitive type for JSON schema matching.
#[derive(Debug, Clone)]
pub enum PrimitiveType {
    Int,
    Str,
    Float,
    Bool,
}

/// A field in a TypeDef schema.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    pub schema: JsonSchema,
}

/// Build a JsonSchema from a TypeDef's field definitions, resolving nested types.
/// `type_defs` maps type names to their field definitions.
/// `enum_defs` maps enum names to their variant names in ordinal order (C16).
pub fn build_schema_from_typedef(
    type_name: &str,
    fields: &[FieldDef],
    type_defs: &std::collections::HashMap<String, Vec<FieldDef>>,
    enum_defs: &std::collections::HashMap<String, Vec<String>>,
) -> JsonSchema {
    let schema_fields: Vec<SchemaField> = fields
        .iter()
        .filter(|f| !f.is_method)
        .map(|f| {
            let schema = match &f.type_annotation {
                Some(type_expr) => type_expr_to_schema(type_expr, type_defs, enum_defs),
                None => JsonSchema::Primitive(PrimitiveType::Str), // default to Str
            };
            SchemaField {
                name: f.name.clone(),
                schema,
            }
        })
        .collect();
    JsonSchema::TypeDef(type_name.to_string(), schema_fields)
}

/// Convert a TypeExpr to a JsonSchema.
///
/// Resolution order for `TypeExpr::Named`:
/// 1. Primitives (`Int`/`Str`/`Float`/`Bool`)
/// 2. `type_defs` (TypeDef — record-like schema)
/// 3. `enum_defs` (C16 — Enum variant set)
/// 4. Fallback to `Primitive(Str)` for unknown names
fn type_expr_to_schema(
    type_expr: &crate::parser::TypeExpr,
    type_defs: &std::collections::HashMap<String, Vec<FieldDef>>,
    enum_defs: &std::collections::HashMap<String, Vec<String>>,
) -> JsonSchema {
    match type_expr {
        crate::parser::TypeExpr::Named(name) => match name.as_str() {
            "Int" => JsonSchema::Primitive(PrimitiveType::Int),
            "Str" => JsonSchema::Primitive(PrimitiveType::Str),
            "Float" => JsonSchema::Primitive(PrimitiveType::Float),
            "Bool" => JsonSchema::Primitive(PrimitiveType::Bool),
            other => {
                // C16: TypeDef wins over Enum when both exist (Taida disallows
                // collision today; kept explicit for future safety).
                if let Some(fields) = type_defs.get(other) {
                    build_schema_from_typedef(other, fields, type_defs, enum_defs)
                } else if let Some(variants) = enum_defs.get(other) {
                    JsonSchema::Enum(other.to_string(), variants.clone())
                } else {
                    // Unknown type: default to Str
                    JsonSchema::Primitive(PrimitiveType::Str)
                }
            }
        },
        crate::parser::TypeExpr::List(inner) => {
            JsonSchema::List(Box::new(type_expr_to_schema(inner, type_defs, enum_defs)))
        }
        crate::parser::TypeExpr::BuchiPack(fields) => {
            // Inline buchi pack type: @(field: Type, ...)
            let schema_fields: Vec<SchemaField> = fields
                .iter()
                .filter(|f| !f.is_method)
                .map(|f| {
                    let schema = match &f.type_annotation {
                        Some(te) => type_expr_to_schema(te, type_defs, enum_defs),
                        None => JsonSchema::Primitive(PrimitiveType::Str),
                    };
                    SchemaField {
                        name: f.name.clone(),
                        schema,
                    }
                })
                .collect();
            JsonSchema::TypeDef("BuchiPack".to_string(), schema_fields)
        }
        _ => JsonSchema::Primitive(PrimitiveType::Str),
    }
}

/// Cast a JSON value into a Taida value according to a schema.
///
/// Schema matching rules:
/// 1. Field match: only schema fields are extracted, extras ignored
/// 2. Missing field: default value for the field's type
/// 3. Type mismatch: default value
/// 4. null: default value (null exclusion philosophy)
/// 5. Nested: recursive matching
/// 6. List: each element matched against element schema
pub fn json_to_typed_value(json: &serde_json::Value, schema: &JsonSchema) -> Value {
    match schema {
        JsonSchema::Primitive(prim) => json_to_primitive(json, prim),
        JsonSchema::TypeDef(type_name, schema_fields) => {
            match json {
                serde_json::Value::Object(obj) => {
                    let mut fields: Vec<(String, Value)> = Vec::new();
                    for sf in schema_fields {
                        let value = if let Some(json_val) = obj.get(&sf.name) {
                            if json_val.is_null() {
                                field_missing_default(&sf.schema)
                            } else {
                                json_to_typed_value(json_val, &sf.schema)
                            }
                        } else {
                            field_missing_default(&sf.schema)
                        };
                        fields.push((sf.name.clone(), value));
                    }
                    fields.push(("__type".to_string(), Value::Str(type_name.clone())));
                    Value::BuchiPack(fields)
                }
                serde_json::Value::Null => {
                    // null -> all defaults
                    let mut fields: Vec<(String, Value)> = Vec::new();
                    for sf in schema_fields {
                        fields.push((sf.name.clone(), field_missing_default(&sf.schema)));
                    }
                    fields.push(("__type".to_string(), Value::Str(type_name.clone())));
                    Value::BuchiPack(fields)
                }
                _ => {
                    // Non-object -> all defaults
                    let mut fields: Vec<(String, Value)> = Vec::new();
                    for sf in schema_fields {
                        fields.push((sf.name.clone(), field_missing_default(&sf.schema)));
                    }
                    fields.push(("__type".to_string(), Value::Str(type_name.clone())));
                    Value::BuchiPack(fields)
                }
            }
        }
        JsonSchema::List(elem_schema) => match json {
            serde_json::Value::Array(arr) => {
                let items: Vec<Value> = arr
                    .iter()
                    .map(|elem| json_to_typed_value(elem, elem_schema))
                    .collect();
                Value::list(items)
            }
            serde_json::Value::Null => Value::list(Vec::new()),
            _ => Value::list(Vec::new()),
        },
        JsonSchema::Enum(name, variants) => {
            // C16: JSON wire format is the variant name Str.
            // C18-2: On match, return `Value::EnumVal(enum_name, ordinal)`
            // (was `Value::Int(ordinal)` in C16) so that the round-trip
            // through `jsonEncode` emits the variant-name Str rather than
            // the ordinal Int. Downstream `PartialEq` accepts both forms
            // for backward compat — see `src/interpreter/value.rs`.
            match json {
                serde_json::Value::String(s) => {
                    if let Some(ordinal) = variants.iter().position(|v| v == s) {
                        Value::EnumVal(name.clone(), ordinal as i64)
                    } else {
                        make_lax_enum_inline()
                    }
                }
                _ => make_lax_enum_inline(),
            }
        }
    }
}

/// C16: Default value for a JSON schema field whose key is missing / null.
///
/// For most schemas this is identical to `default_for_schema` (Int(0), Str(""),
/// nested defaults, etc.). For `JsonSchema::Enum` we diverge: a missing Enum
/// field returns `Lax[Enum]` (silent coercion禁止) so the caller is forced to
/// acknowledge the boundary via `|==` / `getOrDefault`.
fn field_missing_default(schema: &JsonSchema) -> Value {
    match schema {
        JsonSchema::Enum(_, _) => make_lax_enum_inline(),
        // Nested TypeDef: recurse so inner Enum fields get Lax, not Int(0).
        JsonSchema::TypeDef(type_name, schema_fields) => {
            let mut result_fields: Vec<(String, Value)> = schema_fields
                .iter()
                .map(|f| (f.name.clone(), field_missing_default(&f.schema)))
                .collect();
            result_fields.push(("__type".to_string(), Value::Str(type_name.clone())));
            Value::BuchiPack(result_fields)
        }
        _ => default_for_schema(schema),
    }
}

/// C16: Lax[Enum] shape for JSON mold Enum validation failure.
///
/// Kept identical to `mold_eval::make_lax_value(false, Int(0), Int(0))` so that
/// the 3-backend parity can be verified structurally:
///   @(hasValue=false, __value=Int(0), __default=Int(0), __type="Lax")
///
/// `Int(0)` encodes the first variant's ordinal — Taida's "最初のバリアント = デフォルト"
/// rule (`docs/guide/01_types.md:609`) is preserved as the Lax fallback.
fn make_lax_enum_inline() -> Value {
    Value::BuchiPack(vec![
        ("hasValue".to_string(), Value::Bool(false)),
        ("__value".to_string(), Value::Int(0)),
        ("__default".to_string(), Value::Int(0)),
        ("__type".to_string(), Value::Str("Lax".to_string())),
    ])
}

/// Convert a JSON value to a primitive Taida value.
///
/// Philosophy I: "null/undefined の完全排除 — 全ての型にデフォルト値を保証"
/// Parse failures and type mismatches silently fall back to the type's default
/// value (0, 0.0, "", false). This is intentional per Taida's null-exclusion
/// philosophy, though it means parse errors are indistinguishable from legitimate
/// zero/empty values.
fn json_to_primitive(json: &serde_json::Value, prim: &PrimitiveType) -> Value {
    match prim {
        PrimitiveType::Int => match json {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Value::Int(f as i64)
                } else {
                    Value::Int(0)
                }
            }
            serde_json::Value::String(s) => {
                s.parse::<i64>().map(Value::Int).unwrap_or(Value::Int(0))
            }
            serde_json::Value::Bool(b) => Value::Int(if *b { 1 } else { 0 }),
            _ => Value::Int(0),
        },
        PrimitiveType::Float => match json {
            serde_json::Value::Number(n) => Value::Float(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => s
                .parse::<f64>()
                .map(Value::Float)
                .unwrap_or(Value::Float(0.0)),
            serde_json::Value::Bool(b) => Value::Float(if *b { 1.0 } else { 0.0 }),
            _ => Value::Float(0.0),
        },
        PrimitiveType::Str => match json {
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Number(n) => Value::Str(format!("{}", n)),
            serde_json::Value::Bool(b) => Value::Str(format!("{}", b)),
            serde_json::Value::Null => Value::Str(String::new()),
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                Value::Str(serde_json::to_string(json).unwrap_or_default())
            }
        },
        PrimitiveType::Bool => match json {
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Bool(n.as_f64().is_some_and(|f| f != 0.0)),
            serde_json::Value::String(s) => Value::Bool(!s.is_empty()),
            serde_json::Value::Null => Value::Bool(false),
            _ => Value::Bool(false),
        },
    }
}

/// Get the default value for a schema type.
pub fn default_for_schema(schema: &JsonSchema) -> Value {
    match schema {
        JsonSchema::Primitive(PrimitiveType::Int) => Value::Int(0),
        JsonSchema::Primitive(PrimitiveType::Float) => Value::Float(0.0),
        JsonSchema::Primitive(PrimitiveType::Str) => Value::Str(String::new()),
        JsonSchema::Primitive(PrimitiveType::Bool) => Value::Bool(false),
        JsonSchema::TypeDef(type_name, fields) => {
            let mut result_fields: Vec<(String, Value)> = fields
                .iter()
                .map(|f| (f.name.clone(), default_for_schema(&f.schema)))
                .collect();
            result_fields.push(("__type".to_string(), Value::Str(type_name.clone())));
            Value::BuchiPack(result_fields)
        }
        JsonSchema::List(_) => Value::list(Vec::new()),
        // C16: Enum default is the first variant's ordinal (= Int(0)).
        // This matches Taida's "最初のバリアントがデフォルト" rule.
        JsonSchema::Enum(_, _) => Value::Int(0),
    }
}

/// jsonEncode(value) -> Str
/// Converts a Taida value to a compact JSON string.
///
/// C18-2: This legacy function does NOT use the enum-aware variant-name
/// encoder because it has no access to `enum_defs`. The prelude dispatcher
/// in `src/interpreter/prelude.rs` routes `jsonEncode` through the
/// enum-aware path (`taida_value_to_json_with_enum_defs`). This
/// function is retained for the standalone test suite and any caller
/// that does not have an `Interpreter` instance.
#[cfg(test)]
pub fn stdlib_json_encode(args: &[Value]) -> Result<Value, String> {
    let val = args.first().unwrap_or(&Value::Unit);
    match val {
        Value::Json(j) => Ok(Value::Str(serde_json::to_string(j).unwrap_or_default())),
        _ => {
            let json = taida_value_to_json(val);
            Ok(Value::Str(serde_json::to_string(&json).unwrap_or_default()))
        }
    }
}

/// jsonPretty(value) -> Str — see `stdlib_json_encode` for the C18-2
/// variant-name contract. Retained for tests.
#[cfg(test)]
pub fn stdlib_json_pretty(args: &[Value]) -> Result<Value, String> {
    let val = args.first().unwrap_or(&Value::Unit);
    let json = taida_value_to_json(val);
    Ok(Value::Str(
        serde_json::to_string_pretty(&json).unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_encode_buchi_pack() {
        let pack = Value::BuchiPack(vec![
            ("name".to_string(), Value::Str("Alice".to_string())),
            ("age".to_string(), Value::Int(30)),
        ]);
        let result = stdlib_json_encode(&[pack]).unwrap();
        let Value::Str(s) = result else {
            unreachable!("Expected Str from json_encode");
        };
        assert!(s.contains("\"name\":\"Alice\""));
        assert!(s.contains("\"age\":30"));
    }

    #[test]
    fn test_json_pretty() {
        let pack = Value::BuchiPack(vec![("x".to_string(), Value::Int(1))]);
        let result = stdlib_json_pretty(&[pack]).unwrap();
        let Value::Str(s) = result else {
            unreachable!("Expected Str from json_pretty");
        };
        assert!(s.contains('\n')); // Pretty printed should have newlines
        assert!(s.contains("\"x\": 1"));
    }

    #[test]
    fn test_json_encode_accepts_json_value() {
        let json = Value::Json(serde_json::json!({"x": 42}));
        let result = stdlib_json_encode(&[json]).unwrap();
        let Value::Str(s) = result else {
            unreachable!("Expected Str from json_encode");
        };
        assert!(s.contains("\"x\":42") || s.contains("\"x\": 42"));
    }

    #[test]
    fn test_json_to_taida_value_deep_conversion() {
        let json = serde_json::json!({
            "name": "Alice",
            "scores": [100, 95],
            "active": true,
            "data": null
        });
        let result = json_to_taida_value(&json);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from json_to_taida_value");
        };
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "name" && *v == Value::Str("Alice".to_string()))
        );
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "data" && *v == Value::Str(String::new()))
        );
    }

    #[test]
    fn test_taida_value_to_json_with_json_value() {
        let json_val = serde_json::json!({"a": 1});
        let val = Value::Json(json_val.clone());
        let result = taida_value_to_json(&val);
        assert_eq!(result, json_val);
    }

    // ── Molten Iron: schema-based casting tests ─────────────

    #[test]
    fn test_schema_primitive_int() {
        let schema = JsonSchema::Primitive(PrimitiveType::Int);
        assert_eq!(
            json_to_typed_value(&serde_json::json!(42), &schema),
            Value::Int(42)
        );
        assert_eq!(
            json_to_typed_value(&serde_json::json!("not a number"), &schema),
            Value::Int(0)
        );
        assert_eq!(
            json_to_typed_value(&serde_json::json!(null), &schema),
            Value::Int(0)
        );
    }

    #[test]
    fn test_schema_primitive_str() {
        let schema = JsonSchema::Primitive(PrimitiveType::Str);
        assert_eq!(
            json_to_typed_value(&serde_json::json!("hello"), &schema),
            Value::Str("hello".to_string())
        );
        assert_eq!(
            json_to_typed_value(&serde_json::json!(null), &schema),
            Value::Str(String::new())
        );
    }

    #[test]
    fn test_schema_typedef_basic() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "age".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Int),
                },
            ],
        );
        let json = serde_json::json!({"name": "Asuka", "age": 14, "extra": "ignored"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from schema typedef");
        };
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "name" && *v == Value::Str("Asuka".to_string()))
        );
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "age" && *v == Value::Int(14))
        );
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "__type" && *v == Value::Str("User".to_string()))
        );
        // "extra" should NOT be present
        assert!(!fields.iter().any(|(k, _)| k == "extra"));
    }

    #[test]
    fn test_schema_missing_fields_get_defaults() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "age".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Int),
                },
                SchemaField {
                    name: "email".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
            ],
        );
        let json = serde_json::json!({"name": "Asuka"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from schema missing fields");
        };
        assert_eq!(
            fields.iter().find(|(k, _)| k == "age").unwrap().1,
            Value::Int(0)
        );
        assert_eq!(
            fields.iter().find(|(k, _)| k == "email").unwrap().1,
            Value::Str(String::new())
        );
    }

    #[test]
    fn test_schema_type_mismatch_defaults() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![SchemaField {
                name: "age".to_string(),
                schema: JsonSchema::Primitive(PrimitiveType::Int),
            }],
        );
        let json = serde_json::json!({"age": "not a number"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from schema type mismatch");
        };
        assert_eq!(
            fields.iter().find(|(k, _)| k == "age").unwrap().1,
            Value::Int(0)
        );
    }

    #[test]
    fn test_schema_null_to_defaults() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "age".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Int),
                },
            ],
        );
        let json = serde_json::json!({"name": null, "age": null});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from schema null defaults");
        };
        assert_eq!(
            fields.iter().find(|(k, _)| k == "name").unwrap().1,
            Value::Str(String::new())
        );
        assert_eq!(
            fields.iter().find(|(k, _)| k == "age").unwrap().1,
            Value::Int(0)
        );
    }

    #[test]
    fn test_schema_nested_typedef() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "address".to_string(),
                    schema: JsonSchema::TypeDef(
                        "Address".to_string(),
                        vec![
                            SchemaField {
                                name: "city".to_string(),
                                schema: JsonSchema::Primitive(PrimitiveType::Str),
                            },
                            SchemaField {
                                name: "zip".to_string(),
                                schema: JsonSchema::Primitive(PrimitiveType::Str),
                            },
                        ],
                    ),
                },
            ],
        );
        let json =
            serde_json::json!({"name": "Asuka", "address": {"city": "Tokyo-3", "zip": "999"}});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("Expected BuchiPack from nested schema");
        };
        let Some((_, Value::BuchiPack(addr_fields))) = fields.iter().find(|(k, _)| k == "address")
        else {
            unreachable!("Expected address to be BuchiPack");
        };
        assert!(
            addr_fields
                .iter()
                .any(|(k, v)| k == "city" && *v == Value::Str("Tokyo-3".to_string()))
        );
    }

    #[test]
    fn test_schema_list() {
        let schema = JsonSchema::List(Box::new(JsonSchema::TypeDef(
            "Pilot".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "syncRate".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Int),
                },
            ],
        )));
        let json = serde_json::json!([
            {"name": "Asuka", "syncRate": 78},
            {"name": "Rei", "syncRate": 65}
        ]);
        let result = json_to_typed_value(&json, &schema);
        let Value::List(items) = result else {
            unreachable!("Expected List from schema list");
        };
        assert_eq!(items.len(), 2);
        if let Value::BuchiPack(fields) = &items[0] {
            assert!(
                fields
                    .iter()
                    .any(|(k, v)| k == "name" && *v == Value::Str("Asuka".to_string()))
            );
        }
    }

    #[test]
    fn test_default_for_schema() {
        assert_eq!(
            default_for_schema(&JsonSchema::Primitive(PrimitiveType::Int)),
            Value::Int(0)
        );
        assert_eq!(
            default_for_schema(&JsonSchema::Primitive(PrimitiveType::Str)),
            Value::Str(String::new())
        );
        assert_eq!(
            default_for_schema(&JsonSchema::List(Box::new(JsonSchema::Primitive(
                PrimitiveType::Int
            )))),
            Value::list(Vec::new())
        );
    }

    // ── C16: Enum schema tests ─────────────────────────

    fn enum_status_schema() -> JsonSchema {
        JsonSchema::Enum(
            "Status".to_string(),
            vec![
                "Active".to_string(),
                "Inactive".to_string(),
                "Pending".to_string(),
            ],
        )
    }

    fn is_lax_enum(value: &Value) -> bool {
        let Value::BuchiPack(fields) = value else {
            return false;
        };
        let has_value = fields
            .iter()
            .find(|(k, _)| k == "hasValue")
            .map(|(_, v)| v == &Value::Bool(false))
            .unwrap_or(false);
        let inner_value = fields
            .iter()
            .find(|(k, _)| k == "__value")
            .map(|(_, v)| v == &Value::Int(0))
            .unwrap_or(false);
        let default = fields
            .iter()
            .find(|(k, _)| k == "__default")
            .map(|(_, v)| v == &Value::Int(0))
            .unwrap_or(false);
        let tag = fields
            .iter()
            .find(|(k, _)| k == "__type")
            .map(|(_, v)| v == &Value::Str("Lax".to_string()))
            .unwrap_or(false);
        has_value && inner_value && default && tag
    }

    #[test]
    fn test_c16_enum_variant_match_returns_ordinal() {
        let schema = enum_status_schema();
        assert_eq!(
            json_to_typed_value(&serde_json::json!("Active"), &schema),
            Value::Int(0)
        );
        assert_eq!(
            json_to_typed_value(&serde_json::json!("Inactive"), &schema),
            Value::Int(1)
        );
        assert_eq!(
            json_to_typed_value(&serde_json::json!("Pending"), &schema),
            Value::Int(2)
        );
    }

    #[test]
    fn test_c16_enum_mismatch_returns_lax() {
        let schema = enum_status_schema();
        let result = json_to_typed_value(&serde_json::json!("Bogus"), &schema);
        assert!(
            is_lax_enum(&result),
            "expected Lax[Enum] for mismatched variant, got {:?}",
            result
        );
    }

    #[test]
    fn test_c16_enum_non_string_returns_lax() {
        let schema = enum_status_schema();
        for json in [
            serde_json::json!(0),
            serde_json::json!(1.5),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let result = json_to_typed_value(&json, &schema);
            assert!(
                is_lax_enum(&result),
                "expected Lax[Enum] for non-string JSON {:?}, got {:?}",
                json,
                result
            );
        }
    }

    #[test]
    fn test_c16_enum_field_in_typedef_match() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "status".to_string(),
                    schema: enum_status_schema(),
                },
            ],
        );
        let json = serde_json::json!({"name": "Alice", "status": "Pending"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("expected BuchiPack, got {:?}", result);
        };
        assert_eq!(
            fields.iter().find(|(k, _)| k == "status").unwrap().1,
            Value::Int(2)
        );
    }

    #[test]
    fn test_c16_enum_field_in_typedef_mismatch_yields_lax() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        let json = serde_json::json!({"status": "Unknown"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("expected BuchiPack, got {:?}", result);
        };
        let status = &fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert!(
            is_lax_enum(status),
            "expected Lax[Enum] for mismatched field, got {:?}",
            status
        );
    }

    #[test]
    fn test_c16_enum_field_missing_yields_lax() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        // key missing
        let json = serde_json::json!({"name": "no status"});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("expected BuchiPack, got {:?}", result);
        };
        let status = &fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert!(
            is_lax_enum(status),
            "expected Lax[Enum] for missing field, got {:?}",
            status
        );
    }

    #[test]
    fn test_c16_enum_field_null_yields_lax() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        let json = serde_json::json!({"status": null});
        let result = json_to_typed_value(&json, &schema);
        let Value::BuchiPack(fields) = result else {
            unreachable!("expected BuchiPack, got {:?}", result);
        };
        let status = &fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert!(
            is_lax_enum(status),
            "expected Lax[Enum] for null field, got {:?}",
            status
        );
    }

    #[test]
    fn test_c16_enum_nested_in_typedef_in_typedef() {
        // Wrapper { meta: Info { status: Enum } }
        let info = JsonSchema::TypeDef(
            "Info".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        let wrapper = JsonSchema::TypeDef(
            "Wrapper".to_string(),
            vec![SchemaField {
                name: "meta".to_string(),
                schema: info,
            }],
        );
        // Outer missing → nested Enum must still be Lax.
        let json = serde_json::json!({});
        let result = json_to_typed_value(&json, &wrapper);
        let Value::BuchiPack(fields) = result else {
            unreachable!("expected BuchiPack, got {:?}", result);
        };
        let Value::BuchiPack(meta_fields) =
            &fields.iter().find(|(k, _)| k == "meta").unwrap().1.clone()
        else {
            unreachable!("expected meta to be BuchiPack");
        };
        let status = &meta_fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert!(
            is_lax_enum(status),
            "expected Lax[Enum] for deeply-nested missing field, got {:?}",
            status
        );
    }

    #[test]
    fn test_c16_default_for_schema_enum_is_first_ordinal() {
        let schema = enum_status_schema();
        // Top-level Enum __default stays Int(0) to preserve 最初のバリアント rule.
        assert_eq!(default_for_schema(&schema), Value::Int(0));
    }

    // ── C16B-001 regression: TypeDef default for Enum field ─────────
    //
    // The parse-error path (outer Lax.__value / __default) must carry a
    // TypeDef whose Enum field is `Int(0)` — NOT `Lax[Enum]`. `Lax[Enum]`
    // is reserved for actual validation failures (mismatch / missing key /
    // null field). This pins Interpreter as the reference; Native's
    // `json_default_value_for_desc('T')` and JS's `__taida_defaultForSchema`
    // are expected to match.

    #[test]
    fn test_c16b001_default_for_typedef_enum_field_is_int_not_lax() {
        let schema = JsonSchema::TypeDef(
            "User".to_string(),
            vec![
                SchemaField {
                    name: "name".to_string(),
                    schema: JsonSchema::Primitive(PrimitiveType::Str),
                },
                SchemaField {
                    name: "status".to_string(),
                    schema: enum_status_schema(),
                },
            ],
        );
        let Value::BuchiPack(fields) = default_for_schema(&schema) else {
            unreachable!("expected BuchiPack default for TypeDef");
        };
        let status = &fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert_eq!(
            status,
            &Value::Int(0),
            "TypeDef default must embed Int(0) for Enum fields (not Lax)"
        );
        let name = &fields.iter().find(|(k, _)| k == "name").unwrap().1;
        assert_eq!(name, &Value::Str(String::new()));
    }

    #[test]
    fn test_c16b001_default_for_nested_typedef_enum_field_is_int_not_lax() {
        let inner = JsonSchema::TypeDef(
            "Info".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        let outer = JsonSchema::TypeDef(
            "Wrapper".to_string(),
            vec![SchemaField {
                name: "meta".to_string(),
                schema: inner,
            }],
        );
        let Value::BuchiPack(fields) = default_for_schema(&outer) else {
            unreachable!("expected BuchiPack default for outer TypeDef");
        };
        let Value::BuchiPack(meta_fields) =
            &fields.iter().find(|(k, _)| k == "meta").unwrap().1.clone()
        else {
            unreachable!("expected meta to be BuchiPack");
        };
        let status = &meta_fields.iter().find(|(k, _)| k == "status").unwrap().1;
        assert_eq!(
            status,
            &Value::Int(0),
            "Nested TypeDef default must embed Int(0) for Enum fields (not Lax)"
        );
    }

    #[test]
    fn test_c16b001_default_for_list_of_typedef_enum_is_empty_list() {
        let user = JsonSchema::TypeDef(
            "User".to_string(),
            vec![SchemaField {
                name: "status".to_string(),
                schema: enum_status_schema(),
            }],
        );
        let list = JsonSchema::List(Box::new(user));
        assert_eq!(default_for_schema(&list), Value::list(Vec::new()));
    }
}
