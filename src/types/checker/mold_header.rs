//! mold_header — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::parser::*;
use crate::types::Type;
use std::collections::{HashMap, HashSet};

use super::{MoldHeaderSpec, TypeChecker};

impl TypeChecker {
    pub(super) fn binding_diag(code: &str, message: String, hint: &str) -> String {
        format!("[{}] {} Hint: {}", code, message, hint)
    }

    pub(super) fn type_expr_mentions_type_param(ty: &TypeExpr, name: &str) -> bool {
        match ty {
            TypeExpr::Named(type_name) => type_name == name,
            TypeExpr::BuchiPack(fields) => fields.iter().any(|field| {
                field
                    .type_annotation
                    .as_ref()
                    .is_some_and(|field_ty| Self::type_expr_mentions_type_param(field_ty, name))
            }),
            TypeExpr::List(inner) => Self::type_expr_mentions_type_param(inner, name),
            TypeExpr::Generic(type_name, args) => {
                type_name == name
                    || args
                        .iter()
                        .any(|arg| Self::type_expr_mentions_type_param(arg, name))
            }
            TypeExpr::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| Self::type_expr_mentions_type_param(param, name))
                    || Self::type_expr_mentions_type_param(ret, name)
            }
        }
    }

    pub(super) fn type_param_name_is_reserved(&self, name: &str) -> bool {
        self.declared_concrete_type_names.contains(name)
            || self.registry.type_defs.contains_key(name)
            || self.registry.enum_defs.contains_key(name)
            || self.registry.mold_defs.contains_key(name)
            || !matches!(
                self.registry.resolve_type(&TypeExpr::Named(name.to_string())),
                Type::Named(ref resolved) if resolved == name
            )
    }

    pub(super) fn effective_mold_header_args(md: &ClassLikeDef) -> Vec<MoldHeaderArg> {
        // (E30 Sub-step 2.1) Mold kind の ClassLikeDef のみ呼び出される想定。
        let mold_args = md.mold_args().cloned().unwrap_or_default();
        md.name_args.as_ref().cloned().unwrap_or(mold_args)
    }

    pub(super) fn merge_field_defs(parent: &[FieldDef], child: &[FieldDef]) -> Vec<FieldDef> {
        let mut merged = parent.to_vec();
        for child_field in child {
            if let Some(existing) = merged
                .iter_mut()
                .find(|field| field.name == child_field.name)
            {
                *existing = child_field.clone();
            } else {
                merged.push(child_field.clone());
            }
        }
        merged
    }

    pub(super) fn header_arg_label(arg: &MoldHeaderArg) -> String {
        match arg {
            MoldHeaderArg::TypeParam(tp) => match &tp.constraint {
                Some(constraint) => {
                    format!("{} <= :{}", tp.name, Self::type_expr_to_string(constraint))
                }
                None => tp.name.clone(),
            },
            MoldHeaderArg::Concrete(ty) => format!(":{}", Self::type_expr_to_string(ty)),
        }
    }

    pub(super) fn collect_mold_type_param_names(args: &[MoldHeaderArg]) -> Vec<String> {
        args.iter()
            .filter_map(|arg| match arg {
                MoldHeaderArg::TypeParam(tp) => Some(tp.name.clone()),
                MoldHeaderArg::Concrete(_) => None,
            })
            .collect()
    }

    pub(super) fn inheritance_uses_headers(inh: &ClassLikeDef) -> bool {
        // (E30 Sub-step 2.1) Inheritance kind の ClassLikeDef のみ呼び出される想定。
        inh.parent_args().is_some() || inh.name_args.is_some()
    }

    pub(super) fn predeclare_header_metadata(&mut self, statements: &[Statement]) {
        // (E30 Sub-step 2.1) ClassLikeDef + kind discriminator dispatch
        self.mold_header_specs.clear();
        self.declared_header_arities.clear();

        for stmt in statements {
            if let Statement::ClassLikeDef(cl) = stmt {
                match &cl.kind {
                    ClassLikeKind::BuchiPack => {
                        self.declared_header_arities.insert(cl.name.clone(), 0);
                    }
                    ClassLikeKind::Mold { .. } => {
                        let header_args = Self::effective_mold_header_args(cl);
                        self.mold_header_specs.insert(
                            cl.name.clone(),
                            MoldHeaderSpec {
                                header_args: header_args.clone(),
                            },
                        );
                        self.declared_header_arities
                            .insert(cl.name.clone(), header_args.len());
                    }
                    ClassLikeKind::Inheritance { .. } => {}
                }
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for stmt in statements {
                let Statement::ClassLikeDef(inh) = stmt else {
                    continue;
                };
                if !inh.is_inheritance() {
                    continue;
                }
                let inh_parent = inh.parent().expect("inheritance kind has parent");
                let inh_child = &inh.name;

                let parent_header = self
                    .mold_header_specs
                    .get(inh_parent)
                    .map(|spec| spec.header_args.clone());
                let parent_arity = parent_header
                    .as_ref()
                    .map(Vec::len)
                    .or_else(|| self.declared_header_arities.get(inh_parent).copied());

                if let Some(parent_header) = parent_header {
                    let child_header = inh
                        .name_args
                        .clone()
                        .or_else(|| inh.parent_args().cloned())
                        .unwrap_or_else(|| parent_header.clone());
                    if self
                        .mold_header_specs
                        .get(inh_child)
                        .map(|spec| spec.header_args.as_slice())
                        != Some(child_header.as_slice())
                    {
                        self.mold_header_specs.insert(
                            inh_child.clone(),
                            MoldHeaderSpec {
                                header_args: child_header.clone(),
                            },
                        );
                        changed = true;
                    }

                    let child_arity = child_header.len();
                    if self.declared_header_arities.get(inh_child) != Some(&child_arity) {
                        self.declared_header_arities
                            .insert(inh_child.clone(), child_arity);
                        changed = true;
                    }
                } else if !Self::inheritance_uses_headers(inh)
                    && let Some(parent_arity) = parent_arity
                    && self.declared_header_arities.get(inh_child) != Some(&parent_arity)
                {
                    self.declared_header_arities
                        .insert(inh_child.clone(), parent_arity);
                    changed = true;
                }
            }
        }
    }

    pub(super) fn find_forbidden_default_ref(
        expr: &Expr,
        forbidden: &HashSet<String>,
    ) -> Option<String> {
        match expr {
            Expr::Ident(name, _) => {
                if forbidden.contains(name) {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::TemplateLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Gorilla(_)
            | Expr::Placeholder(_)
            | Expr::EnumVariant(_, _, _)
            | Expr::TypeLiteral(_, _, _)
            | Expr::Hole(_) => None,
            Expr::BuchiPack(fields, _) => fields
                .iter()
                .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden)),
            Expr::ListLit(items, _) => items
                .iter()
                .find_map(|item| Self::find_forbidden_default_ref(item, forbidden)),
            Expr::BinaryOp(left, _, right, _) => Self::find_forbidden_default_ref(left, forbidden)
                .or_else(|| Self::find_forbidden_default_ref(right, forbidden)),
            Expr::UnaryOp(_, inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
            Expr::FuncCall(callee, args, _) => Self::find_forbidden_default_ref(callee, forbidden)
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                }),
            Expr::MethodCall(obj, _, args, _) => Self::find_forbidden_default_ref(obj, forbidden)
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                }),
            Expr::FieldAccess(obj, _, _) => Self::find_forbidden_default_ref(obj, forbidden),
            Expr::CondBranch(arms, _) => arms.iter().find_map(|arm| {
                arm.condition
                    .as_ref()
                    .and_then(|cond| Self::find_forbidden_default_ref(cond, forbidden))
                    .or_else(|| {
                        arm.body.iter().find_map(|stmt| {
                            if let Statement::Expr(e) = stmt {
                                Self::find_forbidden_default_ref(e, forbidden)
                            } else {
                                None
                            }
                        })
                    })
            }),
            Expr::Pipeline(exprs, _) => exprs
                .iter()
                .find_map(|node| Self::find_forbidden_default_ref(node, forbidden)),
            Expr::MoldInst(_, type_args, fields, _) => type_args
                .iter()
                .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                .or_else(|| {
                    fields
                        .iter()
                        .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden))
                }),
            Expr::Unmold(inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
            Expr::Lambda(params, body, _) => {
                let mut nested_forbidden = forbidden.clone();
                for param in params {
                    nested_forbidden.remove(&param.name);
                }
                Self::find_forbidden_default_ref(body, &nested_forbidden)
            }
            Expr::TypeInst(_, fields, _) => fields
                .iter()
                .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden)),
            Expr::Throw(inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
        }
    }

    /// returns true when `name` is an active generic type parameter
    /// whose declared subtype constraint is a numeric primitive (`Num` / `Int`
    /// `Float`). Such a type variable is treated as numeric for arithmetic
    /// (`+` / `-` / `*`) and ordering operators inside the function body.
    pub(super) fn type_param_is_numeric(&self, name: &str) -> bool {
        let Some(tp) = self.lookup_active_type_param(name) else {
            return false;
        };
        matches!(
            tp.constraint.as_ref(),
            Some(TypeExpr::Named(n)) if n == "Num" || n == "Int" || n == "Float"
        )
    }

    /// if `name` is an active generic type parameter whose
    /// declared subtype constraint is a function type (e.g. `F <=:T =>:T`),
    /// return the resolved `Type::Function(...)` for that constraint.
    /// Returns `None` for non-function constraints (or unconstrained vars).
    pub(super) fn type_param_function_constraint(&self, name: &str) -> Option<Type> {
        let tp = self.lookup_active_type_param(name)?;
        let constraint = tp.constraint.as_ref()?;
        if matches!(constraint, TypeExpr::Function(_, _)) {
            Some(self.registry.resolve_type(constraint))
        } else {
            None
        }
    }

    pub(super) fn mold_header_type_compatible(&self, actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            (Type::Unknown, Type::Unknown) => true,
            (Type::Unknown, _) | (_, Type::Unknown) => false,
            (
                Type::Function(actual_params, actual_ret),
                Type::Function(expected_params, expected_ret),
            ) => {
                actual_params.len() == expected_params.len()
                    && actual_params.iter().zip(expected_params.iter()).all(
                        |(actual_param, expected_param)| {
                            self.mold_header_type_compatible(actual_param, expected_param)
                                && self.mold_header_type_compatible(expected_param, actual_param)
                        },
                    )
                    && self.mold_header_type_compatible(actual_ret, expected_ret)
            }
            _ => self.registry.is_subtype_of(actual, expected),
        }
    }

    pub(super) fn builtin_mold_kind_matches(
        &self,
        actual: &Type,
        kind: crate::types::mold_specs::MoldArgKind,
    ) -> bool {
        use crate::types::mold_specs::MoldArgKind;

        if matches!(actual, Type::Unknown | Type::Any) {
            return true;
        }
        match kind {
            MoldArgKind::Any => true,
            MoldArgKind::Bool => actual == &Type::Bool,
            MoldArgKind::Function => matches!(actual, Type::Function(_, _)),
            MoldArgKind::Int => actual == &Type::Int,
            MoldArgKind::Str => actual == &Type::Str,
            MoldArgKind::NullaryFunction => {
                matches!(actual, Type::Function(params, _) if params.is_empty())
            }
            MoldArgKind::UnaryFunction => {
                matches!(actual, Type::Function(params, _) if params.len() == 1)
            }
            MoldArgKind::UnaryPredicate => match actual {
                Type::Function(params, ret) if params.len() == 1 => {
                    matches!(ret.as_ref(), Type::Bool | Type::Unknown | Type::Any)
                }
                _ => false,
            },
            MoldArgKind::BinaryFunction => {
                matches!(actual, Type::Function(params, _) if params.len() == 2)
            }
            MoldArgKind::List => matches!(actual, Type::List(_)),
            MoldArgKind::ListOrStream => {
                matches!(actual, Type::List(_))
                    || matches!(actual, Type::Generic(name, _) if name == "Stream")
            }
            MoldArgKind::Numeric => actual.is_numeric(),
            // F56 Phase 4: a sealed carrier of any inner type.
            MoldArgKind::Sealed => {
                matches!(actual, Type::Generic(name, _) if name == "Secret" || name == "Moltenized")
            }
            // F56 Phase 4: a sealed carrier whose inner is byte-like (Str/Bytes).
            MoldArgKind::SealedBytes => match actual {
                Type::Generic(name, args) if name == "Secret" || name == "Moltenized" => {
                    args.first().is_none_or(|inner| {
                        matches!(inner, Type::Str | Type::Bytes | Type::Unknown | Type::Any)
                    })
                }
                _ => false,
            },
            // F56 Phase 4: a non-secret Str/Bytes (rejects a sealed argument).
            MoldArgKind::StrOrBytes => matches!(actual, Type::Str | Type::Bytes),
        }
    }

    pub(super) fn builtin_mold_kind_label(
        kind: crate::types::mold_specs::MoldArgKind,
    ) -> &'static str {
        use crate::types::mold_specs::MoldArgKind;

        match kind {
            MoldArgKind::Any => "any value",
            MoldArgKind::Bool => "Bool",
            MoldArgKind::Function => "function",
            MoldArgKind::Int => "Int",
            MoldArgKind::Str => "Str",
            MoldArgKind::NullaryFunction => "zero-argument function",
            MoldArgKind::UnaryFunction => "1-argument function",
            MoldArgKind::UnaryPredicate => "1-argument Bool predicate",
            MoldArgKind::BinaryFunction => "2-argument function",
            MoldArgKind::List => "List",
            MoldArgKind::ListOrStream => "List or Stream",
            MoldArgKind::Numeric => "numeric",
            MoldArgKind::Sealed => "a sealed Secret/Moltenized",
            MoldArgKind::SealedBytes => "a sealed Secret/Moltenized wrapping Str or Bytes",
            MoldArgKind::StrOrBytes => "Str or Bytes",
        }
    }

    pub(super) fn bind_mold_header_arg(
        &self,
        arg: &MoldHeaderArg,
        actual: &Type,
        bound_types: &mut HashMap<String, Type>,
    ) {
        if let MoldHeaderArg::TypeParam(tp) = arg {
            bound_types.insert(tp.name.clone(), actual.clone());
        }
    }

    pub(super) fn bind_generic_type_pattern(
        &self,
        pattern: &Type,
        actual: &Type,
        generic_names: &HashSet<String>,
        bindings: &mut HashMap<String, Type>,
    ) -> bool {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => {
                if actual == &Type::Unknown {
                    return true;
                }
                if let Some(bound) = bindings.get(name) {
                    self.mold_header_type_compatible(actual, bound)
                        && self.mold_header_type_compatible(bound, actual)
                } else {
                    bindings.insert(name.clone(), actual.clone());
                    true
                }
            }
            Type::List(pattern_inner) => match actual {
                Type::List(actual_inner) => self.bind_generic_type_pattern(
                    pattern_inner,
                    actual_inner,
                    generic_names,
                    bindings,
                ),
                _ => false,
            },
            Type::Generic(pattern_name, pattern_args) => match actual {
                Type::Generic(actual_name, actual_args)
                    if pattern_name == actual_name && pattern_args.len() == actual_args.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(pattern_arg, actual_arg)| {
                            self.bind_generic_type_pattern(
                                pattern_arg,
                                actual_arg,
                                generic_names,
                                bindings,
                            )
                        })
                }
                _ => false,
            },
            Type::BuchiPack(pattern_fields) => match actual {
                Type::BuchiPack(actual_fields) => {
                    pattern_fields.iter().all(|(pattern_name, pattern_ty)| {
                        actual_fields
                            .iter()
                            .find(|(actual_name, _)| actual_name == pattern_name)
                            .is_some_and(|(_, actual_ty)| {
                                self.bind_generic_type_pattern(
                                    pattern_ty,
                                    actual_ty,
                                    generic_names,
                                    bindings,
                                )
                            })
                    })
                }
                _ => false,
            },
            Type::Function(pattern_params, pattern_ret) => match actual {
                Type::Function(actual_params, actual_ret)
                    if pattern_params.len() == actual_params.len() =>
                {
                    pattern_params.iter().zip(actual_params.iter()).all(
                        |(pattern_param, actual_param)| {
                            self.bind_generic_type_pattern(
                                pattern_param,
                                actual_param,
                                generic_names,
                                bindings,
                            )
                        },
                    ) && self.bind_generic_type_pattern(
                        pattern_ret,
                        actual_ret,
                        generic_names,
                        bindings,
                    )
                }
                _ => false,
            },
            _ => self.registry.is_subtype_of(actual, pattern),
        }
    }
}
