//! validate — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::lexer::Span;
use crate::parser::*;
use crate::types::Type;
use std::collections::{HashMap, HashSet};

use super::{
    CageRunnerType, CompileTarget, CryptoSym, MoldBindingDef, RESERVED_INTERNAL_FIELD_PREFIX,
    TypeChecker, TypeError, default_fn_generatable,
};

impl TypeChecker {
    pub(super) fn validate_mold_root_header(
        &mut self,
        md: &ClassLikeDef,
        header_args: &[MoldHeaderArg],
    ) {
        // (E30 Sub-step 2.1) Mold kind の ClassLikeDef のみ呼び出される想定。
        // (E30 Phase 3 / Lock-B Sub-B3) `[E1407]` umbrella = 親型適用の arity mismatch
        // (header arity / prefix preservation / 親種別 / type param uniqueness 含む).
        // 本箇所は「Mold root が built-in `Mold` 親に対する適用 arity を維持しているか」
        // を確認するため、umbrella の root 部の発火点。
        let mold_args: Vec<MoldHeaderArg> = md.mold_args().cloned().unwrap_or_default();
        if mold_args.len() != 1 {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "MoldDef '{}' must keep the built-in parent `Mold` header at arity 1, got {}",
                        md.name,
                        mold_args.len()
                    ),
                    "Write `Mold[T] => Child[T, U, ...] = @(...)`; extend header slots on the child side, not on `Mold` itself.",
                ),
                span: md.span.clone(),
            });
        }

        self.validate_child_header_prefix(
            "MoldDef",
            &md.name,
            "Mold",
            &mold_args,
            header_args,
            &md.span,
        );
        self.validate_unique_mold_type_param_names("MoldDef", &md.name, header_args, &md.span);
    }

    fn validate_child_header_prefix(
        &mut self,
        kind: &str,
        child_name: &str,
        parent_name: &str,
        parent_args: &[MoldHeaderArg],
        child_args: &[MoldHeaderArg],
        span: &Span,
    ) {
        // (E30 Phase 3 / Lock-B Sub-B3) `[E1407]` umbrella = 親型適用の arity mismatch.
        // 子側 header が親 header arity 以上 + 親 header を prefix として preserve していることを確認。
        // Lock-B Sub-B3 verdict: 子側で型引数を追加するのは OK (`Result[T,P] => CustomResult[T,P,V]`)、
        // しかし shrink (`Result[T,P] => CustomResult[T]`) や prefix 改変 (`Result[T,P] => CustomResult[U,P]`)
        // は arity / header 構造 mismatch として reject。
        if child_args.len() < parent_args.len() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "{} '{}' cannot shrink header arity below parent '{}' (child: {}, parent: {})",
                        kind,
                        child_name,
                        parent_name,
                        child_args.len(),
                        parent_args.len()
                    ),
                    "Keep inherited header slots intact and append any new slots on the child side.",
                ),
                span: span.clone(),
            });
            return;
        }

        for (idx, parent_arg) in parent_args.iter().enumerate() {
            if child_args.get(idx) != Some(parent_arg) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1407",
                        format!(
                            "{} '{}' must preserve inherited header slot {} from '{}' exactly; expected {}, got {}",
                            kind,
                            child_name,
                            idx + 1,
                            parent_name,
                            Self::header_arg_label(parent_arg),
                            child_args
                                .get(idx)
                                .map(Self::header_arg_label)
                                .unwrap_or_else(|| "<missing>".to_string())
                        ),
                        "Keep inherited header slots as an exact prefix and append new slots only after the parent header.",
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_unique_mold_type_param_names(
        &mut self,
        kind: &str,
        name: &str,
        header_args: &[MoldHeaderArg],
        span: &Span,
    ) {
        // (E30 Phase 3 / Lock-B Sub-B3) `[E1407]` umbrella の周辺発火点 — header 構造一貫性の一部。
        // 同一 header に同名 type-param を二重登場させると arity / 解決の一貫性が崩れるため reject。
        let mut seen = HashSet::<String>::new();
        let mut duplicates = Vec::<String>::new();
        for arg in header_args {
            if let MoldHeaderArg::TypeParam(tp) = arg
                && !seen.insert(tp.name.clone())
                && !duplicates.contains(&tp.name)
            {
                duplicates.push(tp.name.clone());
            }
        }

        if !duplicates.is_empty() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "{} '{}' reuses header type parameter name(s): {}",
                        kind,
                        name,
                        duplicates.join(", ")
                    ),
                    "Use each header type parameter name at most once; append new child slots with distinct names.",
                ),
                span: span.clone(),
            });
        }

        // F42 sweep [E1523]: detect built-in type names mistakenly written
        // as Mold header type variables. `Mold[Int]` is silently read as
        // a type variable `Int`, masking the user's intent of a concrete
        // type argument. Surface the misuse with an actionable diagnostic.
        for arg in header_args {
            if let MoldHeaderArg::TypeParam(tp) = arg
                && Self::is_builtin_type_name(&tp.name)
            {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1523] {} '{}' header type variable '{}' collides with built-in type name. \
                         Use `{}[:{}]` for a concrete type argument or `{}[T <= :{}]` for a constrained type variable. \
                         See PHILOSOPHY.md III and docs/reference/diagnostic_codes.md [E1523].",
                        kind, name, tp.name, name, tp.name, name, tp.name
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_mold_extension_bindings(
        &mut self,
        def: MoldBindingDef<'_>,
        parent_arity: usize,
        header_args: &[MoldHeaderArg],
        fields: &[FieldDef],
        inherited_field_names: &HashSet<String>,
    ) {
        // Declare-only function fields are NOT counted as positional
        // binding targets for additional child-side header type-args.
        // They are interface members whose values are supplied at
        // instantiation time or by an automatically-generated
        // `defaultFn`. Counting them here would (a) silently consume a
        // child-side type-arg slot that the user intended to bind to a
        // regular new field, and (b) suppress the `[E1401]` "unbound
        // type parameter" diagnostic that surfaces this mistake. See
        // `FieldDef::is_declare_only_fn_field`.
        let positional_field_count = fields
            .iter()
            .filter(|f| {
                !f.is_method
                    && f.default_value.is_none()
                    && f.name != "filling"
                    && !inherited_field_names.contains(&f.name)
                    && !f.is_declare_only_fn_field()
            })
            .count();

        let extra_args = header_args.len().saturating_sub(parent_arity);
        let mut remaining_field_slots = positional_field_count;
        let mut unbound_type_params = Vec::new();
        let mut unbound_header_args = Vec::new();
        for arg in header_args.iter().skip(parent_arity) {
            if remaining_field_slots > 0 {
                remaining_field_slots -= 1;
                continue;
            }
            match arg {
                MoldHeaderArg::TypeParam(tp) => {
                    unbound_type_params.push(tp.name.clone());
                    unbound_header_args.push(tp.name.clone());
                }
                MoldHeaderArg::Concrete(ty) => {
                    unbound_header_args.push(format!(":{}", Self::type_expr_to_string(ty)));
                }
            }
        }

        if extra_args > 0 && !unbound_header_args.is_empty() {
            let (message, hint) = if unbound_type_params.len() == unbound_header_args.len() {
                (
                    format!(
                        "{} '{}' has unbound type parameter(s): {}. additional child-side header arguments must map to new non-default fields after the inherited prefix",
                        def.kind,
                        def.name,
                        unbound_type_params.join(", ")
                    ),
                    "Add new required non-default fields on the child definition so every appended type parameter has a binding target.",
                )
            } else {
                (
                    format!(
                        "{} '{}' has header argument(s) without binding target(s): {}. additional child-side header arguments must map to new non-default fields after the inherited prefix",
                        def.kind,
                        def.name,
                        unbound_header_args.join(", ")
                    ),
                    "Add new required non-default fields on the child definition so every appended header argument has a binding target.",
                )
            };
            self.errors.push(TypeError {
                message: Self::binding_diag("E1401", message, hint),
                span: def.span.clone(),
            });
        }
    }

    pub(super) fn validate_inheritance_header_arities(
        &mut self,
        inh: &ClassLikeDef,
        parent_header: Option<&[MoldHeaderArg]>,
    ) {
        // (E30 Sub-step 2.1) Inheritance kind の ClassLikeDef のみ呼び出される想定。
        // (E30 Phase 3 / Lock-B Sub-B3) `[E1407]` umbrella の Inheritance variant 発火点。
        // 親型適用の arity 一致 / mold-like parent 要件 / 子側 arity ≥ 親 を確認する。
        let inh_parent = inh.parent().expect("inheritance kind has parent");
        let inh_child = &inh.name;
        if Self::inheritance_uses_headers(inh) && parent_header.is_none() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' can only declare `Parent[...] => Child[...]` headers when parent '{}' is a mold-like type",
                        inh_child, inh_parent
                    ),
                    "Use header syntax only when inheriting from `Mold[...]` or another mold-derived child header.",
                ),
                span: inh.span.clone(),
            });
            return;
        }

        let parent_arity = parent_header.map(|args| args.len()).unwrap_or_else(|| {
            self.declared_header_arities
                .get(inh_parent)
                .copied()
                .unwrap_or(0)
        });

        if let Some(parent_args) = inh.parent_args()
            && parent_args.len() != parent_arity
        {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' must spell the parent header for '{}' with {} slot(s), got {}",
                        inh_child,
                        inh_parent,
                        parent_arity,
                        parent_args.len()
                    ),
                    "Use the parent type's formal header arity when writing `Parent[...] => Child[...]`.",
                ),
                span: inh.span.clone(),
            });
        }

        let child_arity = self.inheritance_child_arity(inh, parent_arity);
        if child_arity < parent_arity {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' cannot shrink header arity below parent '{}' (child: {}, parent: {})",
                        inh_child, inh_parent, child_arity, parent_arity
                    ),
                    "Keep inherited header slots intact and append any new slots on the child side.",
                ),
                span: inh.span.clone(),
            });
        }

        if let Some(parent_header) = parent_header {
            let parent_args_ref: Vec<MoldHeaderArg> = inh
                .parent_args()
                .cloned()
                .unwrap_or_else(|| parent_header.to_vec());
            self.validate_child_header_prefix(
                "InheritanceDef",
                inh_child,
                inh_parent,
                parent_header,
                &parent_args_ref,
                &inh.span,
            );
            let child_args_ref: Vec<MoldHeaderArg> = inh
                .name_args
                .as_ref()
                .cloned()
                .unwrap_or_else(|| parent_args_ref.clone());
            self.validate_child_header_prefix(
                "InheritanceDef",
                inh_child,
                inh_parent,
                parent_header,
                &child_args_ref,
                &inh.span,
            );
        }
    }

    pub(super) fn validate_generic_function_bindability(&mut self, fd: &FuncDef) -> bool {
        let reserved: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| self.type_param_name_is_reserved(&tp.name))
            .map(|tp| tp.name.clone())
            .collect();
        if !reserved.is_empty() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1510",
                    format!(
                        "Generic function '{}' uses reserved concrete type name(s) as type parameter(s): {}",
                        fd.name,
                        reserved.join(", ")
                    ),
                    "Rename generic type parameters so they do not shadow built-in or concrete type names.",
                ),
                span: fd.span.clone(),
            });
            return false;
        }

        let uninferable: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| {
                !fd.params.iter().any(|param| {
                    param
                        .type_annotation
                        .as_ref()
                        .is_some_and(|ty| Self::type_expr_mentions_type_param(ty, &tp.name))
                })
            })
            .map(|tp| tp.name.clone())
            .collect();
        if uninferable.is_empty() {
            return true;
        }

        self.errors.push(TypeError {
            message: Self::binding_diag(
                "E1510",
                format!(
                    "Generic function '{}' has uninferable type parameter(s): {}",
                    fd.name,
                    uninferable.join(", ")
                ),
                "In inference-only generic functions, every type parameter must appear in a parameter type annotation.",
            ),
            span: fd.span.clone(),
        });
        false
    }

    /// Per-symbol argument-shape validator for the generalized
    /// `taida-lang/crypto` surface. Mirrors `validate_crypto_sha256_call`'s
    /// `[E1506]` diagnostics but is parameterized over the symbol kind so
    /// each function (hash / hmac / encode / decode / random / equals)
    /// enforces its own arity and per-argument type rule.
    pub(super) fn validate_crypto_call(
        &mut self,
        name: &str,
        kind: CryptoSym,
        args: &[Expr],
        span: &Span,
        injected_first_arg: Option<&Type>,
    ) {
        let pipeline_injects_first_arg = injected_first_arg.is_some();
        let (arity, expected_label): (usize, &str) = match kind {
            // 1 arg: Str | Bytes
            CryptoSym::Hash | CryptoSym::Encode => (1, "Str or Bytes"),
            // 1 arg: Str
            CryptoSym::Decode => (1, "Str"),
            // 1 arg: Int
            CryptoSym::Random => (1, "Int"),
            // 2 args: Str | Bytes each
            CryptoSym::Hmac | CryptoSym::Equals => (2, "Str or Bytes"),
        };

        // Reject an arity shortfall outright: crypto builtins have a fixed
        // ABI on every backend, so a missing argument must never reach
        // lowering (the interpreter guards at runtime; native / WASM expect
        // the full argument list). A placeholder-free pipeline stage call
        // receives the piped value as an implicit first argument, so count
        // it as supplied. Hole-bearing calls are partial application and
        // are slot-checked by the [E1505] pass instead.
        let effective_args = args.len() + usize::from(pipeline_injects_first_arg);
        let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));
        if effective_args < arity && !has_hole {
            self.errors.push(TypeError {
                message: format!(
                    "[E1301] Function '{}' expects exactly {} argument(s), got {}. Hint: Pass the missing argument(s); crypto functions have a fixed arity.",
                    name, arity, effective_args
                ),
                span: span.clone(),
            });
        }
        // The excess side: a non-pipeline call with too many arguments is
        // already rejected by the general per-function arity check, but that
        // check counts only the written arguments — a pipeline stage call
        // that looks complete on paper still overflows the fixed ABI once
        // the injected pipe value is counted (the lowered call would carry
        // arity+1 values). Reject it here so no backend sees it.
        if pipeline_injects_first_arg && effective_args > arity && !has_hole {
            self.errors.push(TypeError {
                message: format!(
                    "[E1301] Function '{}' expects exactly {} argument(s), got {} (the piped value counts as the first argument). Hint: Remove the extra argument(s); crypto functions have a fixed arity.",
                    name, arity, effective_args
                ),
                span: span.clone(),
            });
        }

        // The injected pipe value *is* the first argument — apply the same
        // per-symbol type rule to it that the written arguments get below.
        // Unknown stays permissive, matching the written-argument loop.
        let type_ok = |actual_ty: &Type| match kind {
            CryptoSym::Hash | CryptoSym::Encode | CryptoSym::Hmac | CryptoSym::Equals => {
                Self::is_crypto_hash_input_type(actual_ty)
            }
            CryptoSym::Decode => matches!(actual_ty, Type::Str),
            CryptoSym::Random => matches!(actual_ty, Type::Int),
        };
        if let Some(injected_ty) = injected_first_arg
            && *injected_ty != Type::Unknown
            && !type_ok(injected_ty)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1506] Argument 1 of '{}' has type {}, expected {} (the piped value is the first argument).",
                    name, injected_ty, expected_label
                ),
                span: span.clone(),
            });
        }

        // Written arguments fill the slots after the injected one, so cap
        // the walk at the remaining arity and report 1-based positions
        // shifted by the injection.
        let written_slots = arity.saturating_sub(usize::from(pipeline_injects_first_arg));
        let position_base = 1 + usize::from(pipeline_injects_first_arg);
        for (i, arg) in args.iter().take(written_slots).enumerate() {
            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                continue;
            }
            let actual_ty = self.infer_expr_type(arg);
            if actual_ty == Type::Unknown {
                continue;
            }
            if !type_ok(&actual_ty) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1506] Argument {} of '{}' has type {}, expected {}.",
                        i + position_base,
                        name,
                        actual_ty,
                        expected_label
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_crypto_sha256_call(
        &mut self,
        name: &str,
        args: &[Expr],
        span: &Span,
        injected_first_arg: Option<&Type>,
    ) {
        let pipeline_injects_first_arg = injected_first_arg.is_some();
        // Same fixed-arity rule as `validate_crypto_call`: a missing
        // argument must never reach lowering. A placeholder-free pipeline
        // stage call (`data => sha256()`) receives the piped value as its
        // implicit first argument and is therefore complete — and for the
        // same reason a stage call with a written argument is one over.
        if args.is_empty() && !pipeline_injects_first_arg {
            self.errors.push(TypeError {
                message: format!(
                    "[E1301] Function '{}' expects exactly 1 argument(s), got 0. Hint: Pass the missing argument(s); crypto functions have a fixed arity.",
                    name
                ),
                span: span.clone(),
            });
        }
        if pipeline_injects_first_arg
            && !args.is_empty()
            && !args.iter().any(|a| matches!(a, Expr::Hole(_)))
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1301] Function '{}' expects exactly 1 argument(s), got {} (the piped value counts as the first argument). Hint: Remove the extra argument(s); crypto functions have a fixed arity.",
                    name,
                    args.len() + 1
                ),
                span: span.clone(),
            });
        }
        // The injected pipe value is the single argument — apply the same
        // Str | Bytes rule the written argument gets. Unknown stays
        // permissive, matching the written-argument loop.
        if let Some(injected_ty) = injected_first_arg
            && *injected_ty != Type::Unknown
            && !Self::is_crypto_hash_input_type(injected_ty)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1506] Argument 1 of '{}' has type {}, expected Str or Bytes \
                     (the piped value is the first argument). \
                     Hint: use `Bytes[...]()` and unmold the Lax value with `>=>` before hashing raw bytes.",
                    name, injected_ty
                ),
                span: span.clone(),
            });
        }
        // With the injection in place the single slot is taken; without it
        // the written argument fills slot 1.
        let written_slots = usize::from(!pipeline_injects_first_arg);
        for (i, arg) in args.iter().take(written_slots).enumerate() {
            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                continue;
            }
            let actual_ty = self.infer_expr_type(arg);
            if actual_ty == Type::Unknown {
                continue;
            }
            if !Self::is_crypto_hash_input_type(&actual_ty) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1506] Argument {} of '{}' has type {}, expected Str or Bytes. \
                         Hint: use `Bytes[...]()` and unmold the Lax value with `>=>` before hashing raw bytes.",
                        i + 1,
                        name,
                        actual_ty
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_host_call_descriptor(&mut self, type_args: &[Expr], span: &Span) {
        if let Some(steps) = type_args.first() {
            let steps_ty = self.infer_expr_type(steps);
            let steps_ok = matches!(&steps_ty, Type::List(inner) if Self::is_host_step_type(inner));
            if !steps_ok {
                self.errors.push(TypeError {
                    message: format!(
                        "[E3602] HostCall steps must be a list containing only HostStep[...] values, got {}. \
                         Hint: build steps with `@[HostStep[method, args](), ...]`.",
                        steps_ty
                    ),
                    span: steps.span().clone(),
                });
            }
        }

        if let Some(out) = type_args.get(1) {
            let out_ty = self.type_arg_expr_to_type(out);
            if !self.is_wire_encodable_type(&out_ty) {
                self.push_wired_constraint_error("HostCall output", &out_ty, span);
            }
        }
    }

    pub(super) fn validate_host_capability_manifest(&mut self, type_args: &[Expr], span: &Span) {
        let Some(manifest) = &self.host_capability_manifest else {
            return;
        };
        let Some(name_arg) = type_args.first() else {
            return;
        };
        let Some(kind_arg) = type_args.get(1) else {
            return;
        };
        let name = self.string_const_expr(name_arg);
        let kind = self.string_const_expr(kind_arg);
        let (Some(name), Some(kind)) = (name, kind) else {
            self.errors.push(TypeError {
                message: "[E3603] HostCapability[name, kind] requires compile-time Str values when a host capability manifest is active. \
                         Hint: use a string literal for the capability name and a Str constant for the kind."
                    .to_string(),
                span: span.clone(),
            });
            return;
        };
        if !manifest.contains(&(name.clone(), kind.clone())) {
            self.errors.push(TypeError {
                message: format!(
                    "[E3603] HostCapability[\"{}\", \"{}\"] is not declared in the active host capability manifest. \
                     Hint: declare the capability in the selected build target manifest or use a declared binding.",
                    name, kind
                ),
                span: span.clone(),
            });
        }
    }

    pub(super) fn validate_cage_runner_expr(
        &mut self,
        runner: &Expr,
        span: &Span,
    ) -> Option<CageRunnerType> {
        match runner {
            Expr::Lambda(_, _, lambda_span) => {
                self.push_cage_error(
                    "[E1514]",
                    lambda_span,
                    "[E1514] Cage runner must be a CageRilla descriptor, not a direct lambda. \
                     Hint: use a branch descriptor such as `JSCall[path, args, Out]()`."
                        .to_string(),
                );
                None
            }
            Expr::MoldInst(name, type_args, _, runner_span) => {
                if let Some((expected, signature)) = Self::js_rilla_constructor_signature(name)
                    && type_args.len() != expected
                {
                    self.push_cage_error(
                        "[E1517]",
                        runner_span,
                        format!(
                            "[E1517] {} requires {} `[]` type argument(s): `{}`. \
                             Hint: pass the descriptor directly as `Cage[subject, {}]()`.",
                            name, expected, signature, signature
                        ),
                    );
                    return None;
                }
                if Self::is_cage_rilla_child(name) && type_args.len() != 1 {
                    self.push_cage_error(
                        "[E1516]",
                        runner_span,
                        format!(
                            "[E1516] {} takes exactly one `[]` output type argument. \
                             Hint: write `{}[Out]()`; the branch is implied by the child family.",
                            name, name
                        ),
                    );
                    return None;
                }
                if name == "JSON" || name == "JSONRilla" {
                    self.push_cage_error(
                        "[E1518]",
                        runner_span,
                        "[E1518] JSON/Hammer schema casting is not a Cage runner. \
                         Hint: use `JSON[raw, Schema]()` directly and handle its `Lax[T]` result."
                            .to_string(),
                    );
                    return None;
                }
                if name == "HostCall" {
                    self.validate_host_call_descriptor(type_args, runner_span);
                }
                let info = self.cage_runner_type(runner);
                if info.is_none() {
                    self.push_cage_error(
                        "[E1517]",
                        runner_span,
                        format!(
                            "[E1517] Cage runner branch is unresolved for `{}`. \
                             Hint: pass a CageRilla child descriptor such as `JSCall[path, args, Out]()`.",
                            name
                        ),
                    );
                }

                // F42 sweep [E1520] Cage runner Out 検査: `Out = :@()` /
                // `:Unit` / `:Void` (再帰形) を Cage runner の出力型として
                // 書くことを禁止する。docs/api/js.md は「Out に Unit/@()/Void
                // 不可」を明文化しているが、これまで type checker は enforce
                // していなかった。
                if let Some(ref runner_info) = info
                    && matches!(name.as_str(), "JSCall" | "JSNew" | "JSCallAsync")
                    && Self::is_async_type(&runner_info.output)
                {
                    self.push_cage_error(
                        "[E1519]",
                        runner_span,
                        format!(
                            "[E1519] Cage runner `{}` declares Async output. \
                             JS Promise boundaries must declare the resolved non-Async Out type, \
                             not `{}[..., Async[Out]]()`.",
                            name, name
                        ),
                    );
                }

                if let Some(ref runner_info) = info
                    && Self::contains_unit_like_type(&runner_info.output)
                {
                    self.push_cage_error(
                        "[E1520]",
                        runner_span,
                        format!(
                            "[E1520] Cage runner `{}` declares output type {} ('value-absence' type, possibly nested). \
                             Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms like `:Async[Unit]`) as the \
                             Cage descriptor's Out type. Use a meaningful concrete type instead (e.g., `:Int` for byte counts, \
                             `:Bool` for status, a structured BuchiPack). See PHILOSOPHY.md I, docs/reference/diagnostic_codes.md \
                             [E1520], and docs/api/js.md (Out section).",
                            name, runner_info.output
                        ),
                    );
                }
                info
            }
            Expr::Ident(name, ident_span) => {
                let ty = self.infer_expr_type(runner);
                if matches!(ty, Type::Function(_, _)) {
                    self.push_cage_error(
                        "[E1514]",
                        ident_span,
                        format!(
                            "[E1514] Cage runner '{}' is a direct function. \
                             Hint: use a CageRilla descriptor such as `JSCall[path, args, Out]()`.",
                            name
                        ),
                    );
                } else {
                    self.push_cage_error(
                        "[E1517]",
                        ident_span,
                        format!(
                            "[E1517] Cage runner '{}' does not carry a statically known branch. \
                             Hint: pass a CageRilla child descriptor directly.",
                            name
                        ),
                    );
                }
                None
            }
            _ => {
                let ty = self.infer_expr_type(runner);
                if matches!(ty, Type::Function(_, _)) {
                    self.push_cage_error(
                        "[E1514]",
                        span,
                        "[E1514] Cage runner must be a CageRilla descriptor, not a direct function. \
                         Hint: use a branch descriptor such as `JSCall[path, args, Out]()`."
                            .to_string(),
                    );
                } else {
                    self.push_cage_error(
                        "[E1517]",
                        span,
                        "[E1517] Cage runner branch is unresolved. \
                         Hint: pass a CageRilla child descriptor such as `JSCall[path, args, Out]()`."
                            .to_string(),
                    );
                }
                None
            }
        }
    }

    pub(super) fn validate_http_serve_protocol_capability(
        &mut self,
        callee_name: &str,
        args: &[Expr],
    ) {
        if !self.net_http_serve_symbols.contains(callee_name) {
            return;
        }
        if matches!(
            self.compile_target,
            CompileTarget::WasmMin | CompileTarget::WasmEdge
        ) {
            self.errors.push(TypeError {
                message: format!(
                    "[E1612] {} does not support taida-lang/net HTTP API 'httpServe'. \
                     Hint: Use the interpreter, JS, native, wasm-wasi, or wasm-full backend instead.",
                    self.compile_target.label()
                ),
                span: args
                    .first()
                    .map(|arg| arg.span().clone())
                    .unwrap_or_else(|| Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        column: 1,
            node_id: 0,
                    }),
            });
            return;
        }
        if matches!(
            self.compile_target,
            CompileTarget::WasmWasi | CompileTarget::WasmFull
        ) && self.http_serve_handler_arity(args.get(1)) == Some(2)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1612] {} supports only 1-arg response-return taida-lang/net httpServe handlers. \
                     Hint: 2-arg streaming handlers require the interpreter, JS, or native backend.",
                    self.compile_target.label()
                ),
                span: args
                    .get(1)
                    .map(|arg| arg.span().clone())
                    .unwrap_or_else(|| Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        column: 1,
            node_id: 0,
                    }),
            });
        }
        let Some(tls_expr) = args.get(5) else {
            return;
        };
        if let Expr::BuchiPack(fields, _) = tls_expr
            && let Some(protocol_field) = fields.iter().find(|field| field.name == "protocol")
        {
            match &protocol_field.value {
                Expr::EnumVariant(enum_name, _, _)
                    if self.net_http_protocol_type_names.contains(enum_name) => {}
                Expr::EnumVariant(enum_name, _, span)
                    if !self.net_http_protocol_type_names.contains(enum_name) =>
                {
                    self.errors.push(TypeError {
                        message: "[E1506] `httpServe` tls.protocol literal must be HttpProtocol. \
                             Hint: Use `HttpProtocol:H1()` / `HttpProtocol:H2()` / `HttpProtocol:H3()`."
                            .to_string(),
                        span: span.clone(),
                    });
                }
                Expr::StringLit(_, span)
                | Expr::TemplateLit(_, span)
                | Expr::IntLit(_, span)
                | Expr::FloatLit(_, span)
                | Expr::BoolLit(_, span) => {
                    self.errors.push(TypeError {
                        message: "[E1506] `httpServe` tls.protocol literal must be HttpProtocol. \
                             Hint: Use `HttpProtocol:H1()` / `HttpProtocol:H2()` / `HttpProtocol:H3()`."
                            .to_string(),
                        span: span.clone(),
                    });
                }
                other => {
                    // F42 sweep follow-up: catch the dynamic case
                    // `p <= "h2"; ... protocol <= p` where the literal
                    // check above only sees an `Ident` / function call
                    // expression. The HttpProtocol enum is the sole
                    // accepted shape (Str union was withdrawn in
                    // F42B-013), so type-check the dynamic operand and
                    // reject anything that does not resolve to the
                    // HttpProtocol enum (or `Unknown` from a generic
                    // path, which is allowed for caller flexibility).
                    let span = other.span().clone();
                    let inferred = self.infer_expr_type(other);
                    let is_http_protocol = matches!(&inferred, Type::Named(n)
                        if self.net_http_protocol_type_names.contains(n));
                    let is_permitted_unknown = matches!(inferred, Type::Unknown | Type::Molten);
                    if !is_http_protocol && !is_permitted_unknown {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1506] `httpServe` tls.protocol must be HttpProtocol, but the dynamic operand resolves to {}. \
                                 Hint: bind the value to `HttpProtocol:H1()` / `HttpProtocol:H2()` / `HttpProtocol:H3()` before passing it in.",
                                inferred
                            ),
                            span,
                        });
                    }
                }
            }
        }
        if matches!(
            self.compile_target,
            CompileTarget::WasmWasi | CompileTarget::WasmFull
        ) && let Some((span, non_empty)) = self.http_serve_tls_pack_shape(tls_expr)
            && non_empty
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1612] {} supports only plaintext HTTP/1.1 httpServe over inherited WASI file descriptors. \
                     Hint: TLS, HTTP/2, HTTP/3, WebSocket, and streaming body APIs require the interpreter, JS, or native backend.",
                    self.compile_target.label()
                ),
                span: span.clone(),
            });
        }
    }

    /// Define a variable in the current scope.
    ///
    /// ## Scope stack invariant (N-75)
    ///
    /// `scope_stack` is **always non-empty** after construction. The global scope
    /// is pushed in `TypeChecker::new()` as `vec![HashMap::new()]`, and
    /// `pop_scope()` only removes inner scopes pushed by `push_scope()`.
    /// No code path calls `pop_scope()` without a preceding `push_scope()`,
    /// so the global scope is never popped. Methods like `define_var`,
    /// `define_var_with_span`, `lookup_var`, and `all_visible_vars` all rely
    /// on this invariant via `.last_mut()` / `.iter().rev()`.
    ///
    /// If `span` is provided and the name already exists in the current scope,
    /// a compile error is reported (same-scope redefinition is forbidden).
    /// Shadowing across scopes (inner scope redefines outer) is allowed.
    /// Validate that all imported symbols are actually exported by the target module.
    pub(super) fn validate_import_symbols(&mut self, imp: &crate::parser::ImportStmt) {
        use crate::parser::Statement as S;

        // F54 (unknown-symbol unification): every core-bundled package is
        // validated against the catalog export list. Previously only net
        // and abi were checked; a typo'd symbol on os / crypto / pool / js
        // skipped the checker, lowered to Type::Unknown, and was silently
        // dropped by the native lowering — vanishing without a diagnostic.
        if let Some((org, name)) = imp.path.split_once('/')
            && org == crate::pkg::catalog::BUNDLED_ORG
            && let Some(spec) = crate::pkg::catalog::find(name)
        {
            for sym in &imp.symbols {
                if !spec.exports.contains(&sym.name.as_str()) {
                    self.errors.push(TypeError {
                        message: format!(
                            "Symbol '{}' not found in module '{}'. The module exports: {}",
                            sym.name,
                            imp.path,
                            spec.exports.join(", ")
                        ),
                        span: imp.span.clone(),
                    });
                } else if name == "abi"
                    && matches!(
                        sym.name.as_str(),
                        "HostCall" | "HostStep" | "HostCapability"
                    )
                    && sym.alias.is_some()
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1502] taida-lang/abi descriptor '{}' cannot be imported with an alias. \
                             Hint: import '{}' directly so the checker can recognize the built-in host boundary descriptor.",
                            sym.name, sym.name
                        ),
                        span: imp.span.clone(),
                    });
                }
            }
            return;
        }

        // Skip unknown taida-lang/* paths (resolver rejects them at runtime)
        // and npm packages.
        if imp.path.starts_with("npm:") || imp.path.starts_with("taida-lang/") {
            return;
        }

        // Need source_file to resolve relative imports
        let source_file = match &self.source_file {
            Some(f) => f.clone(),
            None => return,
        };

        // Resolve the import path to a .td file + optional facade exports
        let (td_path, pkg_manifest_exports): (std::path::PathBuf, Option<Vec<String>>) = if imp
            .path
            .starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let path = source_dir.join(&imp.path);
            if path.exists() {
                (path, None)
            } else {
                return; // Cannot resolve — let downstream handle
            }
        } else {
            // Package import — resolve via .taida/deps/
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let project_root = Self::find_project_root(source_dir);

            let resolution = if let Some(ref ver) = imp.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &imp.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
            };

            match resolution {
                Some(res) => {
                    match &res.submodule {
                        Some(sub) => {
                            let sub_path = res.pkg_dir.join(format!("{}.td", sub));
                            if sub_path.exists() {
                                (sub_path, None)
                            } else {
                                return; // Cannot resolve submodule — let downstream handle
                            }
                        }
                        None => {
                            // Package root import: use centralized facade validation
                            // B11B-023: Delegates to pkg::facade for DRY validation
                            if let Some(ctx) =
                                crate::pkg::facade::resolve_facade_context(&res.pkg_dir)
                            {
                                let sym_names: Vec<String> =
                                    imp.symbols.iter().map(|s| s.name.clone()).collect();
                                let violations = crate::pkg::facade::validate_facade(
                                    &ctx.facade_exports,
                                    &ctx.entry_path,
                                    &sym_names,
                                );
                                for v in &violations {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1701] {}",
                                            crate::pkg::facade::format_facade_violation(v)
                                        ),
                                        span: imp.span.clone(),
                                    });
                                }
                                if !violations.is_empty() {
                                    return;
                                }
                                (ctx.entry_path, Some(ctx.facade_exports))
                            } else {
                                // No facade — resolve entry module normally
                                let entry_name =
                                    match crate::pkg::manifest::Manifest::from_dir(&res.pkg_dir) {
                                        Ok(Some(manifest)) => manifest.entry,
                                        _ => "main.td".to_string(),
                                    };
                                let entry_path =
                                    if let Some(stripped) = entry_name.strip_prefix("./") {
                                        res.pkg_dir.join(stripped)
                                    } else {
                                        res.pkg_dir.join(&entry_name)
                                    };
                                if entry_path.exists() {
                                    (entry_path, None)
                                } else {
                                    return;
                                }
                            }
                        }
                    }
                }
                None => return, // Package not installed — let downstream handle
            }
        };

        // Parse the target module
        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _) = crate::parser::parse(&source);

        // Collect explicit export list from entry module's <<< statements
        let mut exports = std::collections::HashSet::new();
        let mut has_export = false;
        for stmt in &program.statements {
            if let S::Export(export_stmt) = stmt {
                has_export = true;
                for sym in &export_stmt.symbols {
                    exports.insert(sym.clone());
                }
            }
        }

        // B11B-023: Facade validation (membership + ghost) is now handled by
        // pkg::facade::validate_facade() above. If we reach here with a facade,
        // it means all symbols passed validation — proceed to normal export check.
        if pkg_manifest_exports.is_some() {
            return;
        }

        // If no <<< found, all symbols are exported (backward compat)
        if !has_export {
            return;
        }

        // Validate each imported symbol against entry module's <<< export list
        for sym in &imp.symbols {
            if !exports.contains(&sym.name) {
                let export_list = if exports.is_empty() {
                    "(nothing)".to_string()
                } else {
                    let mut sorted: Vec<&String> = exports.iter().collect();
                    sorted.sort();
                    sorted
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.errors.push(TypeError {
                    message: format!(
                        "[E1701] Symbol '{}' not found in module '{}'. \
                         The module exports: {}",
                        sym.name, imp.path, export_list
                    ),
                    span: imp.span.clone(),
                });
            }
        }
    }

    /// Validate class-like definition fields (TypeDef / MoldDef / InheritanceDef).
    /// Non-method fields must have either a type annotation (`field: Type`)
    /// or a default value (`field <= value`).
    pub(super) fn validate_class_like_fields(
        &mut self,
        kind: &str,
        def_name: &str,
        fields: &[FieldDef],
    ) {
        for field in fields.iter() {
            // C12B-023 bypass closure (3rd layer, 2026-04-15): reject any
            // `__`-prefixed field name in TypeDef / MoldDef / InheritanceDef
            // bodies. This is the definition-site twin of the expression-site
            // reject in `check_mold_errors_in_expr`. Without this check, a
            // user can forge nominal packs indirectly by declaring
            // `Fake = @(__type <= "Regex", ...)` and then instantiating
            // `Fake(...)`, which materialises a pack whose `__type` field
            // is literally `"Regex"` (see `codegen/lower/molds.rs` and
            // `interpreter/eval.rs` — both copy default field values into
            // the pack verbatim). Reject on the `FieldDef` itself,
            // regardless of `is_method`, so that the rule is uniform
            // across fields and methods.
            self.validate_reserved_internal_field_name(kind, def_name, field);
            if !field.is_method && field.type_annotation.is_none() && field.default_value.is_none()
            {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1400",
                        format!(
                            "{} '{}' field '{}' must declare either a type annotation (`{}: Type`) or a default value (`{} <= value`)",
                            kind, def_name, field.name, field.name, field.name
                        ),
                        "Declare fields as `name: Type` or `name <= default`; bare `name` is not allowed."
                    ),
                    span: field.span.clone(),
                });
            }

            // F42 sweep [E1520] field-type check: reject value-absence types
            // (`:@()` / `:Unit` / `:Void`) and nested forms (`:Async[Unit]` /
            // `:Function([Unit], Unit)`) as a field's type annotation.
            // ClassLike / Mold / InheritanceDef field definitions are part of
            // the Taida surface contract, so the same prohibition applies.
            if let Some(type_annotation) = &field.type_annotation {
                let field_ty = self.registry.resolve_type(type_annotation);
                if Self::contains_unit_like_type(&field_ty) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1520] {} '{}' field '{}' has type annotation {} ('value-absence' type, possibly nested). \
                             Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms like `:Async[Unit]` or \
                             `:Function([Unit], Unit)`) as field type annotations. Use a meaningful concrete type instead. \
                             See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                            kind, def_name, field.name, field_ty
                        ),
                        span: field.span.clone(),
                    });
                }
            }

            // (E30 Phase 5 / E30B-003) `[E1410]` reject path —
            // declare-only function fields whose return type cannot be
            // auto-generated by `defaultFn` (Lock-D verdict, Phase 6 land)
            // must be supplied with an explicit default at definition time.
            // The check is definition-site (not instantiation-site): once
            // the field passes here, `defaultFn` (Phase 6) materialises a
            // proper return-type default at instantiation, so no further
            // runtime mismatch can occur.
            //
            // Lock-C verdict (E30 Phase 0, 2026-04-28):
            //   - opaque / unknown alias return → reject with [E1410]
            //   - primitive / class-like / cycle / Async / List etc. → accept
            //
            // Phase 4 (E30B-002) acceptance regression guard: the four
            // existing e30b_002_*_passes fixtures all have generatable
            // return types (Str / T / Unit / T) and continue to pass.
            if field.is_declare_only_fn_field()
                && let Some(type_annotation) = &field.type_annotation
            {
                let mut visiting = std::collections::HashSet::new();
                // The current definition is not registered until after
                // field validation completes. Seed the cycle guard with the
                // definition name so self-referential defaultFn returns such
                // as `Foo = @(next: Unit => :Foo)` mirror the runtime
                // default materializer's cycle handling instead of looking
                // like an opaque type.
                visiting.insert(def_name.to_string());
                if !default_fn_generatable(type_annotation, &self.registry, &mut visiting) {
                    self.errors.push(TypeError {
                        message: Self::binding_diag(
                            "E1410",
                            format!(
                                "{} '{}' declare-only function field '{}' requires default function or explicit value: return type cannot be auto-generated by defaultFn (opaque or unknown type)",
                                kind, def_name, field.name
                            ),
                            "Either provide an explicit default (`field <= someFn`), or change the return type to one with a generatable default (primitive / registered class-like / List / Lax / Async)."
                        ),
                        span: field.span.clone(),
                    });
                }
            }
        }
    }

    /// bypass closure (3rd layer): reject `FieldDef` whose name
    /// starts with the reserved internal-field prefix (`__`). Shared by
    /// TypeDef / MoldDef / InheritanceDef. Emits `[E1617]` — the same
    /// diagnostic code used for (1) the AST-level `Expr::BuchiPack`/
    /// `Expr::TypeInst` literal reject in `check_mold_errors_in_expr`
    /// and (2) the wasm backend Regex rejection in
    /// `emit_wasm_c::validate_regex_api_for_wasm`. The three
    /// checks form a 3-layer defence (definition / expression /
    /// backend) — any user-authored code path that tries to fabricate
    /// a nominal pack now fails at `taida check`.
    ///
    /// Rationale: `__`-prefix is the language-wide convention for
    /// compiler-internal tags (`__type`, `__value`, `__default`,
    /// `__body_stream`, etc.). These fields are materialised by
    /// runtime-side `Value::BuchiPack(...)` construction (Rust) and
    /// IR ops — never through parser-produced `FieldDef` nodes in
    /// well-formed code. The parser does not synthesise any
    /// `__`-prefixed `FieldDef` (see `parser.rs:1373/1473/1511/1524/
    /// 1571/1590` — all field names come from user source or the
    /// literal `"unmold"`). So this check can unconditionally reject
    /// `__`-prefixed `FieldDef.name` without a built-in exception
    /// escape hatch.
    fn validate_reserved_internal_field_name(
        &mut self,
        kind: &str,
        def_name: &str,
        field: &FieldDef,
    ) {
        if !field.name.starts_with(RESERVED_INTERNAL_FIELD_PREFIX) {
            return;
        }
        self.errors.push(TypeError {
            message: format!(
                "[E1617] {} '{}' declares field `{}`, whose `__`-prefix is reserved for \
                 compiler-internal use. User definitions must not declare `__`-prefixed \
                 fields: they would materialise as compiler-internal tags (e.g., `__type`, \
                 `__value`) on the runtime pack and fabricate fake nominal-type identity \
                 without the invariants that official constructors guarantee. \
                 Hint: rename the field to a non-`__`-prefixed name, or use the official \
                 constructor (e.g., `Regex(pat, flags?)`, `Lax(...)`, `Async(...)`) \
                 instead of forging the pack by hand.",
                kind, def_name, field.name
            ),
            span: field.span.clone(),
        });
    }

    /// Validate custom mold instantiation binding rules for `[]` and `()`.
    pub(super) fn validate_custom_mold_inst_bindings(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[BuchiField],
        span: &Span,
    ) {
        let mold_fields = match self.mold_field_defs.get(name).cloned() {
            Some(f) => f,
            None => return,
        };

        // Declare-only function fields are excluded from the
        // required-positional `[]` set: they are interface members
        // (`fn: A => :B` form, no body, no default) whose values are filled in
        // at instantiation time via `()` overrides. They are also classified as
        // "optional" so that explicit `(transform <= ...)` overrides in `()`
        // are accepted without an "undefined option" diagnostic. A future
        // default function path should keep this classification while removing
        // the current `Value::Unit` placeholder.
        let required_fields: Vec<String> = mold_fields
            .iter()
            .filter(|f| {
                !f.is_method
                    && f.default_value.is_none()
                    && f.name != "filling"
                    && !f.is_declare_only_fn_field()
            })
            .map(|f| f.name.clone())
            .collect();
        let optional_fields: Vec<String> = mold_fields
            .iter()
            .filter(|f| !f.is_method && (f.default_value.is_some() || f.is_declare_only_fn_field()))
            .map(|f| f.name.clone())
            .collect();

        // filling + non-default fields
        let required_positional = 1 + required_fields.len();
        if type_args.len() < required_positional {
            let missing_names: Vec<String> = std::iter::once("filling".to_string())
                .chain(required_fields.iter().cloned())
                .skip(type_args.len())
                .collect();
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1402",
                    format!(
                        "MoldInst '{}' requires {} positional `[]` argument(s), got {} (missing: {})",
                        name,
                        required_positional,
                        type_args.len(),
                        missing_names.join(", ")
                    ),
                    "Provide missing required values in `[]` order: `filling`, then non-default fields."
                ),
                span: span.clone(),
            });
        }

        if type_args.len() > required_positional {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1403",
                    format!(
                        "MoldInst '{}' takes {} positional `[]` argument(s), got {}. \
defaulted fields must be provided via `()`",
                        name,
                        required_positional,
                        type_args.len()
                    ),
                    "Move optional/defaulted values from `[]` to named `()` options.",
                ),
                span: span.clone(),
            });
        }

        let required_set: std::collections::HashSet<String> = required_fields.into_iter().collect();
        let optional_set: std::collections::HashSet<String> = optional_fields.into_iter().collect();
        let mut seen = std::collections::HashSet::<String>::new();

        for field in fields {
            if !seen.insert(field.name.clone()) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1404",
                        format!("MoldInst '{}' has duplicate option '{}'", name, field.name),
                        "Specify each named option in `()` at most once.",
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            if required_set.contains(&field.name) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1405",
                        format!(
                            "MoldInst '{}' field '{}' must be passed via `[]`, not `()`",
                            name, field.name
                        ),
                        "Pass non-default fields as positional `[]` arguments in declaration order."
                    ),
                    span: field.span.clone(),
                });
            } else if !optional_set.contains(&field.name) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1406",
                        format!(
                            "MoldInst '{}' has undefined option '{}' in `()`",
                            name, field.name
                        ),
                        "Use only fields declared with defaults as `()` options.",
                    ),
                    span: field.span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_builtin_mold_spec(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[BuchiField],
        span: &Span,
    ) {
        let Some(spec) = crate::types::mold_specs::lookup_mold_spec(name) else {
            return;
        };

        let arity_ok = spec.accepts_arity(type_args.len());
        if !arity_ok {
            let message = if name == "Molten" {
                "Molten takes no type arguments: Molten[]()".to_string()
            } else {
                format!(
                    "[E1505] `{}` expects {} positional `[]` argument(s), got {}.",
                    name,
                    spec.arity_description(),
                    type_args.len()
                )
            };
            self.errors.push(TypeError {
                message,
                span: span.clone(),
            });
        }

        if arity_ok {
            // Callback-kinded arguments (unary/binary function or predicate)
            // are inferred WITH the element-type hint derived from the list
            // argument, mirroring method-position lambdas — a bare
            // `self.infer_expr_type(lambda)` here fired [E1527] on every
            // unannotated mold callback before the HOF inference arm could
            // supply the expected function type.
            let elem_hint = std::cell::OnceCell::<Type>::new();
            for (idx, arg) in type_args.iter().enumerate() {
                let Some(kind) = spec.arg_kinds.get(idx).copied() else {
                    continue;
                };
                use crate::types::mold_specs::MoldArgKind as K;
                let hint = match kind {
                    K::UnaryFunction | K::UnaryPredicate | K::BinaryFunction => {
                        let elem = elem_hint
                            .get_or_init(|| {
                                let list_ty = type_args
                                    .first()
                                    .map(|a| self.infer_expr_type(a))
                                    .unwrap_or(Type::Unknown);
                                Self::mold_list_elem_type(&list_ty)
                            })
                            .clone();
                        let ret = if matches!(kind, K::UnaryPredicate) {
                            Type::Bool
                        } else {
                            Type::Unknown
                        };
                        match kind {
                            K::BinaryFunction => {
                                // Fold-family callback: (Acc, T) => Acc. The
                                // accumulator type comes from the init arg.
                                let acc = type_args
                                    .get(1)
                                    .map(|a| self.infer_expr_type(a))
                                    .unwrap_or(Type::Unknown);
                                Some(Type::Function(vec![acc.clone(), elem], Box::new(acc)))
                            }
                            _ => Some(Type::Function(vec![elem], Box::new(ret))),
                        }
                    }
                    _ => None,
                };
                self.validate_builtin_mold_arg_kind_hinted(name, idx, arg, kind, span, hint);
            }
        }

        if spec.options.is_empty() {
            return;
        }

        let mut seen = std::collections::HashSet::<String>::new();
        for field in fields {
            if !seen.insert(field.name.clone()) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1404",
                        format!("MoldInst '{}' has duplicate option '{}'", name, field.name),
                        "Specify each named option in `()` at most once.",
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            let Some(option) = spec.options.iter().find(|option| option.name == field.name) else {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1406",
                        format!(
                            "MoldInst '{}' has undefined option '{}' in `()`",
                            name, field.name
                        ),
                        "Use only named options declared by the builtin mold registry.",
                    ),
                    span: field.span.clone(),
                });
                continue;
            };
            self.validate_builtin_mold_option_kind(name, &field.name, &field.value, option.kind);
        }
    }

    fn validate_builtin_mold_arg_kind_hinted(
        &mut self,
        mold_name: &str,
        idx: usize,
        arg: &Expr,
        kind: crate::types::mold_specs::MoldArgKind,
        span: &Span,
        hint: Option<Type>,
    ) {
        if matches!(kind, crate::types::mold_specs::MoldArgKind::Any) {
            return;
        }
        let actual = match hint {
            Some(expected) => self.infer_expr_type_with_expected(arg, &expected),
            None => self.infer_expr_type(arg),
        };
        if self.builtin_mold_kind_matches(&actual, kind) {
            return;
        }
        self.errors.push(TypeError {
            message: format!(
                "[E1506] `{}` argument {} has type {}, expected {}.",
                mold_name,
                idx + 1,
                actual,
                Self::builtin_mold_kind_label(kind)
            ),
            span: span.clone(),
        });
    }

    fn validate_builtin_mold_option_kind(
        &mut self,
        mold_name: &str,
        option_name: &str,
        value: &Expr,
        kind: crate::types::mold_specs::MoldArgKind,
    ) {
        if matches!(kind, crate::types::mold_specs::MoldArgKind::Any) {
            return;
        }
        let actual = self.infer_expr_type(value);
        if self.builtin_mold_kind_matches(&actual, kind) {
            return;
        }
        self.errors.push(TypeError {
            message: format!(
                "[E1506] `{}` option '{}' has type {}, expected {}.",
                mold_name,
                option_name,
                actual,
                Self::builtin_mold_kind_label(kind)
            ),
            span: value.span().clone(),
        });
    }

    pub(super) fn validate_mold_header_constraints(
        &mut self,
        name: &str,
        type_args: &[Expr],
        span: &Span,
    ) {
        let Some(spec) = self.mold_header_specs.get(name).cloned() else {
            return;
        };

        let mut bound_types = HashMap::<String, Type>::new();
        for (idx, actual_expr) in type_args.iter().enumerate() {
            let actual = self.infer_expr_type(actual_expr);
            let Some(header_arg) = spec.header_args.get(idx) else {
                continue;
            };
            self.validate_single_mold_header_arg(
                name,
                idx,
                &actual,
                header_arg,
                &bound_types,
                span,
            );
            self.bind_mold_header_arg(header_arg, &actual, &mut bound_types);
        }
    }

    fn validate_single_mold_header_arg(
        &mut self,
        name: &str,
        idx: usize,
        actual: &Type,
        header_arg: &MoldHeaderArg,
        bound_types: &HashMap<String, Type>,
        span: &Span,
    ) {
        match header_arg {
            MoldHeaderArg::TypeParam(tp) => {
                if let Some(constraint) = &tp.constraint {
                    let expected = self.resolve_mold_header_type(constraint, bound_types);
                    if Self::is_wired_constraint_type(&expected) {
                        if !self.is_wire_encodable_type(actual) {
                            self.push_wired_constraint_error(
                                &format!(
                                    "MoldInst '{}' positional `[]` argument {} ('{}')",
                                    name,
                                    idx + 1,
                                    tp.name
                                ),
                                actual,
                                span,
                            );
                        }
                        return;
                    }
                    if !self.mold_header_type_compatible(actual, &expected) {
                        self.errors.push(TypeError {
                            message: Self::binding_diag(
                                "E1409",
                                format!(
                                    "MoldInst '{}' positional `[]` argument {} violates constraint on '{}': expected {}, got {}",
                                    name,
                                    idx + 1,
                                    tp.name,
                                    expected,
                                    actual
                                ),
                                "Pass a value whose inferred type satisfies the constrained mold header.",
                            ),
                            span: span.clone(),
                        });
                    }
                }
            }
            MoldHeaderArg::Concrete(concrete) => {
                let expected = self.resolve_mold_header_type(concrete, bound_types);
                if !self.mold_header_type_compatible(actual, &expected) {
                    self.errors.push(TypeError {
                        message: Self::binding_diag(
                            "E1408",
                            format!(
                                "MoldInst '{}' positional `[]` argument {} is fixed to {}, got {}",
                                name,
                                idx + 1,
                                expected,
                                actual
                            ),
                            "Pass a value whose inferred type matches the concrete mold header.",
                        ),
                        span: span.clone(),
                    });
                }
            }
        }
    }

    pub(super) fn validate_generic_function_bindings(
        &mut self,
        fd: &FuncDef,
        bindings: &HashMap<String, Type>,
        span: &Span,
    ) {
        for type_param in &fd.type_params {
            let Some(actual) = bindings.get(&type_param.name) else {
                continue;
            };
            let Some(constraint) = &type_param.constraint else {
                continue;
            };
            let expected = self.resolve_mold_header_type(constraint, bindings);
            if Self::is_wired_constraint_type(&expected) {
                if !self.is_wire_encodable_type(actual) {
                    self.push_wired_constraint_error(
                        &format!("Generic function type parameter '{}'", type_param.name),
                        actual,
                        span,
                    );
                }
                continue;
            }
            if !self.mold_header_type_compatible(actual, &expected) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1509] Generic function type parameter '{}' violates its constraint: expected {}, got {}. Hint: Pass arguments that satisfy the declared generic constraint.",
                        type_param.name, expected, actual
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    pub(super) fn validate_generic_function_inference(
        &mut self,
        fd: &FuncDef,
        bindings: &HashMap<String, Type>,
        span: &Span,
    ) -> bool {
        let missing: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| !bindings.contains_key(&tp.name))
            .map(|tp| tp.name.clone())
            .collect();
        if missing.is_empty() {
            return true;
        }

        self.errors.push(TypeError {
            message: Self::binding_diag(
                "E1510",
                format!(
                    "Generic function '{}' could not infer type parameter(s): {}",
                    fd.name,
                    missing.join(", ")
                ),
                "Pass arguments whose annotated parameter types determine every generic type parameter.",
            ),
            span: span.clone(),
        });
        false
    }

    pub(super) fn validate_function_param_defaults(&mut self, fd: &FuncDef, param_types: &[Type]) {
        let param_names: Vec<String> = fd.params.iter().map(|p| p.name.clone()).collect();

        for (i, param) in fd.params.iter().enumerate() {
            let ty = param_types.get(i).cloned().unwrap_or(Type::Unknown);

            if let Some(default_expr) = &param.default_value {
                let forbidden: HashSet<String> = param_names[i..].iter().cloned().collect();
                if let Some(illegal_ref) =
                    Self::find_forbidden_default_ref(default_expr, &forbidden)
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1302] Default value for parameter '{}' cannot reference '{}' (self or later parameter). Hint: Reference only earlier parameters in default expressions.",
                            param.name, illegal_ref
                        ),
                        span: param.span.clone(),
                    });
                }

                let default_ty = self.infer_expr_type(default_expr);
                if ty != Type::Unknown
                    && default_ty != Type::Unknown
                    && !self.registry.is_subtype_of(&default_ty, &ty)
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1303] Default value type mismatch for parameter '{}': expected {}, got {}. Hint: Make the default expression assignable to the parameter type.",
                            param.name, ty, default_ty
                        ),
                        span: param.span.clone(),
                    });
                }
            }

            self.define_var(&param.name, ty);
        }
    }

    /// Closed-constructor validation for class-like
    /// `Name(field <= value,...)` instantiations.
    ///
    /// Anonymous packs (`@(...)`) keep their open / structural shape and
    /// are intentionally left untouched by this validator. Named
    /// constructors backed by a `mold_field_defs[name]` declaration are
    /// promoted to closed form here:
    ///
    /// 1. Duplicate field names → `[E1404]` (single appearance per call).
    /// 2. Undeclared field names → `[E1406]` (the typo path that
    /// previously fell back to a default value at runtime — e.g.
    /// `Pilot(typo_age <= 14)` silently dropping the typo and giving
    /// `age = 0`).
    /// 3. Method fields (`is_method = true`) cannot be passed as
    /// constructor arguments — methods are part of the type's
    /// behaviour, not its data — `[E1407]`.
    /// 4. Declared field value type must be compatible with the field's
    /// declared type → `[E1506]` (existing arg-type code).
    /// 5. Error-derived types' `type` field is auto-set to the concrete
    /// type name. Passing `type <= "Same"` is allowed (idempotent
    /// legacy aid); any other literal / non-literal value is rejected
    /// via `[E1408]` so `type` cannot be spoofed.
    /// 6. Omitted fields are NOT rejected — the value is filled by the
    /// declared default / by the `defaultFn` synthesised in
    /// for declare-only function fields. This honours the "every type
    /// has a default" PHILOSOPHY without forcing every constructor
    /// call site to enumerate every field.
    pub(super) fn validate_type_inst_constructor(
        &mut self,
        name: &str,
        fields: &[BuchiField],
        _span: &Span,
    ) {
        let Some(field_defs) = self.mold_field_defs.get(name).cloned() else {
            // Name is not a registered class-like / mold-like type
            // declaration (e.g. an Enum variant call, a stale name, a
            // user-defined function call, etc.). Defer to other paths
            // for those — this validator is scoped strictly to the
            // closed-constructor surface for known types.
            return;
        };

        let is_error_type = self.registry.is_error_type(name);
        // Build lookup tables once. Method names are pulled from
        // `mold_field_defs` (which carries `is_method`), data names
        // additionally include inherited fields from
        // `registry.type_defs` (which contains parent-merged fields,
        // including built-in Error parent fields like `type` /
        // `message`). Without this fallback, `MyError(message <= ...)`
        // would be rejected as undefined because the AST-level
        // `mold_field_defs` only carries the *declared* extras.
        let inherited_field_types: std::collections::HashMap<String, Type> = self
            .registry
            .get_type_fields(name)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let declared_data: std::collections::HashMap<&str, &FieldDef> = field_defs
            .iter()
            .filter(|f| !f.is_method)
            .map(|f| (f.name.as_str(), f))
            .collect();
        let declared_methods: std::collections::HashSet<&str> = field_defs
            .iter()
            .filter(|f| f.is_method)
            .map(|f| f.name.as_str())
            .collect();

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for field in fields {
            // (a) duplicate detection
            if !seen.insert(field.name.clone()) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1404] Constructor '{}' has duplicate field '{}'. \
                         Hint: pass each field at most once in a `Name(...)` constructor call.",
                        name, field.name
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            // `__`-prefix is already handled by `check_mold_errors_in_expr`
            // (`[E1617]`); skip here so we don't double-report.
            if field.name.starts_with(RESERVED_INTERNAL_FIELD_PREFIX) {
                continue;
            }

            // (b) method field cannot be passed
            if declared_methods.contains(field.name.as_str()) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1407] Constructor '{}' cannot accept method field '{}' as a value. \
                         Hint: methods are part of the type's behaviour and are defined in the \
                         type declaration, not assigned per-instance.",
                        name, field.name
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            // (c) undeclared field
            let declared_opt = declared_data.get(field.name.as_str()).copied();
            let inherited_ty = inherited_field_types.get(field.name.as_str()).cloned();

            // Error-derived types: `type` is the auto-set inheritance tag.
            // The base `Error` parent merges `type: Str` into the field map,
            // so `inherited_field_types` always contains it for Error
            // subclasses. Without this hoisted check the validator below
            // would happily accept `MyError(type <= someVar)` (variable
            // bypass) or `MyError(type <= "Other")` because the field is
            // not "undeclared". The validator must always require a string
            // literal whose value matches the type name.
            if is_error_type && field.name == "type" {
                if let Expr::StringLit(value, _) = &field.value
                    && value == name
                {
                    // idempotent legacy literal — allowed
                } else {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1408] Error constructor '{}' auto-sets the `type` field. \
                             The `type` argument must be a string literal whose value exactly matches the type name (\"{}\"); \
                             variables, expressions, and any other string value are rejected. \
                             Hint: drop the `type` argument or pass the matching string literal `type <= \"{}\"`.",
                            name, name, name
                        ),
                        span: field.span.clone(),
                    });
                }
                continue;
            }

            if declared_opt.is_none() && inherited_ty.is_none() {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1406] Constructor '{}' has no field named '{}'. \
                         Hint: check the type declaration; only declared data fields can be passed \
                         as `Name(field <= value, ...)`.",
                        name, field.name
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            // (d) value type compatibility against declared / inherited type
            let expected_ty = if let Some(declared) = declared_opt {
                declared
                    .type_annotation
                    .as_ref()
                    .map(|ta| self.registry.resolve_type(ta))
                    .unwrap_or(Type::Unknown)
            } else {
                inherited_ty.unwrap_or(Type::Unknown)
            };
            if matches!(expected_ty, Type::Unknown) {
                continue;
            }
            let actual_ty = self.infer_expr_type(&field.value);
            if matches!(actual_ty, Type::Unknown) {
                continue;
            }
            if Self::contains_unknown(&actual_ty) || Self::contains_unknown(&expected_ty) {
                continue;
            }
            if !self.registry.is_subtype_of(&actual_ty, &expected_ty) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1506] Constructor '{}' field '{}' has type {}, expected {}. \
                         Hint: pass a value of the declared field type, or use an explicit conversion mold.",
                        name, field.name, actual_ty, expected_ty
                    ),
                    span: field.span.clone(),
                });
            }
        }
    }
}
