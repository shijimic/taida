use super::method_spec::{builtin_method_spec, builtin_recv_of, render_return_kind};
use super::*;

impl TypeChecker {
    /// Static arity/signature table for builtin-type methods, split out of
    /// `check_method_args` so the spec can be exercised directly by the
    /// builtin-method spec tests. Pure factoring: body unchanged.
    /// Argument types for a builtin method, keyed by receiver and name.
    /// The spec table is argless (these depend on the receiver's element
    /// types), so this is the one place arg types live; `builtin_method_signature`
    /// pairs them with the table arity. Only methods the table lists need a
    /// real entry — absent names are filtered out by the table first, so the
    /// `_ => vec![]` arms (nullary receivers, state checks) are never observed
    /// for a non-method name.
    fn builtin_method_arg_types(obj_type: &Type, method: &str) -> Vec<Type> {
        match obj_type {
            Type::Str => match method {
                "contains" | "startsWith" | "endsWith" | "indexOf" | "lastIndexOf"
                | "indexOfLax" | "lastIndexOfLax" => vec![Type::Str],
                "get" => vec![Type::Int],
                "replace" | "replaceAll" => vec![Type::Any, Type::Str],
                "split" => vec![Type::Any],
                "match" | "search" | "searchLax" => vec![Type::Named("Regex".to_string())],
                _ => vec![],
            },
            Type::Bytes => match method {
                "get" => vec![Type::Int],
                _ => vec![],
            },
            Type::List(inner) => match method {
                "get" => vec![Type::Int],
                "contains" | "indexOf" | "lastIndexOf" | "indexOfLax" | "lastIndexOfLax" => {
                    vec![inner.as_ref().clone()]
                }
                "any" | "all" | "none" => vec![Type::Function(
                    vec![inner.as_ref().clone()],
                    Box::new(Type::Bool),
                )],
                "reduce" | "fold" => vec![Type::Any, Type::Any],
                _ => vec![],
            },
            Type::Named(name) if name == "HashMap" => match method {
                "get" | "remove" | "has" | "merge" => vec![Type::Any],
                "set" => vec![Type::Any, Type::Any],
                _ => vec![],
            },
            Type::Generic(name, args) if name == "HashMap" => {
                let key = args.first().cloned().unwrap_or(Type::Unknown);
                let value = args.get(1).cloned().unwrap_or(Type::Unknown);
                match method {
                    "get" | "remove" | "has" => vec![key],
                    "set" => vec![key, value],
                    "merge" => vec![Type::Generic("HashMap".to_string(), vec![key, value])],
                    _ => vec![],
                }
            }
            Type::Named(name) if name == "Set" => match method {
                "add" | "remove" | "has" | "union" | "intersect" | "diff" => vec![Type::Any],
                _ => vec![],
            },
            Type::Generic(name, args) if name == "Set" => {
                // every Set method slot stays
                // element-lenient. Cross-element-type membership and mutation
                // are pinned runtime semantics (enum ordinal == Int,
                // Int in Set[Float]; Bool into
                // a value-tagged Set, value-tag dedup parity), so the checker
                // must not reject what the reference interpreter accepts.
                // Mirrors the bare Named("Set") receiver arm above, which
                // already passes Any for all six methods. Element precision
                // lives at the binding site instead
                // (`Set[Str] <= setOf(@[1, 2])` stays rejected).
                let _ = args;
                match method {
                    "add" | "remove" | "has" | "union" | "intersect" | "diff" => vec![Type::Any],
                    _ => vec![],
                }
            }
            Type::Generic(name, inner_args) if name == "Lax" => {
                let inner = inner_args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "getOrDefault" => vec![inner],
                    "map" => vec![Type::Function(vec![inner], Box::new(Type::Any))],
                    "flatMap" => vec![Type::Function(
                        vec![inner],
                        Box::new(Type::Generic("Lax".to_string(), vec![Type::Any])),
                    )],
                    _ => vec![],
                }
            }
            Type::Generic(name, inner_args) if name == "Result" => {
                let success_ty = inner_args.first().cloned().unwrap_or(Type::Unknown);
                let error_ty = inner_args.get(1).cloned().unwrap_or(Type::Unknown);
                match method {
                    "map" => vec![Type::Function(vec![success_ty], Box::new(Type::Any))],
                    "flatMap" => vec![Type::Function(
                        vec![success_ty],
                        Box::new(Type::Generic(
                            "Result".to_string(),
                            vec![Type::Any, error_ty],
                        )),
                    )],
                    "mapError" => vec![Type::Function(vec![error_ty], Box::new(Type::Any))],
                    "getOrDefault" => vec![success_ty],
                    _ => vec![],
                }
            }
            Type::Generic(name, inner_args) if name == "Async" => {
                let inner = inner_args.first().cloned().unwrap_or(Type::Unknown);
                match method {
                    "map" => vec![Type::Function(vec![inner], Box::new(Type::Any))],
                    "getOrDefault" => vec![inner],
                    _ => vec![],
                }
            }
            // Num / Bool / Gorillax / RelaxedGorillax / Error builtin methods
            // are all nullary.
            _ => vec![],
        }
    }

    pub(super) fn builtin_method_signature(
        &mut self,
        obj_type: &Type,
        method: &str,
    ) -> Option<(usize, usize, Vec<Type>)> {
        // Statically enumerable receivers take their arity from the spec
        // table (the SSOT the universe cross-test pins to this function);
        // argument types, which depend on the receiver's element types,
        // come from `builtin_method_arg_types`. A builtin receiver whose
        // method the table omits has no signature here — except Error,
        // whose unknown names resolve against user-defined members.
        if let Some(recv) = builtin_recv_of(obj_type) {
            if let Some(spec) = builtin_method_spec(recv, method) {
                let args = Self::builtin_method_arg_types(obj_type, method);
                return Some((spec.min_args, spec.max_args, args));
            }
            if let Type::Error(error_name) = obj_type {
                return self.named_method_signature(error_name, method);
            }
            return None;
        }
        match obj_type {
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
                            // `Unit => :T` is a zero-argument signature marker.
                            // Treat the single Unit param as an empty signature
                            // so callers can write `pack.fn()` without the arity
                            // check wrongly demanding one argument.
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
            // Declared methods and function-valued fields on a `Named` type
            // must obey the same boundary discipline as a `BuchiPack` literal.
            Type::Named(type_name) => self.named_method_signature(type_name, method),
            _ => {
                // Unknown/unresolved receivers, generic receivers without
                // method signatures, and user Named types without
                // function-valued fields: the checker cannot enumerate
                // methods on types it does not fully know.
                None
            }
        }
    }

    pub(super) fn check_method_args(
        &mut self,
        obj_type: &Type,
        method: &str,
        args: &[Expr],
        span: &Span,
    ) {
        // Get method arity: (min_args, max_args, param_types)
        // Only check for known methods with well-defined signatures.
        let method_sig: Option<(usize, usize, Vec<Type>)> =
            self.builtin_method_signature(obj_type, method);

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
                || matches!(obj_type, Type::Named(n) if self.mold_field_defs.contains_key(n) || self.registry.get_type_fields(n).is_some())
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

    fn named_method_signature(
        &self,
        type_name: &str,
        method: &str,
    ) -> Option<(usize, usize, Vec<Type>)> {
        if let Some(fields) = self.mold_field_defs.get(type_name)
            && let Some(method_def) = fields
                .iter()
                .find(|field| field.is_method && field.name == method)
                .and_then(|field| field.method_def.as_ref())
        {
            let params = method_def
                .params
                .iter()
                .map(|param| {
                    param
                        .type_annotation
                        .as_ref()
                        .map(|ty| self.registry.resolve_type(ty))
                        .unwrap_or(Type::Unknown)
                })
                .collect::<Vec<_>>();
            return Some((params.len(), params.len(), params));
        }

        self.registry.get_type_fields(type_name).and_then(|fields| {
            fields
                .iter()
                .find(|(name, _)| name == method)
                .and_then(|(_, ty)| match ty {
                    Type::Function(params, _) => {
                        // `Unit => :T` marks a zero-argument signature.
                        let effective = if params.len() == 1 && params[0] == Type::Unit {
                            vec![]
                        } else {
                            params.clone()
                        };
                        Some((effective.len(), effective.len(), effective))
                    }
                    _ => None,
                })
        })
    }

    fn named_method_return_type(&self, type_name: &str, method: &str) -> Option<Type> {
        if let Some(fields) = self.mold_field_defs.get(type_name)
            && let Some(method_def) = fields
                .iter()
                .find(|field| field.is_method && field.name == method)
                .and_then(|field| field.method_def.as_ref())
        {
            return Some(
                method_def
                    .return_type
                    .as_ref()
                    .map(|ty| self.registry.resolve_type(ty))
                    .unwrap_or(Type::Unknown),
            );
        }

        self.registry.get_type_fields(type_name).and_then(|fields| {
            fields
                .iter()
                .find(|(name, _)| name == method)
                .and_then(|(_, ty)| match ty {
                    Type::Function(_, ret) => Some((**ret).clone()),
                    _ => None,
                })
        })
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
        // Statically enumerable receivers read their argless return type
        // straight from the spec table (the SSOT the universe cross-test
        // pins to this function). A builtin receiver whose method the
        // table omits returns Unknown — except Error, which falls back to
        // user-defined members. Receivers the table does not model at all
        // (Json/Molten/Stream/user Named/other) drop to the tail below.
        if let Some(recv) = builtin_recv_of(obj_type) {
            if let Some(spec) = builtin_method_spec(recv, method) {
                return render_return_kind(spec.ret, obj_type);
            }
            if let Type::Error(error_name) = obj_type {
                return self
                    .named_method_return_type(error_name, method)
                    .unwrap_or(Type::Unknown);
            }
            return Type::Unknown;
        }
        match obj_type {
            // Opaque primitives expose no methods — not even toString.
            Type::Json | Type::Molten => Type::Unknown,
            // Stream is a return-path-only receiver (no arity entry).
            Type::Generic(name, _) if name == "Stream" => match method {
                "length" => Type::Int,
                "isEmpty" => Type::Bool,
                "toString" => Type::Str,
                _ => Type::Unknown,
            },
            // User-defined Named types: declared members, else the
            // universal toString helper.
            Type::Named(type_name) => {
                if let Some(ret) = self.named_method_return_type(type_name, method) {
                    ret
                } else if method == "toString" {
                    Type::Str
                } else {
                    Type::Unknown
                }
            }
            // Everything else gets the universal toString helper only.
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

#[cfg(test)]
mod arg_types_tests {
    use super::*;

    // REFACTOR-FB-002: pin the element-type dependence of
    // `builtin_method_arg_types`. The arity table (`BUILTIN_METHOD_SPECS`) is
    // argless, so the *content* of the argument types — which element type
    // flows into which position, that key↔value and success↔error are not
    // swapped, and the function-argument signatures — is only verified here.
    // This lifts the off-table-receiver equivalence off visual inspection
    // (/so #9 recommended condition 3).
    fn arg_types(obj: Type, method: &str) -> Vec<Type> {
        TypeChecker::builtin_method_arg_types(&obj, method)
    }

    #[test]
    fn list_element_type_flows_into_arg_positions() {
        let li = Type::List(Box::new(Type::Int));
        assert_eq!(arg_types(li.clone(), "contains"), vec![Type::Int]);
        assert_eq!(arg_types(li.clone(), "indexOf"), vec![Type::Int]);
        assert_eq!(arg_types(li.clone(), "lastIndexOfLax"), vec![Type::Int]);
        assert_eq!(
            arg_types(li.clone(), "any"),
            vec![Type::Function(vec![Type::Int], Box::new(Type::Bool))]
        );
        assert_eq!(arg_types(li, "reduce"), vec![Type::Any, Type::Any]);
    }

    #[test]
    fn hashmap_key_and_value_keep_their_positions() {
        let m = Type::Generic("HashMap".to_string(), vec![Type::Str, Type::Int]);
        // key-typed lookups
        assert_eq!(arg_types(m.clone(), "get"), vec![Type::Str]);
        assert_eq!(arg_types(m.clone(), "has"), vec![Type::Str]);
        assert_eq!(arg_types(m.clone(), "remove"), vec![Type::Str]);
        // set takes (key, value) — the order must not swap
        assert_eq!(arg_types(m.clone(), "set"), vec![Type::Str, Type::Int]);
        // merge takes another HashMap[key, value]
        assert_eq!(
            arg_types(m, "merge"),
            vec![Type::Generic(
                "HashMap".to_string(),
                vec![Type::Str, Type::Int]
            )]
        );
    }

    #[test]
    fn set_method_slots_stay_element_lenient() {
        // cross-element-type membership/mutation is pinned
        // runtime semantics (enum ordinal == Int; Int in
        // Set[Float]; value-tag dedup Bool into Set[Int]), so every Set
        // method slot passes Any — matching the bare Named("Set") arm.
        let s = Type::Generic("Set".to_string(), vec![Type::Int]);
        for method in ["add", "remove", "has", "union", "intersect", "diff"] {
            assert_eq!(arg_types(s.clone(), method), vec![Type::Any]);
        }
    }

    #[test]
    fn result_success_and_error_types_are_not_swapped() {
        let r = Type::Generic("Result".to_string(), vec![Type::Int, Type::Str]);
        // map operates on the success type (first type arg)
        assert_eq!(
            arg_types(r.clone(), "map"),
            vec![Type::Function(vec![Type::Int], Box::new(Type::Any))]
        );
        // mapError operates on the error type (second type arg)
        assert_eq!(
            arg_types(r.clone(), "mapError"),
            vec![Type::Function(vec![Type::Str], Box::new(Type::Any))]
        );
        // getOrDefault carries the success type
        assert_eq!(arg_types(r, "getOrDefault"), vec![Type::Int]);
    }

    #[test]
    fn lax_and_async_inner_type_flows() {
        let lax = Type::Generic("Lax".to_string(), vec![Type::Str]);
        assert_eq!(arg_types(lax.clone(), "getOrDefault"), vec![Type::Str]);
        assert_eq!(
            arg_types(lax, "map"),
            vec![Type::Function(vec![Type::Str], Box::new(Type::Any))]
        );
        let a = Type::Generic("Async".to_string(), vec![Type::Int]);
        assert_eq!(
            arg_types(a.clone(), "map"),
            vec![Type::Function(vec![Type::Int], Box::new(Type::Any))]
        );
        assert_eq!(arg_types(a, "getOrDefault"), vec![Type::Int]);
    }

    #[test]
    fn str_arg_types_and_nullary_receivers_are_pinned() {
        assert_eq!(arg_types(Type::Str, "contains"), vec![Type::Str]);
        assert_eq!(arg_types(Type::Str, "get"), vec![Type::Int]);
        assert_eq!(arg_types(Type::Str, "replace"), vec![Type::Any, Type::Str]);
        assert_eq!(
            arg_types(Type::Str, "match"),
            vec![Type::Named("Regex".to_string())]
        );
        // nullary builtin receivers (Num / Bool state checks, toString) carry
        // no argument types.
        assert_eq!(arg_types(Type::Int, "toString"), Vec::<Type>::new());
        assert_eq!(arg_types(Type::Bool, "toString"), Vec::<Type>::new());
        assert_eq!(arg_types(Type::Int, "isNaN"), Vec::<Type>::new());
    }
    #[test]
    fn special_receivers_resolve_only_declared_signatures() {
        let mut checker = TypeChecker::new();
        for receiver in [
            Type::Json,
            Type::Molten,
            Type::Generic("Stream".into(), vec![Type::Int]),
        ] {
            assert_eq!(
                checker.builtin_method_signature(&receiver, "toString"),
                None
            );
            assert_eq!(checker.builtin_method_signature(&receiver, "unknown"), None);
        }
        let fields = vec![(
            "convert".into(),
            Type::Function(vec![Type::Str], Box::new(Type::Int)),
        )];
        checker
            .registry
            .type_defs
            .insert("Widget".into(), fields.clone());
        checker
            .registry
            .error_types
            .insert("CustomError".into(), fields.clone());
        checker
            .registry
            .type_defs
            .insert("CustomError".into(), fields);
        for receiver in [
            Type::Named("Widget".into()),
            Type::Error("CustomError".into()),
        ] {
            assert_eq!(
                checker.builtin_method_signature(&receiver, "convert"),
                Some((1, 1, vec![Type::Str]))
            );
            assert_eq!(checker.builtin_method_signature(&receiver, "unknown"), None);
        }
    }
}
