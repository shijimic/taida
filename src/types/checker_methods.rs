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
                "indexOfLax" | "lastIndexOfLax" => Some((1, 1, vec![Type::Str])),
                "get" => Some((1, 1, vec![Type::Int])),
                // `replace` / `replaceAll` / `split` accept either a fixed
                // string or a Regex pack. The runtime dispatches by the value
                // tag, so the checker keeps the slot intentionally open while
                // still enforcing arity.
                "replace" | "replaceAll" => Some((2, 2, vec![Type::Any, Type::Str])),
                "split" => Some((1, 1, vec![Type::Any])),
                // `match` / `search` are Regex-only APIs.
                // The first argument must be a :Regex BuchiPack (the
                // `Regex(...)` constructor's return value).
                "match" | "search" => Some((1, 1, vec![Type::Named("Regex".to_string())])),
                "searchLax" => Some((1, 1, vec![Type::Named("Regex".to_string())])),
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
            Type::List(inner) => match method {
                "length" => Some((0, 0, vec![])),
                "isEmpty" => Some((0, 0, vec![])),
                "first" | "last" | "max" | "min" => Some((0, 0, vec![])),
                "get" => Some((1, 1, vec![Type::Int])),
                "contains" => Some((1, 1, vec![inner.as_ref().clone()])),
                "indexOf" | "lastIndexOf" => Some((1, 1, vec![inner.as_ref().clone()])),
                "indexOfLax" | "lastIndexOfLax" => Some((1, 1, vec![inner.as_ref().clone()])),
                // Pin predicate / mapper parameter types to the list element
                // type so higher-order method boundaries stay explicit.
                "any" | "all" | "none" => Some((
                    1,
                    1,
                    vec![Type::Function(
                        vec![inner.as_ref().clone()],
                        Box::new(Type::Bool),
                    )],
                )),
                "take" | "drop" => Some((1, 1, vec![Type::Int])),
                "unique" | "reverse" | "sort" | "flatten" => Some((0, 0, vec![])),
                "map" => Some((
                    1,
                    1,
                    vec![Type::Function(
                        vec![inner.as_ref().clone()],
                        Box::new(Type::Any),
                    )],
                )),
                "filter" => Some((
                    1,
                    1,
                    vec![Type::Function(
                        vec![inner.as_ref().clone()],
                        Box::new(Type::Bool),
                    )],
                )),
                "flatMap" => Some((
                    1,
                    1,
                    vec![Type::Function(
                        vec![inner.as_ref().clone()],
                        Box::new(Type::List(Box::new(Type::Any))),
                    )],
                )),
                // `fold` / `reduce` callback type depends on arg0 (`init`)
                // and is checked by the dedicated accumulator path below.
                // The static method signature stays arity-only here so the
                // generic param loop does not infer a weaker callback first.
                "reduce" | "fold" => Some((2, 2, vec![Type::Any, Type::Any])),
                "join" => Some((1, 1, vec![Type::Str])),
                "slice" => Some((2, 2, vec![Type::Int, Type::Int])),
                "push" | "append" => Some((1, 1, vec![inner.as_ref().clone()])),
                "concat" => Some((1, 1, vec![Type::List(Box::new(inner.as_ref().clone()))])),
                "zip" => Some((1, 1, vec![Type::List(Box::new(Type::Any))])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Named(name) if name == "HashMap" => match method {
                "get" => Some((1, 1, vec![Type::Any])),
                "set" => Some((2, 2, vec![Type::Any, Type::Any])),
                "remove" => Some((1, 1, vec![Type::Any])),
                "has" => Some((1, 1, vec![Type::Any])),
                "keys" | "values" | "entries" => Some((0, 0, vec![])),
                "size" | "isEmpty" => Some((0, 0, vec![])),
                "merge" => Some((1, 1, vec![Type::Any])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Generic(name, args) if name == "HashMap" => {
                let key = args.first().cloned().unwrap_or(Type::Unknown);
                let value = args.get(1).cloned().unwrap_or(Type::Unknown);
                match method {
                    "get" | "remove" | "has" => Some((1, 1, vec![key])),
                    "set" => Some((2, 2, vec![key, value])),
                    "keys" | "values" | "entries" => Some((0, 0, vec![])),
                    "size" | "isEmpty" => Some((0, 0, vec![])),
                    "merge" => Some((
                        1,
                        1,
                        vec![Type::Generic(
                            "HashMap".to_string(),
                            vec![key.clone(), value.clone()],
                        )],
                    )),
                    "toString" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Named(name) if name == "Set" => match method {
                "add" | "remove" => Some((1, 1, vec![Type::Any])),
                "has" => Some((1, 1, vec![Type::Any])),
                "union" | "intersect" | "diff" => Some((1, 1, vec![Type::Any])),
                "toList" => Some((0, 0, vec![])),
                "size" | "isEmpty" => Some((0, 0, vec![])),
                "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            Type::Generic(name, args) if name == "Set" => {
                let value = args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "add" | "remove" | "has" => Some((1, 1, vec![value])),
                    "union" | "intersect" | "diff" => Some((
                        1,
                        1,
                        vec![Type::Generic("Set".to_string(), vec![value.clone()])],
                    )),
                    "toList" => Some((0, 0, vec![])),
                    "size" | "isEmpty" => Some((0, 0, vec![])),
                    "toString" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Generic(name, inner_args) if name == "Lax" => {
                // The `default` arg of `getOrDefault` must match the Lax
                // inner type T. PHILOSOPHY I forbids silent type drift.
                // `getOrThrow` is Result-only on the runtime side; Lax does
                // not currently surface it, so we leave the existing arity-only
                // signature for Result below.
                //
                // map/flatMap signatures are pinned to `Function([T], U)` /
                // `Function([T], Lax[U])` so argument-type and return-wrapper
                // mismatch is caught at type-check time via [E1508].
                let inner = inner_args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "hasValue" | "isEmpty" => Some((0, 0, vec![])),
                    "getOrDefault" => Some((1, 1, vec![inner.clone()])),
                    "map" => Some((
                        1,
                        1,
                        vec![Type::Function(vec![inner.clone()], Box::new(Type::Any))],
                    )),
                    "flatMap" => Some((
                        1,
                        1,
                        vec![Type::Function(
                            vec![inner.clone()],
                            Box::new(Type::Generic("Lax".to_string(), vec![Type::Any])),
                        )],
                    )),
                    "errorInfo" => Some((0, 0, vec![])),
                    "unmold" => Some((0, 0, vec![])),
                    "toString" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Generic(name, inner_args) if name == "Result" => {
                // Match Lax/Result behaviour: both accumulators (success type
                // at index 0) get strict `default` arg type checking.
                //
                //   map(fn: T -> U) -> Result[U, P]              (error type P を保存)
                //   flatMap(fn: T -> Result[U, P]) -> Result[U, P]  (方針 A: error type 保存 strict)
                //   mapError(fn: P -> Q) -> Result[T, Q]
                let success_ty = inner_args.first().cloned().unwrap_or(Type::Unknown);
                let error_ty = inner_args.get(1).cloned().unwrap_or(Type::Unknown);
                match method {
                    "isSuccess" | "isError" => Some((0, 0, vec![])),
                    "map" => Some((
                        1,
                        1,
                        vec![Type::Function(
                            vec![success_ty.clone()],
                            Box::new(Type::Any),
                        )],
                    )),
                    // 方針 A: return Result の error type が receiver と一致必須
                    "flatMap" => Some((
                        1,
                        1,
                        vec![Type::Function(
                            vec![success_ty.clone()],
                            Box::new(Type::Generic(
                                "Result".to_string(),
                                vec![Type::Any, error_ty.clone()],
                            )),
                        )],
                    )),
                    "mapError" => Some((
                        1,
                        1,
                        vec![Type::Function(vec![error_ty.clone()], Box::new(Type::Any))],
                    )),
                    "getOrDefault" => Some((1, 1, vec![success_ty])),
                    "getOrThrow" => Some((0, 0, vec![])),
                    "toString" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Generic(name, inner_args) if name == "Async" => {
                let inner = inner_args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "isPending" | "isFulfilled" | "isRejected" => Some((0, 0, vec![])),
                    "map" => Some((
                        1,
                        1,
                        vec![Type::Function(vec![inner.clone()], Box::new(Type::Any))],
                    )),
                    "getOrDefault" => Some((1, 1, vec![inner])),
                    "toString" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Generic(name, _) if name == "Gorillax" || name == "RelaxedGorillax" => {
                match method {
                    "hasValue" | "isEmpty" | "errorInfo" | "toString" => Some((0, 0, vec![])),
                    "relax" if name == "Gorillax" => Some((0, 0, vec![])),
                    _ => None,
                }
            }
            Type::Error(_) => match method {
                "errorInfo" | "throw" | "toString" => Some((0, 0, vec![])),
                _ => None,
            },
            // A function-valued pack field invoked via `pack.fn(arg)` is
            // syntactically a MethodCall, but the receiver is a BuchiPack
            // and the "method" is really a stored function value. Surface
            // its declared signature so the regular boundary subtype check
            // applies.
            Type::BuchiPack(fields) => {
                fields
                    .iter()
                    .find(|(name, _)| name == method)
                    .and_then(|(_, ty)| match ty {
                        Type::Function(params, _) => {
                            // `Unit => :T` is a zero-argument signature
                            // marker. Treat the single Unit param as an
                            // empty signature so callers can write
                            // `pack.fn()` without the arity check
                            // wrongly demanding one argument.
                            let effective = if params.len() == 1 && params[0] == Type::Unit {
                                vec![]
                            } else {
                                params.clone()
                            };
                            Some((effective.len(), effective.len(), effective))
                        }
                        _ => None,
                    })
            }
            // A function-valued field on a declared `Named` type must obey
            // the same boundary discipline as a `BuchiPack` literal. Walk
            // the type's registered fields and surface the signature of any
            // `Type::Function` field that matches the called name.
            Type::Named(type_name) => self.registry.get_type_fields(type_name).and_then(|fields| {
                fields
                    .iter()
                    .find(|(name, _)| name == method)
                    .and_then(|(_, ty)| match ty {
                        Type::Function(params, _) => {
                            // See `Type::BuchiPack` arm: `Unit => :T` marks a
                            // zero-argument signature.
                            let effective = if params.len() == 1 && params[0] == Type::Unit {
                                vec![]
                            } else {
                                params.clone()
                            };
                            Some((effective.len(), effective.len(), effective))
                        }
                        _ => None,
                    })
            }),
            _ => {
                // For unknown/unresolved receiver types, generic receivers
                // without method signatures, and user-defined Named types
                // without function-valued fields, skip method argument
                // checking. The checker cannot enumerate methods on types it
                // does not fully know.
                None
            }
        };

        // errorInfo intentionally uses an explicit allow-list. Accepting any
        // `(type, message)` shaped pack would reintroduce duck typing here.
        // The canonical ErrorInfo carrier currently lives on Lax + Gorillax /
        // RelaxedGorillax + Error. Result / Async runtime answers do not yet
        // exist on any backend, so admitting them here would let type-correct
        // programs panic across 2/3 backends at runtime.
        if method == "errorInfo"
            && !matches!(
                obj_type,
                Type::Generic(name, _)
                    if name == "Gorillax"
                        || name == "RelaxedGorillax"
                        || name == "Lax"
            )
            && !matches!(obj_type, Type::Error(_))
            && !matches!(obj_type, Type::Unknown | Type::Any)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1509] Unknown method 'errorInfo' on type {}. \
                     Hint: errorInfo() is available on Lax, Gorillax, RelaxedGorillax, and Error values.",
                    obj_type
                ),
                span: span.clone(),
            });
        }

        // FL-22: Emit error for unknown methods on known concrete types
        if method_sig.is_none() && method != "toString" && method != "errorInfo" {
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
                || matches!(obj_type, Type::Generic(n, _) if n == "Lax" || n == "Result" || n == "Gorillax" || n == "RelaxedGorillax")
                || matches!(obj_type, Type::Error(_));
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
                    if matches!(expected_ty, Type::Unknown | Type::Any) {
                        continue;
                    }
                    // Lambda bidirectional inference: when the expected param
                    // type is Function([T], _), hint the
                    // lambda's untyped params with T. This lets users write
                    // `obj.map(_ x = x + 1)` without an explicit type annotation
                    // and still benefit from full pin checking.
                    let actual_ty = self.infer_expr_type_with_expected(arg, expected_ty);
                    if actual_ty == Type::Unknown {
                        continue;
                    }
                    // If the higher-order argument is a function value whose
                    // return remains unresolved, reject it when the expected
                    // wrapper demands a more specific return type.
                    if let (Type::Function(_, act_ret), Type::Function(_, exp_ret)) =
                        (&actual_ty, expected_ty)
                        && matches!(act_ret.as_ref(), Type::Unknown)
                        && !matches!(exp_ret.as_ref(), Type::Unknown | Type::Any)
                    {
                        let arg_label = match arg {
                            Expr::Ident(name, _) => format!("'{}'", name),
                            _ => format!("{}", i + 1),
                        };
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1508] Method '{}' argument {} is a function value with no \
                                 inferable return type. \
                                 Hint: Add an explicit return type annotation (e.g. `=> :{}`) \
                                 to the function definition.",
                                method, arg_label, exp_ret
                            ),
                            span: span.clone(),
                        });
                        continue;
                    }
                    // If the higher-order argument is a named function
                    // reference whose params include unresolved types and the
                    // expected slot demands a concrete param type, reject it.
                    // Lambdas are exempt because they are bidirectionally
                    // inferred from `expected_ty`.
                    //
                    // A fully unannotated named function (`f x = x * 2`) is
                    // treated as gradual typing: body inference fills in the
                    // gaps unless the author has committed to a partial
                    // annotation such as an explicit return type or one
                    // explicit parameter type.
                    if matches!(arg, Expr::Ident(_, _))
                        && let (Type::Function(act_params, act_ret), Type::Function(exp_params, _)) =
                            (&actual_ty, expected_ty)
                    {
                        // Strict reject only fires when at least one param has
                        // an explicit annotation. The two gradual-typing shapes
                        //   (a) fully unannotated:  `f x = ...`
                        //   (b) return-annotated, params unannotated:
                        //       `f x = x * 2 => :Int`
                        // are admitted; body inference covers params when the
                        // author has not written annotations.
                        // Mixed shapes (one param annotated, another
                        // not) still trip the strict rule.
                        let all_params_unannotated =
                            act_params.iter().all(|t| matches!(t, Type::Unknown));
                        if !all_params_unannotated {
                            let mismatch =
                                act_params.iter().zip(exp_params.iter()).position(|(a, e)| {
                                    matches!(a, Type::Unknown) && !matches!(e, Type::Unknown)
                                });
                            if let Some(pos) = mismatch {
                                let arg_label = match arg {
                                    Expr::Ident(name, _) => format!("'{}'", name),
                                    _ => format!("{}", i + 1),
                                };
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1508] Method '{}' argument {} ({}) has an unannotated \
                                         parameter at position {}. \
                                         Hint: Add an explicit type annotation (e.g. `param: {}`) \
                                         to the function definition.",
                                        method,
                                        i + 1,
                                        arg_label,
                                        pos + 1,
                                        exp_params[pos]
                                    ),
                                    span: span.clone(),
                                });
                                continue;
                            }
                        }
                        let _ = act_ret;
                    }
                    // Apply the strict function-arg subtype rule at every
                    // method-call boundary, including registry-resolved Named
                    // and BuchiPack structural paths. This forbids `Int →
                    // Float` implicit widening across function/method-arg
                    // slots while preserving the wider rule for direct
                    // numeric arithmetic and assignment.
                    let pass = if let (
                        Type::Function(actual_params, actual_ret),
                        Type::Function(expected_params, expected_ret),
                    ) = (&actual_ty, expected_ty)
                    {
                        actual_params.len() == expected_params.len()
                            && actual_params.iter().zip(expected_params.iter()).all(
                                |(actual, expected)| {
                                    matches!(expected, Type::Unknown | Type::Any)
                                        || self
                                            .registry
                                            .is_function_arg_subtype_of(expected, actual)
                                },
                            )
                            && (matches!(expected_ret.as_ref(), Type::Unknown | Type::Any)
                                || self.method_hof_return_compatible(
                                    actual_ret.as_ref(),
                                    expected_ret.as_ref(),
                                ))
                    } else {
                        self.registry
                            .is_function_arg_subtype_of(&actual_ty, expected_ty)
                    };
                    if Self::contains_unknown(&actual_ty) && !Self::contains_unknown(expected_ty) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1508] Method '{}' argument {} has type {}, expected {}. \
                                 Hint: Add annotations or simplify the function body so inference can resolve the argument type.",
                                method,
                                i + 1,
                                actual_ty,
                                expected_ty
                            ),
                            span: span.clone(),
                        });
                        continue;
                    }
                    if !pass {
                        let hint = match expected_ty {
                            Type::Function(exp_params, _) => format!(
                                "Hint: Pass a function whose parameter types match {}.",
                                exp_params
                                    .iter()
                                    .map(|p| p.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            _ => "Hint: Pass a value of the correct type.".to_string(),
                        };
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1508] Method '{}' argument {} has type {}, expected {}. \
                                 {}",
                                method,
                                i + 1,
                                actual_ty,
                                expected_ty,
                                hint
                            ),
                            span: span.clone(),
                        });
                    }
                }
            }
            self.check_list_accumulator_method_args(obj_type, method, args, span);
        }
    }

    fn method_hof_return_compatible(&self, actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            // Method signatures use `?` as a local generic placeholder for
            // HOF outputs such as `Lax[Any]`, `Result[Any, P]`, and `@[Any]`.
            // Keep that wildcard scoped here so unresolved inference remains
            // separate from explicit "any concrete type" slots.
            (_, Type::Unknown) => true,
            (_, Type::Any) => true,
            (Type::List(actual_inner), Type::List(expected_inner)) => {
                self.method_hof_return_compatible(actual_inner, expected_inner)
            }
            (
                Type::Generic(actual_name, actual_args),
                Type::Generic(expected_name, expected_args),
            ) if actual_name == expected_name && actual_args.len() == expected_args.len() => {
                actual_args
                    .iter()
                    .zip(expected_args.iter())
                    .all(|(actual_arg, expected_arg)| {
                        self.method_hof_return_compatible(actual_arg, expected_arg)
                    })
            }
            _ => self.registry.is_subtype_of(actual, expected),
        }
    }

    fn check_list_accumulator_method_args(
        &mut self,
        obj_type: &Type,
        method: &str,
        args: &[Expr],
        span: &Span,
    ) {
        if !matches!(method, "fold" | "reduce") {
            return;
        }
        let Type::List(inner) = obj_type else {
            return;
        };
        if args.len() != 2 {
            return;
        }
        let init_ty = self.infer_expr_type(&args[0]);
        if init_ty == Type::Unknown || Self::contains_unknown(&init_ty) {
            let fallback_fn = Type::Function(
                vec![Type::Unknown, inner.as_ref().clone()],
                Box::new(Type::Unknown),
            );
            let actual_ty = self.infer_expr_type_with_expected(&args[1], &fallback_fn);
            if actual_ty == Type::Unknown {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1508] Method '{}' argument 2 needs a function of type {}; the callback could not be inferred. \
                         Hint: Add parameter and return type annotations.",
                        method, fallback_fn
                    ),
                    span: span.clone(),
                });
                return;
            }
            if !self
                .registry
                .is_function_arg_subtype_of(&actual_ty, &fallback_fn)
            {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1508] Method '{}' argument 2 has type {}, expected {}. \
                         Hint: The callback must accept the list element type even when the init type is unresolved.",
                        method, actual_ty, fallback_fn
                    ),
                    span: span.clone(),
                });
            }
            return;
        }
        let expected_fn = Type::Function(
            vec![init_ty.clone(), inner.as_ref().clone()],
            Box::new(init_ty.clone()),
        );
        let actual_ty = self.infer_expr_type_with_expected(&args[1], &expected_fn);
        if actual_ty == Type::Unknown {
            self.errors.push(TypeError {
                message: format!(
                    "[E1508] Method '{}' argument 2 needs a function of type {}; the callback could not be inferred. \
                     Hint: Add parameter and return type annotations.",
                    method, expected_fn
                ),
                span: span.clone(),
            });
            return;
        }
        if Self::contains_unknown(&actual_ty) && !Self::contains_unknown(&expected_fn) {
            self.errors.push(TypeError {
                message: format!(
                    "[E1508] Method '{}' argument 2 has type {}, expected {}. \
                     Hint: The accumulator parameter and callback return must resolve to the init value type {}.",
                    method, actual_ty, expected_fn, init_ty
                ),
                span: span.clone(),
            });
            return;
        }
        if !self
            .registry
            .is_function_arg_subtype_of(&actual_ty, &expected_fn)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1508] Method '{}' argument 2 has type {}, expected {}. \
                     Hint: The accumulator parameter and callback return must match the init value type {}.",
                    method, actual_ty, expected_fn, init_ty
                ),
                span: span.clone(),
            });
        }
    }

    /// Bidirectional Lambda inference.
    ///
    /// Infer a Lambda's type using the expected
    /// `Type::Function(expected_params, _)` to fill in missing param
    /// annotations. If the lambda has explicit type annotations on
    /// params, those win and must be compatible with the expected
    /// parameter type. If a param has no annotation, the expected param
    /// type at the same index is used as a hint.
    ///
    /// This is the only place where a bidirectional hint flows into
    /// Lambda inference. The general lambda inference path remains
    /// unchanged (no annotation -> `Type::Unknown`).
    pub(super) fn infer_lambda_with_hint(&mut self, expr: &Expr, expected: &Type) -> Type {
        let (Expr::Lambda(params, body, _), Type::Function(expected_params, _)) = (expr, expected)
        else {
            return self.infer_expr_type(expr);
        };
        // Compute resolved param types: explicit annotation wins, else hint.
        let param_types: Vec<Type> = params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if let Some(annotation) = &p.type_annotation {
                    let annotated = self.registry.resolve_type(annotation);
                    if let Some(expected_param) = expected_params.get(i)
                        && *expected_param != Type::Unknown
                        && !self.registry.is_subtype_of(&annotated, expected_param)
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1528] Lambda parameter '{}' annotation {} conflicts with expected function parameter {}.",
                                p.name, annotated, expected_param
                            ),
                            span: p.span.clone(),
                        });
                    }
                    annotated
                } else {
                    expected_params.get(i).cloned().unwrap_or(Type::Unknown)
                }
            })
            .collect();
        // Push a temporary scope for the lambda body with hinted param types.
        self.push_scope();
        for (i, p) in params.iter().enumerate() {
            self.define_var(
                &p.name,
                param_types.get(i).cloned().unwrap_or(Type::Unknown),
            );
        }
        let ret_type = self.infer_expr_type(body);
        self.pop_scope();
        let fn_ty = Type::Function(param_types, Box::new(ret_type));
        // Last-write wins: idempotent record overrides any earlier path.
        self.typed_expr_table.record(expr, fn_ty.clone());
        fn_ty
    }

    /// Infer the return type of a method call **using the actual
    /// lambda/fn arguments** to pin generic parameters (U / Q in
    /// `Lax[T].map(fn: T -> U) -> Lax[U]` etc.).
    ///
    /// Falls back to `infer_method_return_type` for methods that don't
    /// need arg-aware inference. The MethodCall arm in
    /// `infer_expr_type_inner` calls this variant.
    pub(super) fn infer_method_return_type_with_args(
        &mut self,
        obj_type: &Type,
        method: &str,
        args: &[Expr],
    ) -> Type {
        if method == "set" {
            match obj_type {
                Type::Named(name) if name == "HashMap" => {
                    let key = args
                        .first()
                        .map(|arg| self.infer_expr_type(arg))
                        .unwrap_or(Type::Unknown);
                    if let Some(value) = args.get(1) {
                        self.infer_expr_type(value);
                    }
                    return Type::Generic("HashMap".to_string(), vec![key, Type::Any]);
                }
                Type::Generic(name, inner_args) if name == "HashMap" => {
                    let existing_key = inner_args.first().cloned().unwrap_or(Type::Unknown);
                    let existing_value = inner_args.get(1).cloned().unwrap_or(Type::Unknown);
                    let key = if matches!(existing_key, Type::Unknown) {
                        args.first()
                            .map(|arg| self.infer_expr_type(arg))
                            .unwrap_or(Type::Unknown)
                    } else {
                        existing_key
                    };
                    let value = if matches!(existing_value, Type::Unknown) {
                        args.get(1)
                            .map(|arg| self.infer_expr_type(arg))
                            .unwrap_or(Type::Unknown)
                    } else {
                        existing_value
                    };
                    return Type::Generic("HashMap".to_string(), vec![key, value]);
                }
                _ => {}
            }
        }

        if method == "merge"
            && matches!(obj_type, Type::Named(name) if name == "HashMap")
            && let Some(arg) = args.first()
        {
            let other = self.infer_expr_type(arg);
            if matches!(&other, Type::Generic(name, _) if name == "HashMap") {
                return other;
            }
        }

        // Only Lax/Result/Async map/flatMap/mapError need arg-aware inference.
        // Other methods use the existing arg-less variant.
        if let Type::Generic(name, inner_args) = obj_type {
            let success_ty = inner_args.first().cloned().unwrap_or(Type::Unknown);
            let error_ty = inner_args.get(1).cloned().unwrap_or(Type::Unknown);

            // Helper: extract the lambda's actual return type (U) from
            // args[0]. The MethodCall arm calls `check_method_args`
            // immediately before this helper, and that path already
            // records the bidirectional-hinted lambda type into the
            // typed table. Reusing that record avoids re-inferring the
            // lambda body — which would double-evaluate side-effecting
            // checker work like duplicate `[E1508]` emission and grow
            // cost quadratically through nested chains.
            let lambda_ret = |this: &mut Self, expected_param: &Type| -> Type {
                let Some(arg) = args.first() else {
                    return Type::Unknown;
                };
                if let Some(Type::Function(_, ret)) = this.typed_expr_table.lookup(arg) {
                    return (**ret).clone();
                }
                let expected_fn =
                    Type::Function(vec![expected_param.clone()], Box::new(Type::Unknown));
                let inferred = this.infer_lambda_with_hint(arg, &expected_fn);
                if let Type::Function(_, ret) = inferred {
                    return *ret;
                }
                Type::Unknown
            };

            match (name.as_str(), method) {
                ("Lax", "map") => {
                    let u = lambda_ret(self, &success_ty);
                    return Type::Generic("Lax".to_string(), vec![u]);
                }
                ("Lax", "flatMap") => {
                    // Lambda returns Lax[U]; extract U for the chain return.
                    let ret = lambda_ret(self, &success_ty);
                    if let Type::Generic(rn, ra) = &ret
                        && rn == "Lax"
                    {
                        return Type::Generic(
                            "Lax".to_string(),
                            vec![ra.first().cloned().unwrap_or(Type::Unknown)],
                        );
                    }
                    return Type::Generic("Lax".to_string(), vec![Type::Unknown]);
                }
                ("Async", "map") => {
                    let u = lambda_ret(self, &success_ty);
                    return Type::Generic("Async".to_string(), vec![u]);
                }
                ("Result", "map") => {
                    let u = lambda_ret(self, &success_ty);
                    return Type::Generic("Result".to_string(), vec![u, error_ty]);
                }
                ("Result", "flatMap") => {
                    // Lambda returns Result[U, P]; extract U; preserve receiver's P (方針 A).
                    let ret = lambda_ret(self, &success_ty);
                    if let Type::Generic(rn, ra) = &ret
                        && rn == "Result"
                    {
                        return Type::Generic(
                            "Result".to_string(),
                            vec![ra.first().cloned().unwrap_or(Type::Unknown), error_ty],
                        );
                    }
                    return Type::Generic("Result".to_string(), vec![Type::Unknown, error_ty]);
                }
                ("Result", "mapError") => {
                    let q = lambda_ret(self, &error_ty);
                    return Type::Generic("Result".to_string(), vec![success_ty, q]);
                }
                _ => {}
            }
        }
        if let Type::List(_) = obj_type
            && matches!(method, "fold" | "reduce")
            && let Some(init) = args.first()
        {
            return self
                .typed_expr_table
                .lookup(init)
                .cloned()
                .unwrap_or_else(|| self.infer_expr_type(init));
        }
        // Delegate every non-Lax/Result/Async receiver back to the
        // arg-less variant. The Named-pack arm there unwraps a
        // function-typed field's declared return (`Function(_, R) -> R`)
        // for callers like `obj.someField(7)`, so reusing it here keeps
        // a single source of truth and avoids duplicating that arm on
        // both code paths.
        self.infer_method_return_type(obj_type, method)
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
                "indexOfLax" | "lastIndexOfLax" => {
                    Type::Generic("Lax".to_string(), vec![Type::Int])
                }
                "get" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                "toString" => Type::Str,
                // replace / replaceAll / split accept fixed strings and Regex values.
                "replace" | "replaceAll" => Type::Str,
                "split" => Type::List(Box::new(Type::Str)),
                // match returns a RegexMatch pack; search returns an Int
                // character index. Typing `match` as Named("RegexMatch")
                // preserves later field access lowering.
                "match" => Type::Named("RegexMatch".to_string()),
                "search" => Type::Int,
                "searchLax" => Type::Generic("Lax".to_string(), vec![Type::Int]),
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
                "indexOfLax" | "lastIndexOfLax" => {
                    Type::Generic("Lax".to_string(), vec![Type::Int])
                }
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
                "get" => Type::Generic("Lax".to_string(), vec![Type::Any]),
                "set" | "remove" | "merge" => Type::Named("HashMap".to_string()),
                "has" => Type::Bool,
                "keys" => Type::List(Box::new(Type::Str)),
                "values" => Type::List(Box::new(Type::Any)),
                "entries" => Type::List(Box::new(Type::Any)),
                "size" => Type::Int,
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::Generic(name, args) if name == "HashMap" => {
                let key = args.first().cloned().unwrap_or(Type::Unknown);
                let value = args.get(1).cloned().unwrap_or(Type::Unknown);
                match method {
                    "get" => Type::Generic("Lax".to_string(), vec![value.clone()]),
                    "set" | "remove" | "merge" => obj_type.clone(),
                    "has" | "isEmpty" => Type::Bool,
                    "keys" => Type::List(Box::new(key)),
                    "values" => Type::List(Box::new(value)),
                    "entries" => Type::List(Box::new(Type::Unknown)),
                    "size" => Type::Int,
                    "toString" => Type::Str,
                    _ => Type::Unknown,
                }
            }
            // Set methods
            Type::Named(name) if name == "Set" => match method {
                "add" | "remove" | "union" | "intersect" | "diff" => Type::Named("Set".to_string()),
                "has" => Type::Bool,
                "toList" => Type::List(Box::new(Type::Unknown)),
                "size" => Type::Int,
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::Generic(name, args) if name == "Set" => {
                let value = args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "add" | "remove" | "union" | "intersect" | "diff" => obj_type.clone(),
                    "has" | "isEmpty" => Type::Bool,
                    "toList" => Type::List(Box::new(value)),
                    "size" => Type::Int,
                    "toString" => Type::Str,
                    _ => Type::Unknown,
                }
            }
            // Lax methods
            Type::Generic(name, args) if name == "Lax" => match method {
                "hasValue" | "isEmpty" => Type::Bool,
                "getOrDefault" => args.first().cloned().unwrap_or(Type::Unknown),
                "map" | "flatMap" => obj_type.clone(),
                "errorInfo" => Type::Generic(
                    "Lax".to_string(),
                    vec![Type::Named("ErrorInfo".to_string())],
                ),
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
            Type::Generic(name, args) if name == "Gorillax" => match method {
                "hasValue" | "isEmpty" => Type::Bool,
                "relax" => Type::Generic("RelaxedGorillax".to_string(), args.clone()),
                "errorInfo" => Type::Generic(
                    "Lax".to_string(),
                    vec![Type::Named("ErrorInfo".to_string())],
                ),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::Generic(name, _) if name == "RelaxedGorillax" => match method {
                "hasValue" | "isEmpty" => Type::Bool,
                "errorInfo" => Type::Generic(
                    "Lax".to_string(),
                    vec![Type::Named("ErrorInfo".to_string())],
                ),
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            Type::Error(_) => match method {
                "errorInfo" => Type::Generic(
                    "Lax".to_string(),
                    vec![Type::Named("ErrorInfo".to_string())],
                ),
                "throw" => Type::Unknown,
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
                        // When the matched field is a function type
                        // (e.g. `Predicate = @(check: Int => :Bool)`),
                        // the method-call result is the function's
                        // declared return type, not the function type
                        // itself. Without unwrapping here the typed
                        // table records `Type::Function(...)` and Bool
                        // detection collapses to false for callers like
                        // `predicate.check(x)`.
                        if let Type::Function(_, ret) = ty {
                            return (**ret).clone();
                        }
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
