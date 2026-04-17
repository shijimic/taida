/// Runtime values for Taida Lang.
///
/// Every type has a default value. There is no null/undefined.
/// All values are immutable — operations return new values.
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::parser::{Param, Statement};

/// A runtime value in Taida.
#[derive(Debug, Clone)]
pub enum Value {
    /// Integer value
    Int(i64),
    /// Floating-point value
    Float(f64),
    /// String value
    Str(String),
    /// Bytes value (immutable byte sequence)
    Bytes(Vec<u8>),
    /// Boolean value
    Bool(bool),
    /// Buchi pack (named fields, ordered)
    BuchiPack(Vec<(String, Value)>),
    /// List of values
    List(Vec<Value>),
    /// Function closure
    Function(FuncValue),
    /// Gorilla — immediate program termination
    Gorilla,
    /// Unit value (empty buchi pack)
    Unit,
    /// Error value (for throw/catch via error ceiling)
    Error(ErrorValue),
    /// Async value — Mold[T] for asynchronous operations
    Async(AsyncValue),
    /// JSON value — 外部データの型安全なエアロック
    Json(serde_json::Value),
    /// Molten value — opaque primitive for external (JS) interop data.
    /// No methods, no field access. Only usable inside Cage.
    Molten,
    /// Stream value — time-series mold type. Values flow over time.
    /// `]=>` collects all values into `@[T]` (blocking).
    Stream(StreamValue),
    /// C18-2 / C18-3: Tagged enum value — the ordinal carries its owning
    /// enum name so that `jsonEncode` can emit the variant name Str and
    /// `Ordinal[]` can assert the argument came from an Enum variant.
    ///
    /// Interop with `Value::Int(ordinal)`:
    /// - equality: `Int(n) == EnumVal(_, n)` (same ordinal matches).
    /// - ordering: same-enum `EnumVal` pairs compare by ordinal; cross-
    ///   enum or Enum↔Int ordering still falls through to `None`.
    /// - arithmetic: addition / subtraction treat EnumVal like Int(n).
    /// - `.toString()` / `to_display_string()` returns the ordinal as a
    ///   Str, preserving the C16 contract (the variant-name representation
    ///   is only used by jsonEncode, not by display).
    ///
    /// This is strictly additive: every existing code path that produces
    /// `Value::Int(ordinal)` for an Enum continues to work. Code paths
    /// that need the enum identity (Phase 2 jsonEncode, Phase 3 Ordinal[],
    /// Phase 4 ordering) use pattern-match on `EnumVal`.
    EnumVal(String, i64),
}

/// A function closure.
#[derive(Debug, Clone)]
pub struct FuncValue {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Statement>,
    /// Captured environment (lexical scope closure).
    /// Shared to avoid recursive deep-clone blow-up when many functions capture prior functions.
    pub closure: Arc<HashMap<String, Value>>,
    /// RCB-242: Declared return type from function definition (if any).
    /// Used for introspection. Runtime type enforcement is handled by the checker.
    pub return_type: Option<crate::parser::TypeExpr>,
}

/// An error value.
#[derive(Debug, Clone)]
pub struct ErrorValue {
    pub error_type: String,
    pub message: String,
    pub fields: Vec<(String, Value)>,
}

/// Async status.
#[derive(Debug, Clone, PartialEq)]
pub enum AsyncStatus {
    Pending,
    Fulfilled,
    Rejected,
}

/// Internal state for a pending async task.
/// Shared via Arc<Mutex<...>> so that AsyncValue can be Clone.
#[derive(Debug)]
pub enum PendingState {
    /// Waiting for a result from a tokio task.
    Waiting(tokio::sync::oneshot::Receiver<Result<Value, String>>),
    /// The task completed and its result has been consumed.
    Done,
}

