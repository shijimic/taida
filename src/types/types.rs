/// Taida Lang type system.
///
/// All types have default values — null/undefined does not exist.
/// Structural subtyping: a value with extra fields is compatible
/// with a type that expects fewer fields (width subtyping).
use std::collections::{HashMap, HashSet};
use std::fmt;

type MoldDefFields = (Vec<String>, Vec<(String, Type)>);

/// A Taida type.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Integer type
    Int,
    /// Floating-point type
    Float,
    /// General number type (Int or Float)
    Num,
    /// String type
    Str,
    /// Bytes type (immutable byte sequence)
    Bytes,
    /// Boolean type
    Bool,
    /// Buchi pack type (named fields)
    BuchiPack(Vec<(String, Type)>),
    /// List type
    List(Box<Type>),
    /// Function type (params -> return)
    Function(Vec<Type>, Box<Type>),
    /// Named user-defined type
    Named(String),
    /// Generic / Mold type instantiation: e.g., Lax[Int]
    Generic(String, Vec<Type>),
    /// Error type (inherits from base Error)
    Error(String),
    /// Unit type (empty buchi pack)
    Unit,
    /// Unknown / not yet inferred
    Unknown,
    /// Any type — used internally for type inference, never user-visible
    Any,
    /// JSON type — opaque external data primitive
    Json,
    /// Molten type — opaque primitive for external (JS) interop data
    Molten,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Float => write!(f, "Float"),
            Type::Num => write!(f, "Num"),
            Type::Str => write!(f, "Str"),
            Type::Bytes => write!(f, "Bytes"),
            Type::Bool => write!(f, "Bool"),
            Type::BuchiPack(fields) => {
                write!(f, "@(")?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, ty)?;
                }
                write!(f, ")")
            }
            Type::List(inner) => write!(f, "@[{}]", inner),
            Type::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") => {}", ret)
            }
            Type::Named(name) => write!(f, "{}", name),
            Type::Generic(name, args) => {
                write!(f, "{}[", name)?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", a)?;
                }
                write!(f, "]")
            }
            Type::Error(name) => write!(f, "{}", name),
            Type::Unit => write!(f, "@()"),
            Type::Unknown => write!(f, "?"),
            Type::Any => write!(f, "?"),
            Type::Json => write!(f, "JSON"),
            Type::Molten => write!(f, "Molten"),
        }
    }
}

