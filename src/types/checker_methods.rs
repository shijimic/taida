use super::*;

impl TypeChecker {
    pub(super) fn check_method_args(
        &mut self,
        obj_type: &Type,
        method: &str,
        args: &[Expr],
        span: &Span,
    ) {
        // Get method arity: (min_args, max_args, param_types)
        // Only check for known methods with well-defined signatures.
        let method_sig: Option<(usize, usize, Vec<Type>)> = match obj_type {
            Type::Str => match method {
                "length" | "toString" => Some((0, 0, vec![])),
                "contains" | "startsWith" | "endsWith" => Some((1, 1, vec![Type::Str])),
                "indexOf" | "lastIndexOf" => Some((1, 1, vec![Type::Str])),
                "get" => Some((1, 1, vec![Type::Int])),
                // B11-4e: replace / replaceAll / split — fixed-string overload.
                // C12-6c: first argument may also be a :Regex BuchiPack
                // (the `Regex(...)` constructor return value). The type
                // checker uses Type::Unknown here so both Str and Named("Regex")
                // are accepted without bypassing the arity check, and the
                // runtime dispatches by inspecting the value's `__type` tag.
                "replace" | "replaceAll" => Some((2, 2, vec![Type::Unknown, Type::Str])),
                "split" => Some((1, 1, vec![Type::Unknown])),
                // C12-6c / C12B-031: match / search are Regex-only APIs.
                // The first argument must be a :Regex BuchiPack (the
                // `Regex(...)` constructor's return value). Rejecting
                // `str.match("a")` / `str.search("a")` at type-check time
                // unifies the failure mode across backends (previously
                // Interpreter/JS threw at runtime, Native silently returned
                // an empty match — see C12B-031).
                "match" | "search" => Some((1, 1, vec![Type::Named("Regex".to_string())])),
                _ => None,
            },
            Type::Int | Type::Float | Type::Num => match method {
                "toString" => Some((0, 0, vec![])),
                "isNaN" | "isInfinite" | "isFinite" | "isPositive" | "isNegative" | "isZero" => {
                    Some((0, 0, vec![]))
                }
                _ => None,
            },
            Type::Bool => match method {
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Bytes => match method {
                "length" => Some((0, 0, vec![])),
                "get" => Some((1, 1, vec![Type::Int])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::List(_) => match method {
                "length" => Some((0, 0, vec![])),
                "isEmpty" => Some((0, 0, vec![])),
                "first" | "last" | "max" | "min" => Some((0, 0, vec![])),
                "get" => Some((1, 1, vec![Type::Int])),
                "contains" => Some((1, 1, vec![Type::Unknown])), // element type varies
                "indexOf" | "lastIndexOf" => Some((1, 1, vec![Type::Unknown])),
                "any" | "all" | "none" => Some((1, 1, vec![Type::Unknown])), // predicate
                "take" | "drop" => Some((1, 1, vec![Type::Int])),
                "unique" | "reverse" | "sort" | "flatten" => Some((0, 0, vec![])),
                "map" | "flatMap" | "filter" => Some((1, 1, vec![Type::Unknown])),
                "reduce" | "fold" => Some((2, 2, vec![Type::Unknown, Type::Unknown])),
                "join" => Some((1, 1, vec![Type::Str])),
                "slice" => Some((2, 2, vec![Type::Int, Type::Int])),
                "push" | "append" => Some((1, 1, vec![Type::Unknown])),
                "concat" => Some((1, 1, vec![Type::Unknown])),
                "zip" => Some((1, 1, vec![Type::Unknown])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Named(name) if name == "HashMap" => match method {
                "get" => Some((1, 1, vec![Type::Unknown])),
                "set" => Some((2, 2, vec![Type::Unknown, Type::Unknown])),
                "remove" => Some((1, 1, vec![Type::Unknown])),
                "has" => Some((1, 1, vec![Type::Unknown])),
                "keys" | "values" | "entries" => Some((0, 0, vec![])),
                "size" | "isEmpty" => Some((0, 0, vec![])),
                "merge" => Some((1, 1, vec![Type::Unknown])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Named(name) if name == "Set" => match method {
                "add" | "remove" => Some((1, 1, vec![Type::Unknown])),
                "has" => Some((1, 1, vec![Type::Unknown])),
                "union" | "intersect" | "diff" => Some((1, 1, vec![Type::Unknown])),
                "toList" => Some((0, 0, vec![])),
                "size" | "isEmpty" => Some((0, 0, vec![])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Generic(name, _) if name == "Lax" => match method {
                "hasValue" | "isEmpty" => Some((0, 0, vec![])),
                "getOrDefault" => Some((1, 1, vec![Type::Unknown])),
                "map" | "flatMap" => Some((1, 1, vec![Type::Unknown])), // function arg
                "unmold" => Some((0, 0, vec![])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Generic(name, _) if name == "Result" => match method {
                "isSuccess" | "isError" => Some((0, 0, vec![])),
                "map" | "flatMap" | "mapError" => Some((1, 1, vec![Type::Unknown])),
                "getOrDefault" => Some((1, 1, vec![Type::Unknown])),
                "getOrThrow" => Some((0, 0, vec![])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            _ => {
                // N-66: For unknown/unresolved receiver types (Type::Unknown, Type::Any,
                // Type::Generic for non-Lax/Result/Async, user-defined Named types without
                // known method signatures), we skip method argument checking. This is
                // intentional: the checker cannot enumerate methods on types it does not
                // fully know. FL-22 handles the known-type case above.
                None
            }
        };

        // FL-22: Emit error for unknown methods on known concrete types
        if method_sig.is_none() && method != "toString" {
            let is_known_type = matches!(
                obj_type,
                Type::Str
                    | Type::Int
                    | Type::Float
                    | Type::Num
                    | Type::Bool
                    | Type::Bytes
                    | Type::List(_)
            ) || matches!(obj_type, Type::Named(n) if n == "HashMap" || n == "Set")
                || matches!(obj_type, Type::Generic(n, _) if n == "Lax" || n == "Result");
            if is_known_type {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1509] Unknown method '{}' on type {}. \
                         Hint: Check the method name for typos, or use a mold instead.",
                        method, obj_type
                    ),
                    span: span.clone(),
                });
            }
        }

        if let Some((min_args, max_args, param_types)) = method_sig {
            // Check arity
            if args.len() < min_args || args.len() > max_args {
                let arity_desc = if min_args == max_args {
                    format!("{}", min_args)
                } else {
                    format!("{}-{}", min_args, max_args)
                };
                self.errors.push(TypeError {
                    message: format!(
                        "[E1508] Method '{}' takes {} argument(s), got {}. \
                         Hint: Check the method signature and provide the correct number of arguments.",
                        method, arity_desc, args.len()
                    ),
                    span: span.clone(),
                });
            }
            // Check argument types
            for (i, arg) in args.iter().enumerate() {
                if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                    continue;
                }
                if let Some(expected_ty) = param_types.get(i) {
                    if *expected_ty == Type::Unknown {
                        continue;
                    }
                    let actual_ty = self.infer_expr_type(arg);
                    if actual_ty == Type::Unknown {
                        continue;
                    }
                    if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1508] Method '{}' argument {} has type {}, expected {}. \
                                 Hint: Pass a value of the correct type.",
                                method,
                                i + 1,
                                actual_ty,
                                expected_ty
                            ),
                            span: span.clone(),
                        });
                    }
                }
            }
        }
    }

    /// Infer the return type of a method call based on the receiver type and method name.
    /// Updated for v0.7.0: operation methods are abolished (mold-ified).
    /// Only state check methods and toString remain.
    pub(super) fn infer_method_return_type(&self, obj_type: &Type, method: &str) -> Type {
        match obj_type {
            Type::Str => match method {
                // State checks (v0.7.0 remaining methods)
                "length" => Type::Int,
                "contains" | "startsWith" | "endsWith" => Type::Bool,
                "indexOf" | "lastIndexOf" => Type::Int,
                "get" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                "toString" => Type::Str,
                // B11-4e: replace / replaceAll / split (fixed-string + C12-6 Regex overload)
                "replace" | "replaceAll" => Type::Str,
                "split" => Type::List(Box::new(Type::Str)),
                // C12-6c: match returns a :RegexMatch BuchiPack; search
                // returns an Int (char index or -1). We type `match` as
                // Named("RegexMatch") so later field access is dispatched
                // through the BuchiPack path at runtime.
                "match" => Type::Named("RegexMatch".to_string()),
                "search" => Type::Int,
                _ => Type::Unknown,
            },
            Type::Int | Type::Float | Type::Num => match method {
                // State checks (v0.7.0 remaining methods)
                "toString" => Type::Str,
                "isNaN" | "isInfinite" | "isFinite" | "isPositive" | "isNegative" | "isZero" => {
                    Type::Bool
                }
                _ => Type::Unknown,
            },
            Type::Bool => match method {
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::Bytes => match method {
                "length" => Type::Int,
                "get" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::List(inner) => match method {
                // State checks (v0.7.0 remaining methods)
                "length" => Type::Int,
                "isEmpty" => Type::Bool,
                "first" | "last" | "max" | "min" => {
                    Type::Generic("Lax".to_string(), vec![*inner.clone()])
                }
                "get" => Type::Generic("Lax".to_string(), vec![*inner.clone()]),
                "contains" => Type::Bool,
                "indexOf" | "lastIndexOf" => Type::Int,
                "any" | "all" | "none" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // JSON is an opaque primitive (molten iron) -- no methods allowed (v0.7.0)
            Type::Json => Type::Unknown,
            // Molten is an opaque primitive -- no methods allowed
            Type::Molten => Type::Unknown,
            // HashMap methods
            Type::Named(name) if name == "HashMap" => match method {
                "get" => Type::Generic("Lax".to_string(), vec![Type::Unknown]),
                "set" => Type::Unit,
                "remove" => Type::Unit,
                "has" => Type::Bool,
                "keys" => Type::List(Box::new(Type::Str)),
                "values" => Type::List(Box::new(Type::Unknown)),
                "entries" => Type::List(Box::new(Type::Unknown)),
                "size" => Type::Int,
                "merge" => Type::Named("HashMap".to_string()),
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // Set methods
            Type::Named(name) if name == "Set" => match method {
                "add" | "remove" => Type::Unit,
                "has" => Type::Bool,
                "union" | "intersect" | "diff" => Type::Named("Set".to_string()),
                "toList" => Type::List(Box::new(Type::Unknown)),
                "size" => Type::Int,
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // Lax methods
            Type::Generic(name, args) if name == "Lax" => match method {
                "hasValue" | "isEmpty" => Type::Bool,
                "getOrDefault" => args.first().cloned().unwrap_or(Type::Unknown),
                "map" | "flatMap" => obj_type.clone(),
                "unmold" => args.first().cloned().unwrap_or(Type::Unknown),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // Result methods
            Type::Generic(name, args) if name == "Result" => match method {
                "isSuccess" | "isError" => Type::Bool,
                "map" | "flatMap" | "mapError" => obj_type.clone(),
                "getOrDefault" => args.first().cloned().unwrap_or(Type::Unknown),
                "getOrThrow" => args.first().cloned().unwrap_or(Type::Unknown),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // Async methods
            Type::Generic(name, args) if name == "Async" => match method {
                "isPending" | "isFulfilled" | "isRejected" => Type::Bool,
                "map" => obj_type.clone(),
                "getOrDefault" => args.first().cloned().unwrap_or(Type::Unknown),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // Stream methods
            Type::Generic(name, _args) if name == "Stream" => match method {
                "length" => Type::Int,
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // For named types, check if they have known fields/methods
            Type::Named(type_name) => {
                if let Some(fields) = self.registry.get_type_fields(type_name) {
                    // Check if method name matches a field (could be a method field)
                    if let Some((_, ty)) = fields.iter().find(|(n, _)| n == method) {
                        ty.clone()
                    } else {
                        // toString is available on all types
                        if method == "toString" {
                            Type::Str
                        } else {
                            Type::Unknown
                        }
                    }
                } else if method == "toString" {
                    Type::Str
                } else {
                    Type::Unknown
                }
            }
            _ => {
                if method == "toString" {
                    Type::Str
                } else {
                    Type::Unknown
                }
            }
        }
    }
}