/// An async value — Mold[T] for asynchronous operations.
///
/// Two modes:
/// - **Resolved** (status = Fulfilled | Rejected): value/error set immediately.
///   This is backward-compatible with the original synchronous simulation.
/// - **Pending** (status = Pending, task = Some(...)): a real tokio task is running.
///   When `]=>` is used, the interpreter calls `block_on` to wait for the result.
#[derive(Debug, Clone)]
pub struct AsyncValue {
    pub status: AsyncStatus,
    /// The resolved value (when Fulfilled)
    pub value: Box<Value>,
    /// The error (when Rejected)
    pub error: Box<Value>,
    /// Handle to a pending tokio task. None for immediately resolved values.
    /// Wrapped in Arc<Mutex<>> so AsyncValue can be Clone.
    pub task: Option<Arc<Mutex<PendingState>>>,
}

/// Stream status.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamStatus {
    Active,
    Completed,
}

/// A stream transform operation — stored as a deferred computation.
#[derive(Debug, Clone)]
pub enum StreamTransform {
    /// Map: apply function to each element
    Map(FuncValue),
    /// Filter: keep only elements where function returns true
    Filter(FuncValue),
    /// Take: take at most N elements
    Take(usize),
    /// TakeWhile: take while predicate returns true
    TakeWhile(FuncValue),
}

/// A stream value — Mold[@[T]] for time-series data.
///
/// Stream[T] holds source items and a chain of lazy transforms.
/// When `]=>` is used, the transforms are applied and items collected into a list.
/// PHILOSOPHY.md III: カタめたいなら、鋳型を作りましょう
#[derive(Debug, Clone)]
pub struct StreamValue {
    /// Source items (already materialized for sync mode)
    pub items: Vec<Value>,
    /// Chain of lazy transforms to apply on collect
    pub transforms: Vec<StreamTransform>,
    /// Stream status
    pub status: StreamStatus,
}

impl Value {
    /// Default value for Int.
    pub fn default_int() -> Self {
        Value::Int(0)
    }

    /// Default value for Float.
    pub fn default_float() -> Self {
        Value::Float(0.0)
    }

    /// Default value for Str.
    pub fn default_str() -> Self {
        Value::Str(String::new())
    }

    /// Default value for Bytes.
    pub fn default_bytes() -> Self {
        Value::Bytes(Vec::new())
    }

    /// Default value for Bool.
    pub fn default_bool() -> Self {
        Value::Bool(false)
    }

    /// Default value for List.
    pub fn default_list() -> Self {
        Value::List(Vec::new())
    }

    /// Default value for BuchiPack (empty).
    pub fn default_buchi() -> Self {
        Value::BuchiPack(Vec::new())
    }

    /// Default value for JSON (empty object `{}`).
    pub fn default_json() -> Self {
        Value::Json(serde_json::Value::Object(serde_json::Map::new()))
    }

    /// Default value for Molten (empty molten iron).
    pub fn default_molten() -> Self {
        Value::Molten
    }

    /// Default value for Stream (empty completed stream).
    pub fn default_stream() -> Self {
        Value::Stream(StreamValue {
            items: Vec::new(),
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        })
    }

    /// Infer the default value from a list's element type.
    /// Looks at the first element to determine what type the list holds,
    /// then returns the appropriate default for that type.
    /// PHILOSOPHY.md I: all types must have default values.
    pub fn default_for_list(items: &[Value]) -> Value {
        match items.first() {
            Some(Value::Int(_)) => Value::Int(0),
            Some(Value::Float(_)) => Value::Float(0.0),
            Some(Value::Str(_)) => Value::Str(String::new()),
            Some(Value::Bytes(_)) => Value::Bytes(Vec::new()),
            Some(Value::Bool(_)) => Value::Bool(false),
            Some(Value::BuchiPack(_)) => Value::BuchiPack(Vec::new()),
            Some(Value::List(_)) => Value::List(Vec::new()),
            Some(Value::Json(_)) => Value::default_json(),
            Some(Value::Molten) => Value::Molten,
            Some(Value::Stream(_)) => Value::default_stream(),
            Some(Value::Unit) => Value::Unit,
            _ => Value::Int(0), // empty list fallback
        }
    }