impl Type {
    /// Returns the default value for a type as a runtime Value.
    /// Every Taida type must have a default value (no null/undefined).
    pub fn default_value_description(&self) -> &'static str {
        match self {
            Type::Int => "0",
            Type::Float => "0.0",
            Type::Num => "0",
            Type::Str => "\"\"",
            Type::Bytes => "Bytes[]",
            Type::Bool => "false",
            Type::BuchiPack(_) => "@(...defaults)",
            Type::List(_) => "@[]",
            Type::Function(_, _) => "<noop>",
            Type::Named(_) => "<default>",
            Type::Generic(_, _) => "<default>",
            Type::Error(_) => "Error(type: \"\", message: \"\")",
            Type::Unit => "@()",
            Type::Unknown => "<unknown>",
            Type::Any => "<any>",
            Type::Json => "{}",
            Type::Molten => "Molten",
        }
    }

    /// Check if this type is a numeric type.
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Int | Type::Float | Type::Num)
    }

    /// Check structural subtype compatibility: `self` is a subtype of `expected`.
    /// Width subtyping: a buchi pack with extra fields is a subtype of one with fewer.
    ///
    /// `Type::Function` follows the standard rule — parameters are
    /// contravariant, the return type is covariant. `Type::Unknown` is not
    /// a compatibility wildcard; unresolved types must be diagnosed before
    /// subtype checks decide user-visible compatibility.
    pub fn is_subtype_of(&self, expected: &Type) -> bool {
        self.is_subtype_of_inner(expected, false)
    }

    /// Like `is_subtype_of`, but the `Int → Float` widening rule is
    /// disabled across the entire type tree. Used by the function-param
    /// contravariance check so that an `Int`-accepting function value
    /// cannot be passed where a `Float`-accepting parameter is expected.
    /// PHILOSOPHY I forbids implicit type conversion at function
    /// boundaries; the residual widening rule is kept only for direct
    /// numeric assignment / arithmetic contexts.
    pub fn is_function_arg_subtype_of(&self, expected: &Type) -> bool {
        self.is_subtype_of_inner(expected, true)
    }

    pub(crate) fn is_subtype_of_inner(&self, expected: &Type, strict_int_float: bool) -> bool {
        if self == expected {
            return true;
        }

        match (self, expected) {
            // Any accepts everything. Unknown does not: unresolved types
            // must be finalized or rejected before compatibility checks.
            (_, Type::Any) => true,
            (Type::Any, _) => true,

            // Int is a subtype of Num, Float is a subtype of Num
            (Type::Int, Type::Num) | (Type::Float, Type::Num) => true,
            (Type::Int, Type::Float) if !strict_int_float => true, // Int widened to Float
            (Type::Int, Type::Float) => false,

            // Structural subtyping for buchi packs
            (Type::BuchiPack(self_fields), Type::BuchiPack(expected_fields)) => {
                expected_fields.iter().all(|(exp_name, exp_type)| {
                    self_fields.iter().any(|(self_name, self_type)| {
                        self_name == exp_name
                            && self_type.is_subtype_of_inner(exp_type, strict_int_float)
                    })
                })
            }

            // List covariance
            (Type::List(a), Type::List(b)) => a.is_subtype_of_inner(b, strict_int_float),

            // Function subtype: param contravariant, return covariant.
            // (P1, ..., Pn) -> R   <:   (Q1, ..., Qn) -> S
            //   iff   Qi <: Pi  (contravariant) for all i
            //   and   R <: S   (covariant)
            //   and   arity matches
            (Type::Function(self_params, self_ret), Type::Function(exp_params, exp_ret)) => {
                self_params.len() == exp_params.len()
                    && self_params
                        .iter()
                        .zip(exp_params.iter())
                        .all(|(self_p, exp_p)| exp_p.is_subtype_of_inner(self_p, strict_int_float))
                    && self_ret.is_subtype_of_inner(exp_ret, strict_int_float)
            }

            // Error subtyping: specific error is subtype of Error
            (Type::Error(_), Type::Error(name)) if name == "Error" => true,
            (Type::Error(_), Type::Named(name)) if name == "Error" => true,
            (Type::Error(a), Type::Named(b)) | (Type::Named(a), Type::Error(b)) if a == b => true,

            // Named type could match by name
            (Type::Named(a), Type::Named(b)) => a == b,
            (Type::Named(a), Type::Generic(b, _)) => {
                a == b && matches!(a.as_str(), "HashMap" | "Set")
            }
            (Type::Generic(a, _), Type::Named(b)) => {
                a == b && matches!(a.as_str(), "HashMap" | "Set")
            }

            // Generic types
            (Type::Generic(a_name, a_args), Type::Generic(b_name, b_args)) => {
                a_name == b_name
                    && a_args.len() == b_args.len()
                    && a_args
                        .iter()
                        .zip(b_args.iter())
                        .all(|(a, b)| a.is_subtype_of_inner(b, strict_int_float))
            }

            _ => false,
        }
    }

    /// Returns true if any nested `Type::Unknown` is present. Used by
    /// `tests/typed_hir_smoke.rs` to verify that `TypedExprTable`
    /// contains no residual `Type::Unknown` after `check_program`
    /// completes for a fully-typed fixture.
    pub fn contains_concrete_unknown(&self) -> bool {
        match self {
            Type::Unknown => true,
            Type::List(inner) => inner.contains_concrete_unknown(),
            Type::Function(params, ret) => {
                params.iter().any(|p| p.contains_concrete_unknown())
                    || ret.contains_concrete_unknown()
            }
            Type::Generic(_, args) => args.iter().any(|a| a.contains_concrete_unknown()),
            Type::BuchiPack(fields) => fields.iter().any(|(_, t)| t.contains_concrete_unknown()),
            _ => false,
        }
    }
}

/// Type definition registry — stores all user-defined types.
#[derive(Debug, Clone, Default)]
pub struct TypeRegistry {
    /// Named type definitions: name -> fields
    pub type_defs: HashMap<String, Vec<(String, Type)>>,
    /// Enum definitions: name -> variants in ordinal order
    pub enum_defs: HashMap<String, Vec<String>>,
    /// Mold type definitions: name -> (type_params, fields)
    pub mold_defs: HashMap<String, MoldDefFields>,
    /// Inheritance relationships: child -> parent
    pub inheritance: HashMap<String, String>,
    /// Error type definitions (inherit from Error)
    pub error_types: HashMap<String, Vec<(String, Type)>>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        // Register built-in Error base type
        registry.type_defs.insert(
            "Error".to_string(),
            vec![
                ("type".to_string(), Type::Str),
                ("message".to_string(), Type::Str),
            ],
        );
        registry.type_defs.insert(
            "ErrorInfo".to_string(),
            vec![
                ("type".to_string(), Type::Str),
                ("message".to_string(), Type::Str),
                ("kind".to_string(), Type::Str),
                ("code".to_string(), Type::Int),
            ],
        );
        registry.type_defs.insert(
            "RelaxedGorillaEscaped".to_string(),
            vec![
                ("type".to_string(), Type::Str),
                ("message".to_string(), Type::Str),
            ],
        );
        registry
            .inheritance
            .insert("RelaxedGorillaEscaped".to_string(), "Error".to_string());
        registry
            .error_types
            .insert("RelaxedGorillaEscaped".to_string(), Vec::new());
        registry
    }

    /// Register a type definition.
    pub fn register_type(&mut self, name: &str, fields: Vec<(String, Type)>) {
        self.type_defs.insert(name.to_string(), fields);
    }

    /// Register an enum definition.
    pub fn register_enum(&mut self, name: &str, variants: Vec<String>) {
        self.enum_defs.insert(name.to_string(), variants);
    }

    /// Register a mold type definition.
    pub fn register_mold(
        &mut self,
        name: &str,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
    ) {
        self.mold_defs
            .insert(name.to_string(), (type_params, fields));
    }

    /// Register an inheritance relationship.
    ///
    /// Returns `false` if registering would create a cycle in the
    /// inheritance chain (e.g. `A => B`, then `B => A`). In that case
    /// the relationship is **not** stored.
    pub fn register_inheritance(
        &mut self,
        parent: &str,
        child: &str,
        extra_fields: Vec<(String, Type)>,
    ) -> bool {
        // RCB-51: Detect cycles before inserting.
        // Self-cycle: child == parent is always a cycle.
        if child == parent {
            return false;
        }
        // Walk from `parent` up to the root; if we encounter `child`
        // anywhere in the chain the new edge would close a cycle.
        let mut cursor = parent.to_string();
        let mut visited = HashSet::new();
        visited.insert(child.to_string());
        while let Some(ancestor) = self.inheritance.get(&cursor) {
            if ancestor == child {
                return false; // Would form a cycle: child -> ... -> parent -> child
            }
            if !visited.insert(ancestor.clone()) {
                // Already seen -- the chain is itself broken or cyclic.
                return false;
            }
            cursor = ancestor.clone();
        }

        self.inheritance
            .insert(child.to_string(), parent.to_string());

        // Child type gets parent fields + its own fields.
        // RCB-215: Remove parent fields that are overridden by child fields
        // to prevent duplicate field entries.
        let mut fields = self.get_type_fields(parent).unwrap_or_default();
        fields.retain(|(name, _)| !extra_fields.iter().any(|(n, _)| n == name));
        fields.extend(extra_fields);
        self.type_defs.insert(child.to_string(), fields);
        true
    }

    /// Register an error type (inherits from an error parent).
    ///
    /// `parent` is the direct parent type name (e.g. "Error" or "AppError").
    /// Delegates field composition and inheritance registration to `register_inheritance`,
    /// then additionally records the type in `error_types` (extra_fields only, not full set).
    ///
    /// Returns `false` if the inheritance would create a cycle.
    pub fn register_error_type(
        &mut self,
        parent: &str,
        name: &str,
        extra_fields: Vec<(String, Type)>,
    ) -> bool {
        debug_assert!(
            self.get_type_fields(parent).is_some(),
            "register_error_type called with unregistered parent: {}",
            parent
        );
        let registered = self.register_inheritance(parent, name, extra_fields.clone());
        if registered {
            self.error_types.insert(name.to_string(), extra_fields);
        }
        registered
    }

    /// Get the fields of a type definition.
    pub fn get_type_fields(&self, name: &str) -> Option<Vec<(String, Type)>> {
        self.type_defs.get(name).cloned()
    }

    pub fn is_enum_type(&self, name: &str) -> bool {
        self.enum_defs.contains_key(name)
    }

    pub fn get_enum_variants(&self, name: &str) -> Option<Vec<String>> {
        self.enum_defs.get(name).cloned()
    }

    pub fn get_enum_variant_ordinal(&self, enum_name: &str, variant_name: &str) -> Option<usize> {
        self.enum_defs
            .get(enum_name)
            .and_then(|variants| variants.iter().position(|variant| variant == variant_name))
    }

    /// Check if a type is an error type (inherits from Error).
    pub fn is_error_type(&self, name: &str) -> bool {
        name == "Error" || self.error_types.contains_key(name)
    }

    /// Check structural subtype compatibility with registry context.
    /// Resolves Named types to their fields for structural comparison.
    pub fn is_subtype_of(&self, actual: &Type, expected: &Type) -> bool {
        self.is_subtype_of_inner(actual, expected, false)
    }

    /// Like `is_subtype_of`, but disallows the `Int → Float` widening
    /// rule across the whole type tree, including registry-resolved
    /// Named / BuchiPack structural paths. Used at every function /
    /// method boundary so that PHILOSOPHY I "no implicit type
    /// conversion" is honoured uniformly. Pre-existing widening on
    /// non-boundary subtype contexts (numeric arithmetic, direct
    /// assignment) is unaffected.
    pub fn is_function_arg_subtype_of(&self, actual: &Type, expected: &Type) -> bool {
        self.is_subtype_of_inner(actual, expected, true)
    }

    fn is_subtype_of_inner(&self, actual: &Type, expected: &Type, strict_int_float: bool) -> bool {
        if actual == expected {
            return true;
        }
        match (actual, expected) {
            // Compound types must recurse through the registry-aware path so
            // inherited/named relationships are preserved inside containers.
            (Type::BuchiPack(actual_fields), Type::BuchiPack(expected_fields)) => {
                expected_fields.iter().all(|(exp_name, exp_type)| {
                    actual_fields.iter().any(|(actual_name, actual_type)| {
                        actual_name == exp_name
                            && self.is_subtype_of_inner(actual_type, exp_type, strict_int_float)
                    })
                })
            }
            (Type::List(actual_inner), Type::List(expected_inner)) => {
                self.is_subtype_of_inner(actual_inner, expected_inner, strict_int_float)
            }
            (
                Type::Function(actual_params, actual_ret),
                Type::Function(expected_params, expected_ret),
            ) => {
                actual_params.len() == expected_params.len()
                    && actual_params.iter().zip(expected_params.iter()).all(
                        |(actual_param, expected_param)| {
                            self.is_subtype_of_inner(expected_param, actual_param, strict_int_float)
                        },
                    )
                    && self.is_subtype_of_inner(actual_ret, expected_ret, strict_int_float)
            }
            (
                Type::Generic(actual_name, actual_args),
                Type::Generic(expected_name, expected_args),
            ) => {
                actual_name == expected_name
                    && actual_args.len() == expected_args.len()
                    && actual_args.iter().zip(expected_args.iter()).all(
                        |(actual_arg, expected_arg)| {
                            self.is_subtype_of_inner(actual_arg, expected_arg, strict_int_float)
                        },
                    )
            }
            (Type::Error(a), Type::Error(b)) => {
                self.named_inherits_from(a, b) || self.named_fields_subtype(a, b, strict_int_float)
            }
            (Type::Error(a), Type::Named(b)) | (Type::Named(a), Type::Error(b)) => {
                self.named_inherits_from(a, b)
            }
            // Named vs Named: check inheritance chain, then structural fields
            (Type::Named(a), Type::Named(b)) => {
                self.named_inherits_from(a, b) || self.named_fields_subtype(a, b, strict_int_float)
            }
            (Type::Named(name), Type::BuchiPack(expected_fields)) => {
                if let Some(actual_fields) = self.get_type_fields(name) {
                    expected_fields.iter().all(|(exp_name, exp_type)| {
                        actual_fields.iter().any(|(self_name, self_type)| {
                            self_name == exp_name
                                && self.is_subtype_of_inner(self_type, exp_type, strict_int_float)
                        })
                    })
                } else {
                    false
                }
            }
            (Type::BuchiPack(actual_fields), Type::Named(name)) => {
                if let Some(expected_fields) = self.get_type_fields(name) {
                    expected_fields.iter().all(|(exp_name, exp_type)| {
                        actual_fields.iter().any(|(self_name, self_type)| {
                            self_name == exp_name
                                && self.is_subtype_of_inner(self_type, exp_type, strict_int_float)
                        })
                    })
                } else {
                    false
                }
            }
            _ => actual.is_subtype_of_inner(expected, strict_int_float),
        }
    }

    fn named_inherits_from(&self, actual: &str, expected: &str) -> bool {
        if actual == expected {
            return true;
        }
        let mut current = actual;
        let mut visited = HashSet::new();
        visited.insert(current.to_string());
        while let Some(parent) = self.inheritance.get(current) {
            if parent == expected {
                return true;
            }
            if !visited.insert(parent.clone()) {
                break;
            }
            current = parent;
        }
        false
    }

    fn named_fields_subtype(&self, actual: &str, expected: &str, strict_int_float: bool) -> bool {
        if let (Some(actual_fields), Some(expected_fields)) =
            (self.get_type_fields(actual), self.get_type_fields(expected))
        {
            expected_fields.iter().all(|(exp_name, exp_type)| {
                actual_fields.iter().any(|(self_name, self_type)| {
                    self_name == exp_name
                        && self.is_subtype_of_inner(self_type, exp_type, strict_int_float)
                })
            })
        } else {
            false
        }
    }

    /// Resolve a type expression to a concrete Type.
    ///
    /// N-74: This method does not cache results. Each call re-traverses the
    /// TypeExpr tree. This is acceptable at current codebase scale because:
    /// 1. Type expressions are typically shallow (1-3 levels deep).
    /// 2. The checker calls resolve_type() O(n) times per program where n is
    /// the number of type annotations -- not per-expression.
    /// 3. Adding a cache would require either interior mutability (&self -> &mut self
    /// propagation) or a RefCell, adding complexity for negligible benefit.
    ///
    /// If profiling reveals this as a bottleneck, consider a HashMap<TypeExpr, Type>
    /// cache with the TypeExpr implementing Hash + Eq.
    pub fn resolve_type(&self, ty: &crate::parser::TypeExpr) -> Type {
        use crate::parser::TypeExpr;
        match ty {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" | "Integer" => Type::Int,
                "Float" => Type::Float,
                "Num" => Type::Num,
                "Str" | "String" => Type::Str,
                "Bytes" => Type::Bytes,
                "Bool" | "Boolean" => Type::Bool,
                "Unit" => Type::Unit,
                "JSON" => Type::Json,
                "Molten" => Type::Molten,
                other => {
                    if self.is_error_type(other) {
                        Type::Error(other.to_string())
                    } else {
                        Type::Named(other.to_string())
                    }
                }
            },
            TypeExpr::BuchiPack(fields) => {
                let resolved: Vec<(String, Type)> = fields
                    .iter()
                    .map(|f| {
                        let field_type = f
                            .type_annotation
                            .as_ref()
                            .map(|t| self.resolve_type(t))
                            .unwrap_or(Type::Unknown);
                        (f.name.clone(), field_type)
                    })
                    .collect();
                Type::BuchiPack(resolved)
            }
            TypeExpr::List(inner) => Type::List(Box::new(self.resolve_type(inner))),
            TypeExpr::Generic(name, args) => {
                let resolved_args: Vec<Type> = args.iter().map(|a| self.resolve_type(a)).collect();
                Type::Generic(name.clone(), resolved_args)
            }
            TypeExpr::Function(params, ret) => {
                let resolved_params: Vec<Type> =
                    params.iter().map(|p| self.resolve_type(p)).collect();
                Type::Function(resolved_params, Box::new(self.resolve_type(ret)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values_exist() {
        // All types must have default values -- PHILOSOPHY.md constraint
        let types = vec![
            Type::Int,
            Type::Float,
            Type::Num,
            Type::Str,
            Type::Bytes,
            Type::Bool,
            Type::BuchiPack(vec![]),
            Type::List(Box::new(Type::Int)),
            Type::Unit,
            Type::Json,
            Type::Molten,
        ];
        for ty in types {
            assert!(
                !ty.default_value_description().is_empty(),
                "Type {} must have a default value",
                ty
            );
        }
    }

    #[test]
    fn test_resolve_unit_type_expr() {
        let registry = TypeRegistry::new();
        assert_eq!(
            registry.resolve_type(&crate::parser::TypeExpr::Named("Unit".to_string())),
            Type::Unit
        );
    }

    #[test]
    fn test_numeric_subtyping() {
        assert!(Type::Int.is_subtype_of(&Type::Num));
        assert!(Type::Float.is_subtype_of(&Type::Num));
        assert!(Type::Int.is_subtype_of(&Type::Float));
        assert!(!Type::Str.is_subtype_of(&Type::Num));
    }

    // E34 Phase 1.1 (Lock-C foundation): Type::Function subtype tests.

    #[test]
    fn test_function_subtype_basic() {
        let f1 = Type::Function(vec![Type::Int], Box::new(Type::Int));
        let f2 = Type::Function(vec![Type::Int], Box::new(Type::Int));
        assert!(f1.is_subtype_of(&f2));

        let f3 = Type::Function(vec![Type::Int], Box::new(Type::Str));
        assert!(!f3.is_subtype_of(&f1));
    }

    #[test]
    fn test_function_subtype_covariant_return() {
        // (Int) -> Int is a subtype of (Int) -> Num (Int <: Num covariantly)
        let f_int = Type::Function(vec![Type::Int], Box::new(Type::Int));
        let f_num = Type::Function(vec![Type::Int], Box::new(Type::Num));
        assert!(f_int.is_subtype_of(&f_num));
        // (Int) -> Num is NOT a subtype of (Int) -> Int (Num is wider than Int)
        assert!(!f_num.is_subtype_of(&f_int));
    }

    #[test]
    fn test_function_subtype_contravariant_param() {
        // (Num) -> Int is a subtype of (Int) -> Int
        // because Num is wider than Int (any Int can be passed to a Num-accepting fn)
        let f_num = Type::Function(vec![Type::Num], Box::new(Type::Int));
        let f_int = Type::Function(vec![Type::Int], Box::new(Type::Int));
        assert!(f_num.is_subtype_of(&f_int));
        // The reverse is NOT true: (Int) -> Int cannot accept a Num
        assert!(!f_int.is_subtype_of(&f_num));
    }

    #[test]
    fn test_function_subtype_arity_mismatch() {
        let f1 = Type::Function(vec![Type::Int, Type::Int], Box::new(Type::Int));
        let f2 = Type::Function(vec![Type::Int], Box::new(Type::Int));
        assert!(!f1.is_subtype_of(&f2));
        assert!(!f2.is_subtype_of(&f1));
    }

    #[test]
    fn test_function_subtype_unknown_is_not_wildcard() {
        let f_unknown_param = Type::Function(vec![Type::Unknown], Box::new(Type::Int));
        let f_int_param = Type::Function(vec![Type::Int], Box::new(Type::Int));
        assert!(!f_unknown_param.is_subtype_of(&f_int_param));
        assert!(!f_int_param.is_subtype_of(&f_unknown_param));

        let f_unknown_ret = Type::Function(vec![Type::Int], Box::new(Type::Unknown));
        let f_int_ret = Type::Function(vec![Type::Int], Box::new(Type::Int));
        assert!(!f_unknown_ret.is_subtype_of(&f_int_ret));
        assert!(!f_int_ret.is_subtype_of(&f_unknown_ret));
    }

    #[test]
    fn test_contains_concrete_unknown() {
        assert!(!Type::Int.contains_concrete_unknown());
        assert!(!Type::Bool.contains_concrete_unknown());
        assert!(Type::Unknown.contains_concrete_unknown());
        assert!(Type::List(Box::new(Type::Unknown)).contains_concrete_unknown());
        assert!(
            Type::Function(vec![Type::Unknown], Box::new(Type::Int)).contains_concrete_unknown()
        );
        assert!(
            Type::Function(vec![Type::Int], Box::new(Type::Unknown)).contains_concrete_unknown()
        );
        assert!(!Type::Function(vec![Type::Int], Box::new(Type::Int)).contains_concrete_unknown());
        assert!(Type::Generic("Lax".to_string(), vec![Type::Unknown]).contains_concrete_unknown());
        assert!(!Type::Generic("Lax".to_string(), vec![Type::Int]).contains_concrete_unknown());
    }

    #[test]
    fn test_structural_subtyping() {
        // Employee (name, age, dept) is subtype of Person (name, age)
        let person = Type::BuchiPack(vec![
            ("name".to_string(), Type::Str),
            ("age".to_string(), Type::Int),
        ]);
        let employee = Type::BuchiPack(vec![
            ("name".to_string(), Type::Str),
            ("age".to_string(), Type::Int),
            ("department".to_string(), Type::Str),
        ]);
        assert!(employee.is_subtype_of(&person));
        assert!(!person.is_subtype_of(&employee));
    }

    #[test]
    fn test_error_subtyping() {
        let base_error = Type::Error("Error".to_string());
        let custom_error = Type::Error("ValidationError".to_string());
        assert!(custom_error.is_subtype_of(&base_error));
    }

    #[test]
    fn test_list_covariance() {
        let int_list = Type::List(Box::new(Type::Int));
        let num_list = Type::List(Box::new(Type::Num));
        assert!(int_list.is_subtype_of(&num_list));
        assert!(!num_list.is_subtype_of(&int_list));
    }

    #[test]
    fn test_type_registry() {
        let mut reg = TypeRegistry::new();

        // Register Person type
        reg.register_type(
            "Person",
            vec![
                ("name".to_string(), Type::Str),
                ("age".to_string(), Type::Int),
            ],
        );

        // Register Employee inheriting from Person
        reg.register_inheritance(
            "Person",
            "Employee",
            vec![("department".to_string(), Type::Str)],
        );

        let emp_fields = reg
            .get_type_fields("Employee")
            .expect("Employee type should be registered after inheritance");
        assert_eq!(emp_fields.len(), 3);

        // Register error type
        reg.register_error_type(
            "Error",
            "ValidationError",
            vec![("field".to_string(), Type::Str)],
        );
        assert!(reg.is_error_type("ValidationError"));
        assert!(reg.is_error_type("Error"));
        assert!(!reg.is_error_type("Person"));
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Type::Int), "Int");
        assert_eq!(format!("{}", Type::List(Box::new(Type::Str))), "@[Str]");
        assert_eq!(format!("{}", Type::Bytes), "Bytes");
        assert_eq!(
            format!(
                "{}",
                Type::BuchiPack(vec![
                    ("name".to_string(), Type::Str),
                    ("age".to_string(), Type::Int),
                ])
            ),
            "@(name: Str, age: Int)"
        );
        assert_eq!(
            format!("{}", Type::Generic("Optional".to_string(), vec![Type::Int])),
            "Optional[Int]"
        );
        assert_eq!(format!("{}", Type::Molten), "Molten");
    }

    #[test]
    fn test_named_structural_subtyping_via_registry() {
        let mut reg = TypeRegistry::new();
        reg.register_type(
            "Person",
            vec![
                ("name".to_string(), Type::Str),
                ("age".to_string(), Type::Int),
            ],
        );
        reg.register_inheritance(
            "Person",
            "Employee",
            vec![("department".to_string(), Type::Str)],
        );

        let person = Type::Named("Person".to_string());
        let employee = Type::Named("Employee".to_string());

        // Employee is subtype of Person (inheritance)
        assert!(reg.is_subtype_of(&employee, &person));
        // Person is NOT subtype of Employee (missing department field)
        assert!(!reg.is_subtype_of(&person, &employee));
    }

    #[test]
    fn test_registry_aware_compound_subtyping_recurses() {
        let mut reg = TypeRegistry::new();
        reg.register_type("Base", vec![("x".to_string(), Type::Int)]);
        reg.register_inheritance("Base", "Child", vec![("y".to_string(), Type::Int)]);

        let base = Type::Named("Base".to_string());
        let child = Type::Named("Child".to_string());

        assert!(reg.is_subtype_of(
            &Type::BuchiPack(vec![("item".to_string(), child.clone())]),
            &Type::BuchiPack(vec![("item".to_string(), base.clone())]),
        ));
        assert!(reg.is_subtype_of(
            &Type::List(Box::new(child.clone())),
            &Type::List(Box::new(base.clone())),
        ));
        assert!(reg.is_subtype_of(
            &Type::Function(vec![], Box::new(child.clone())),
            &Type::Function(vec![], Box::new(base.clone())),
        ));
        assert!(reg.is_subtype_of(
            &Type::Generic("Box".to_string(), vec![child]),
            &Type::Generic("Box".to_string(), vec![base]),
        ));
    }

    #[test]
    fn test_named_structural_subtyping_no_inheritance() {
        let mut reg = TypeRegistry::new();
        reg.register_type(
            "Point2D",
            vec![("x".to_string(), Type::Int), ("y".to_string(), Type::Int)],
        );
        reg.register_type(
            "Point3D",
            vec![
                ("x".to_string(), Type::Int),
                ("y".to_string(), Type::Int),
                ("z".to_string(), Type::Int),
            ],
        );

        let p2d = Type::Named("Point2D".to_string());
        let p3d = Type::Named("Point3D".to_string());

        // Point3D is structurally a subtype of Point2D (has x, y, plus z)
        assert!(reg.is_subtype_of(&p3d, &p2d));
        // Point2D is NOT a subtype of Point3D (missing z)
        assert!(!reg.is_subtype_of(&p2d, &p3d));
    }

    #[test]
    fn test_named_vs_buchipack_subtyping() {
        let mut reg = TypeRegistry::new();
        reg.register_type(
            "Person",
            vec![
                ("name".to_string(), Type::Str),
                ("age".to_string(), Type::Int),
            ],
        );

        let person = Type::Named("Person".to_string());
        let bp = Type::BuchiPack(vec![("name".to_string(), Type::Str)]);

        // Person is subtype of @(name: Str) (has name + age)
        assert!(reg.is_subtype_of(&person, &bp));
        // @(name: Str) is NOT subtype of Person (missing age)
        assert!(!reg.is_subtype_of(&bp, &person));
    }

    #[test]
    fn test_multilevel_error_inheritance() {
        let mut reg = TypeRegistry::new();

        // Error => AppError = @(app_code: Int)
        reg.register_error_type(
            "Error",
            "AppError",
            vec![("app_code".to_string(), Type::Int)],
        );

        // AppError => ValidationError = @(field: Str)
        reg.register_error_type(
            "AppError",
            "ValidationError",
            vec![("field".to_string(), Type::Str)],
        );

        // Check error type recognition
        assert!(reg.is_error_type("Error"));
        assert!(reg.is_error_type("AppError"));
        assert!(reg.is_error_type("ValidationError"));

        // Check field composition: ValidationError should have type, message, app_code, field
        let ve_fields = reg
            .get_type_fields("ValidationError")
            .expect("ValidationError should be registered");
        assert_eq!(
            ve_fields.len(),
            4,
            "ValidationError should have 4 fields: type, message, app_code, field"
        );
        assert!(ve_fields.iter().any(|(n, _)| n == "type"));
        assert!(ve_fields.iter().any(|(n, _)| n == "message"));
        assert!(ve_fields.iter().any(|(n, _)| n == "app_code"));
        assert!(ve_fields.iter().any(|(n, _)| n == "field"));

        // Check inheritance chain: ValidationError -> AppError -> Error
        let error_ty = Type::Named("Error".to_string());
        let app_error_ty = Type::Named("AppError".to_string());
        let val_error_ty = Type::Named("ValidationError".to_string());

        // ValidationError IS-A AppError
        assert!(
            reg.is_subtype_of(&val_error_ty, &app_error_ty),
            "ValidationError should be a subtype of AppError"
        );
        // ValidationError IS-A Error
        assert!(
            reg.is_subtype_of(&val_error_ty, &error_ty),
            "ValidationError should be a subtype of Error"
        );
        // AppError IS-A Error
        assert!(
            reg.is_subtype_of(&app_error_ty, &error_ty),
            "AppError should be a subtype of Error"
        );
        // Error is NOT AppError
        assert!(!reg.is_subtype_of(&error_ty, &app_error_ty));
    }

    #[test]
    fn test_error_named_subtyping_does_not_use_structural_fields() {
        let mut reg = TypeRegistry::new();
        reg.register_type(
            "Pilot",
            vec![
                ("type".to_string(), Type::Str),
                ("message".to_string(), Type::Str),
            ],
        );
        assert!(reg.register_error_type("Error", "MyErr", vec![("code".to_string(), Type::Int)]));

        let my_err = Type::Error("MyErr".to_string());
        let pilot = Type::Named("Pilot".to_string());
        assert!(
            !reg.is_subtype_of(&my_err, &pilot),
            "Error-derived names must not structurally satisfy unrelated named types"
        );
    }

    #[test]
    fn test_multilevel_custom_inheritance() {
        let mut reg = TypeRegistry::new();

        // Vehicle = @(name: Str, speed: Int)
        reg.register_type(
            "Vehicle",
            vec![
                ("name".to_string(), Type::Str),
                ("speed".to_string(), Type::Int),
            ],
        );

        // Vehicle => Car = @(doors: Int)
        reg.register_inheritance("Vehicle", "Car", vec![("doors".to_string(), Type::Int)]);

        // Car => SportsCar = @(turbo: Bool)
        reg.register_inheritance("Car", "SportsCar", vec![("turbo".to_string(), Type::Bool)]);

        // Check field composition: SportsCar should have name, speed, doors, turbo
        let sc_fields = reg
            .get_type_fields("SportsCar")
            .expect("SportsCar should be registered");
        assert_eq!(sc_fields.len(), 4, "SportsCar should have 4 fields");
        assert!(sc_fields.iter().any(|(n, _)| n == "name"));
        assert!(sc_fields.iter().any(|(n, _)| n == "speed"));
        assert!(sc_fields.iter().any(|(n, _)| n == "doors"));
        assert!(sc_fields.iter().any(|(n, _)| n == "turbo"));

        // Check inheritance chain
        let vehicle_ty = Type::Named("Vehicle".to_string());
        let car_ty = Type::Named("Car".to_string());
        let sports_car_ty = Type::Named("SportsCar".to_string());

        // SportsCar IS-A Car
        assert!(reg.is_subtype_of(&sports_car_ty, &car_ty));
        // SportsCar IS-A Vehicle
        assert!(reg.is_subtype_of(&sports_car_ty, &vehicle_ty));
        // Car IS-A Vehicle
        assert!(reg.is_subtype_of(&car_ty, &vehicle_ty));
        // Vehicle is NOT Car
        assert!(!reg.is_subtype_of(&vehicle_ty, &car_ty));
    }

    // -- RCB-51: Cyclic inheritance detection --

    #[test]
    fn test_rcb51_direct_cycle_rejected() {
        // A => B, then B => A should be rejected
        let mut reg = TypeRegistry::new();
        reg.register_type("A", vec![("a".to_string(), Type::Int)]);
        assert!(reg.register_inheritance("A", "B", vec![("b".to_string(), Type::Int)]));
        assert!(
            !reg.register_inheritance("B", "A", vec![("c".to_string(), Type::Int)]),
            "B => A should be rejected (would create A -> B -> A cycle)"
        );
    }

    #[test]
    fn test_rcb51_indirect_cycle_rejected() {
        // A => B => C, then C => A should be rejected
        let mut reg = TypeRegistry::new();
        reg.register_type("A", vec![("a".to_string(), Type::Int)]);
        assert!(reg.register_inheritance("A", "B", vec![("b".to_string(), Type::Int)]));
        assert!(reg.register_inheritance("B", "C", vec![("c".to_string(), Type::Int)]));
        assert!(
            !reg.register_inheritance("C", "A", vec![("d".to_string(), Type::Int)]),
            "C => A should be rejected (would create A -> B -> C -> A cycle)"
        );
    }

    #[test]
    fn test_rcb51_self_cycle_rejected() {
        // A => A should be rejected
        let mut reg = TypeRegistry::new();
        reg.register_type("A", vec![("a".to_string(), Type::Int)]);
        assert!(
            !reg.register_inheritance("A", "A", vec![("b".to_string(), Type::Int)]),
            "A => A should be rejected (self-cycle)"
        );
    }

    #[test]
    fn test_rcb51_is_subtype_of_no_hang_on_cycle() {
        // Even if a cycle somehow exists in the inheritance map,
        // is_subtype_of must terminate (visited-set guard).
        let mut reg = TypeRegistry::new();
        reg.register_type("X", vec![("x".to_string(), Type::Int)]);
        reg.register_type("Y", vec![("y".to_string(), Type::Int)]);
        // Force a cycle by manually inserting (bypass register_inheritance)
        reg.inheritance.insert("X".to_string(), "Y".to_string());
        reg.inheritance.insert("Y".to_string(), "X".to_string());
        let x = Type::Named("X".to_string());
        let y = Type::Named("Y".to_string());
        // Must terminate without hanging
        let _ = reg.is_subtype_of(&x, &y);
        let _ = reg.is_subtype_of(&y, &x);
    }
}