    /// Check if this value is truthy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::BuchiPack(_) => true,
            Value::Function(_) => true,
            Value::Unit => false,
            Value::Gorilla => false,
            Value::Error(_) => true,
            Value::Molten => false,
            Value::Stream(s) => !s.items.is_empty() || s.status == StreamStatus::Active,
            Value::Async(a) => a.status == AsyncStatus::Fulfilled,
            Value::Json(j) => match j {
                serde_json::Value::Null => false,
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
                serde_json::Value::String(s) => !s.is_empty(),
                serde_json::Value::Array(a) => !a.is_empty(),
                serde_json::Value::Object(o) => !o.is_empty(),
            },
            // C18-2: EnumVal shares Int(ordinal) truthiness: the first
            // variant (ordinal 0) is falsy, every other variant is truthy.
            Value::EnumVal(_, n) => *n != 0,
        }
    }

    /// Get a field from a buchi pack or error value.
    pub fn get_field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::BuchiPack(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            Value::Error(err) => {
                // Check built-in fields first
                if name == "type" {
                    return None; // Handled specially
                }
                if name == "message" {
                    return None; // Handled specially
                }
                err.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// Get a field from an error, including built-in fields.
    pub fn get_error_field(&self, name: &str) -> Option<Value> {
        if let Value::Error(err) = self {
            match name {
                "type" => Some(Value::Str(err.error_type.clone())),
                "message" => Some(Value::Str(err.message.clone())),
                _ => err
                    .fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone()),
            }
        } else {
            None
        }
    }

    /// Convert to string representation.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Float(n) => {
                if *n == n.floor() && n.is_finite() {
                    format!("{:.1}", n)
                } else {
                    n.to_string()
                }
            }
            Value::Str(s) => s.clone(),
            Value::Bytes(bytes) => {
                let elems: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                format!("Bytes[@[{}]]", elems.join(", "))
            }
            Value::Bool(b) => b.to_string(),
            Value::BuchiPack(fields) => {
                let mut s = String::from("@(");
                for (i, (name, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{} <= {}", name, val.to_debug_string()));
                }
                s.push(')');
                s
            }
            Value::List(items) => {
                let mut s = String::from("@[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&item.to_debug_string());
                }
                s.push(']');
                s
            }
            Value::Function(f) => format!("<function {}>", f.name),
            Value::Gorilla => "><".to_string(),
            Value::Unit => "@()".to_string(),
            Value::Error(err) => format!("Error({}: {})", err.error_type, err.message),
            Value::Async(a) => match a.status {
                AsyncStatus::Fulfilled => {
                    format!("Async[fulfilled: {}]", a.value.to_display_string())
                }
                AsyncStatus::Rejected => {
                    format!("Async[rejected: {}]", a.error.to_display_string())
                }
                AsyncStatus::Pending => {
                    if a.task.is_some() {
                        "Async[pending (task)]".to_string()
                    } else {
                        "Async[pending]".to_string()
                    }
                }
            },
            Value::Json(j) => serde_json::to_string(j).unwrap_or_default(),
            Value::Molten => "Molten".to_string(),
            Value::Stream(s) => match s.status {
                StreamStatus::Active => "Stream[active]".to_string(),
                StreamStatus::Completed => format!("Stream[completed: {} items]", s.items.len()),
            },
            // C18-2: `.toString()` and display preserve the ordinal Str
            // contract (`docs/guide/01_types.md:609` and ROOT-4 in
            // `.dev/C18_BLOCKERS.md`). jsonEncode uses a dedicated path
            // to emit the variant name.
            Value::EnumVal(_, n) => n.to_string(),
        }
    }

    /// Convert to debug string (with quotes around strings).
    pub fn to_debug_string(&self) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s),
            other => other.to_display_string(),
        }
    }

    /// SEC-007: Truncated display for error messages.
    /// Limits output to approximately `max_len` characters to prevent
    /// large data structures from producing multi-megabyte error messages.
    pub fn to_error_display(&self, max_len: usize) -> String {
        let full = self.to_display_string();
        if full.len() <= max_len {
            return full;
        }
        let suffix = match self {
            Value::List(items) => format!("... ({} items)]", items.len()),
            Value::BuchiPack(fields) => format!("... ({} fields))", fields.len()),
            _ => "...".to_string(),
        };
        let truncate_at = max_len.saturating_sub(suffix.len());
        // Find a safe truncation point (don't split multi-byte chars)
        let mut safe = truncate_at;
        while safe > 0 && !full.is_char_boundary(safe) {
            safe -= 1;
        }
        format!("{}{}", &full[..safe], suffix)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::BuchiPack(a), Value::BuchiPack(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                // Order-independent comparison: each field in a must exist in b with equal value
                a.iter().all(|(name_a, val_a)| {
                    b.iter()
                        .any(|(name_b, val_b)| name_a == name_b && val_a == val_b)
                })
            }
            (Value::Unit, Value::Unit) => true,
            (Value::Gorilla, Value::Gorilla) => true,
            (Value::Async(a), Value::Async(b)) => a.status == b.status && *a.value == *b.value,
            (Value::Json(a), Value::Json(b)) => a == b,
            (Value::Molten, Value::Molten) => true,
            (Value::Stream(a), Value::Stream(b)) => a.status == b.status && a.items == b.items,
            // C18-2: EnumVal equality — same enum + same ordinal. Cross-enum
            // `EnumVal` comparisons return false (the type checker blocks them
            // with [E1605]; this is a defence in depth). `EnumVal` is also
            // equal to `Int(ordinal)` and `Float(ordinal)` with the same
            // numeric value so that existing callers who look at ordinals
            // directly continue to work (e.g. the JSON schema code returns
            // `Value::Int(ordinal)` from `JSON[..., Enum]()`).
            (Value::EnumVal(a_name, a_n), Value::EnumVal(b_name, b_n)) => {
                a_name == b_name && a_n == b_n
            }
            (Value::EnumVal(_, n), Value::Int(m)) | (Value::Int(m), Value::EnumVal(_, n)) => n == m,
            (Value::EnumVal(_, n), Value::Float(f)) | (Value::Float(f), Value::EnumVal(_, n)) => {
                (*n as f64) == *f
            }
            _ => false,
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Str(a), Value::Str(b)) => a.partial_cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
            // C18-4: Same-enum `EnumVal` pairs order by ordinal (declared
            // order is the contract). Cross-enum and Enum↔Int ordering stays
            // `None` so the type checker's [E1605] path still fires when it
            // has to (the checker never reaches here for cross-enum cases,
            // but this is defence in depth for runtime comparisons inside
            // `--no-check` builds).
            (Value::EnumVal(a_name, a_n), Value::EnumVal(b_name, b_n)) if a_name == b_name => {
                a_n.partial_cmp(b_n)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        // PHILOSOPHY.md: all types must have default values
        assert_eq!(Value::default_int(), Value::Int(0));
        assert_eq!(Value::default_float(), Value::Float(0.0));
        assert_eq!(Value::default_str(), Value::Str(String::new()));
        assert_eq!(Value::default_bytes(), Value::Bytes(Vec::new()));
        assert_eq!(Value::default_bool(), Value::Bool(false));
        assert_eq!(Value::default_list(), Value::List(Vec::new()));
    }

    // ── BT-18: Default value guarantee exhaustive tests ──

    #[test]
    fn test_bt18_default_buchi_pack() {
        // BuchiPack default is empty @()
        let d = Value::default_buchi();
        assert_eq!(d, Value::BuchiPack(Vec::new()));
        // Note: empty BuchiPack truthiness depends on implementation
        // (may be truthy since it's a valid struct, just with no fields)
    }

    #[test]
    fn test_bt18_default_json() {
        // JSON default is empty object {}
        let d = Value::default_json();
        match &d {
            Value::Json(v) => assert!(v.is_object(), "JSON default should be object"),
            other => panic!("Expected Json, got: {:?}", other),
        }
    }

    #[test]
    fn test_bt18_default_molten() {
        // Molten default is Molten itself
        assert_eq!(Value::default_molten(), Value::Molten);
    }

    #[test]
    fn test_bt18_default_stream() {
        // Stream default is completed empty stream
        match &Value::default_stream() {
            Value::Stream(sv) => {
                assert!(sv.items.is_empty(), "Default stream should have no items");
                assert_eq!(
                    sv.status,
                    StreamStatus::Completed,
                    "Default stream should be completed"
                );
            }
            other => panic!("Expected Stream, got: {:?}", other),
        }
    }

    #[test]
    fn test_bt18_default_for_list_inference() {
        // default_for_list should infer from first element type
        let int_items = vec![Value::Int(1), Value::Int(2)];
        assert_eq!(Value::default_for_list(&int_items), Value::Int(0));

        let str_items = vec![Value::Str("a".to_string())];
        assert_eq!(
            Value::default_for_list(&str_items),
            Value::Str(String::new())
        );

        let float_items = vec![Value::Float(1.5)];
        assert_eq!(Value::default_for_list(&float_items), Value::Float(0.0));

        // Empty list should return Int(0) as default
        let empty: Vec<Value> = vec![];
        assert_eq!(Value::default_for_list(&empty), Value::Int(0));
    }

    #[test]
    fn test_truthiness() {
        assert!(Value::Bool(true).is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::Str("hello".to_string()).is_truthy());
        assert!(!Value::Str(String::new()).is_truthy());
        assert!(!Value::Unit.is_truthy());
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(42), Value::Int(43));
        // Int-Float cross comparison
        assert_eq!(Value::Int(42), Value::Float(42.0));
        assert_eq!(
            Value::Str("hello".to_string()),
            Value::Str("hello".to_string())
        );
        assert_eq!(Value::Bytes(vec![1, 2]), Value::Bytes(vec![1, 2]));
    }

    #[test]
    fn test_ordering() {
        assert!(Value::Int(1) < Value::Int(2));
        assert!(Value::Float(1.0) < Value::Float(2.0));
        assert!(Value::Int(1) < Value::Float(2.0));
        assert!(Value::Str("a".to_string()) < Value::Str("b".to_string()));
    }

    #[test]
    fn test_display() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Float(314.0 / 100.0).to_string(), "3.14");
        assert_eq!(Value::Str("hello".to_string()).to_string(), "hello");
        assert_eq!(Value::Bytes(vec![1, 2]).to_string(), "Bytes[@[1, 2]]");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Unit.to_string(), "@()");
    }

    #[test]
    fn test_buchi_pack_field_access() {
        let pack = Value::BuchiPack(vec![
            ("name".to_string(), Value::Str("Alice".to_string())),
            ("age".to_string(), Value::Int(30)),
        ]);
        assert_eq!(
            pack.get_field("name"),
            Some(&Value::Str("Alice".to_string()))
        );
        assert_eq!(pack.get_field("age"), Some(&Value::Int(30)));
        assert_eq!(pack.get_field("email"), None);
    }

    #[test]
    fn test_json_default() {
        let json = Value::default_json();
        if let Value::Json(j) = &json {
            assert!(j.is_object());
            assert!(j.as_object().unwrap().is_empty());
        } else {
            panic!("Expected Value::Json");
        }
    }

    #[test]
    fn test_json_truthiness() {
        // null → false
        assert!(!Value::Json(serde_json::Value::Null).is_truthy());
        // empty object → false
        assert!(!Value::Json(serde_json::json!({})).is_truthy());
        // empty array → false
        assert!(!Value::Json(serde_json::json!([])).is_truthy());
        // empty string → false
        assert!(!Value::Json(serde_json::json!("")).is_truthy());
        // 0 → false
        assert!(!Value::Json(serde_json::json!(0)).is_truthy());
        // non-empty object → true
        assert!(Value::Json(serde_json::json!({"a":1})).is_truthy());
        // non-empty array → true
        assert!(Value::Json(serde_json::json!([1])).is_truthy());
        // true → true
        assert!(Value::Json(serde_json::json!(true)).is_truthy());
    }

    #[test]
    fn test_json_equality() {
        let a = Value::Json(serde_json::json!({"x": 1}));
        let b = Value::Json(serde_json::json!({"x": 1}));
        let c = Value::Json(serde_json::json!({"x": 2}));
        assert_eq!(a, b);
        assert_ne!(a, c);
        // JSON != BuchiPack (different types)
        assert_ne!(a, Value::BuchiPack(vec![("x".to_string(), Value::Int(1))]));
    }

    #[test]
    fn test_json_display() {
        let json = Value::Json(serde_json::json!({"a": 1}));
        let s = json.to_display_string();
        assert!(s.contains("\"a\""));
        assert!(s.contains("1"));
    }

    #[test]
    fn test_molten_default() {
        // Molten default is Molten itself (empty molten iron)
        assert_eq!(Value::default_molten(), Value::Molten);
    }

    #[test]
    fn test_molten_truthiness() {
        // Molten (empty molten iron) is falsy
        assert!(!Value::Molten.is_truthy());
    }

    #[test]
    fn test_molten_equality() {
        assert_eq!(Value::Molten, Value::Molten);
        // Molten is not equal to other types
        assert_ne!(Value::Molten, Value::Unit);
        assert_ne!(Value::Molten, Value::Int(0));
    }

    #[test]
    fn test_molten_display() {
        assert_eq!(Value::Molten.to_display_string(), "Molten");
        assert_eq!(Value::Molten.to_string(), "Molten");
    }

    #[test]
    fn test_molten_debug_string() {
        assert_eq!(Value::Molten.to_debug_string(), "Molten");
    }

    #[test]
    fn test_stream_default() {
        let stream = Value::default_stream();
        if let Value::Stream(s) = &stream {
            assert!(s.items.is_empty());
            assert_eq!(s.status, StreamStatus::Completed);
            assert!(s.transforms.is_empty());
        } else {
            panic!("Expected Value::Stream");
        }
    }

    #[test]
    fn test_stream_truthiness() {
        // Empty completed stream → false
        assert!(!Value::default_stream().is_truthy());
        // Active stream → true
        assert!(
            Value::Stream(StreamValue {
                items: Vec::new(),
                transforms: Vec::new(),
                status: StreamStatus::Active,
            })
            .is_truthy()
        );
        // Completed with items → true
        assert!(
            Value::Stream(StreamValue {
                items: vec![Value::Int(1)],
                transforms: Vec::new(),
                status: StreamStatus::Completed,
            })
            .is_truthy()
        );
    }

    #[test]
    fn test_stream_display() {
        let active = Value::Stream(StreamValue {
            items: Vec::new(),
            transforms: Vec::new(),
            status: StreamStatus::Active,
        });
        assert_eq!(active.to_display_string(), "Stream[active]");

        let completed = Value::Stream(StreamValue {
            items: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        assert_eq!(completed.to_display_string(), "Stream[completed: 3 items]");
    }

    #[test]
    fn test_stream_equality() {
        let a = Value::Stream(StreamValue {
            items: vec![Value::Int(1)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        let b = Value::Stream(StreamValue {
            items: vec![Value::Int(1)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        let c = Value::Stream(StreamValue {
            items: vec![Value::Int(2)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
