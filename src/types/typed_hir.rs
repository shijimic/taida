//! Typed HIR side table.
//!
//! The type-checker records the inferred `Type` of every observed
//! `Expr` here. Codegen lowering consumes the table by id so the
//! "is this expression a Bool?" decision becomes a typed lookup,
//! removing the historical method-name allow-list and reducing the
//! number of places that have to re-derive type information from
//! syntax.
//!
//! ## Design notes
//!
//! - **`ExprId` is the AST expression node id** carried in `Span::node_id`.
//! The parser assigns it in a post-parse pass. Source spans are diagnostic
//! locations only; they are not expression identity.
//! - **Side table, not an AST mirror**. The table lives next to the
//! untyped AST and is keyed by id; consumers query it explicitly.
//! - **`record(&Expr, Type)` is idempotent**: a second concrete record
//! for the same expression overwrites the first. Bare `Type::Unknown`
//! remains checker-local and is removed rather than published through
//! this table; nested residual `Unknown` remains visible so backend
//! boundaries can reject it.

use std::collections::{HashMap, HashSet};

use crate::parser::Expr;

use super::types::Type;

/// Stable identifier of an `Expr` in the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub usize);

impl ExprId {
    /// Read the AST expression node id.
    pub fn from_expr(expr: &Expr) -> Self {
        Self(expr.node_id())
    }
}

/// AST → Type side table populated by the type-checker and consumed by
/// codegen lowering for typed queries (`is_bool`, `lookup`).
#[derive(Debug, Clone, Default)]
pub struct TypedExprTable {
    types: HashMap<ExprId, Type>,
    /// Type of the value each pipeline stage passes on, keyed by the stage
    /// expression. It differs from the stage's own type when the stage is
    /// applied to the piped value (`x => half`: `half` is a function, the
    /// stage passes on its result).
    stage_results: HashMap<ExprId, Type>,
    /// Names that refer to a function the checker gives no full type: a
    /// prelude or core function used as a value (`f <= jsonEncode`). Their
    /// parameters take any value, so no `Type` can be published for them,
    /// but the backends still need to know that the value is a function.
    function_refs: HashSet<ExprId>,
    /// While positive, only fully resolved types are published (see
    /// [`TypedExprTable::begin_best_effort`]).
    best_effort_depth: usize,
}

impl TypedExprTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the inferred type of `expr`. Bare `Type::Unknown` means
    /// "not publishable yet", so it removes any earlier concrete entry
    /// for that expression. Nested residual `Unknown` is retained and
    /// caught by invariant checks.
    pub fn record(&mut self, expr: &Expr, ty: Type) {
        let id = ExprId::from_expr(expr);
        // Synthetic nodes stand for no source expression; a record for one
        // would be found by every other synthetic node.
        if id.0 >= crate::parser::SYNTHETIC_NODE_ID_BASE {
            return;
        }
        let suppress_unpublishable = matches!(&ty, Type::Unknown)
            || matches!(&ty, Type::List(inner) if matches!(inner.as_ref(), Type::Unknown))
            || (self.best_effort_depth > 0 && ty.contains_concrete_unknown());
        if suppress_unpublishable {
            self.types.remove(&id);
        } else {
            self.types.insert(id, ty);
        }
    }

    /// Record the type of the value the pipeline stage `stage` passes on.
    pub fn record_stage_result(&mut self, stage: &Expr, ty: Type) {
        let id = ExprId::from_expr(stage);
        if id.0 >= crate::parser::SYNTHETIC_NODE_ID_BASE {
            return;
        }
        if ty.contains_concrete_unknown() {
            self.stage_results.remove(&id);
        } else {
            self.stage_results.insert(id, ty);
        }
    }

    /// Type of the value the pipeline stage `stage` passes on, if recorded.
    pub fn stage_result(&self, stage: &Expr) -> Option<&Type> {
        self.stage_results.get(&ExprId::from_expr(stage))
    }

    /// Enter a best-effort section: types inferred only to inform the
    /// backends, outside the checker's own validation, are published only
    /// when fully resolved, so a partial guess never trips the residual
    /// `Unknown` invariant. Pair with [`TypedExprTable::end_best_effort`].
    pub fn begin_best_effort(&mut self) {
        self.best_effort_depth += 1;
    }

    pub fn end_best_effort(&mut self) {
        self.best_effort_depth = self.best_effort_depth.saturating_sub(1);
    }

    /// Record whether the name `expr` refers to a function without a full
    /// type (see `function_refs`).
    pub fn note_function_ref(&mut self, expr: &Expr, is_function: bool) {
        let id = ExprId::from_expr(expr);
        if id.0 >= crate::parser::SYNTHETIC_NODE_ID_BASE {
            return;
        }
        if is_function {
            self.function_refs.insert(id);
        } else {
            self.function_refs.remove(&id);
        }
    }

    /// Whether the name `expr` refers to a function without a full type.
    pub fn is_function_ref(&self, expr: &Expr) -> bool {
        self.function_refs.contains(&ExprId::from_expr(expr))
    }

    /// Lookup the type of `expr`. Returns `None` if not recorded.
    pub fn lookup(&self, expr: &Expr) -> Option<&Type> {
        let id = ExprId::from_expr(expr);
        self.types.get(&id)
    }

    /// Convenience for codegen: returns `true` iff the recorded type
    /// for `expr` is exactly `Type::Bool`.
    pub fn is_bool(&self, expr: &Expr) -> bool {
        matches!(self.lookup(expr), Some(Type::Bool))
    }

    /// Number of recorded entries. Used for diagnostics / introspection.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// True if no entries recorded.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Returns `true` if any recorded type still contains a residual
    /// `Type::Unknown`. The full-pin contract for `Lax` / `Result` /
    /// `Async` method signatures requires the table to be free of
    /// residuals after the type-checker completes; this helper drives
    /// that fixture-level assertion.
    pub fn has_residual_unknown(&self) -> bool {
        self.types.values().any(|t| t.contains_concrete_unknown())
    }

    pub fn residual_unknown_types(&self) -> Vec<&Type> {
        // In expression-id order, so the diagnostic lists them the same
        // way on every run.
        let mut residuals: Vec<(&ExprId, &Type)> = self
            .types
            .iter()
            .filter(|(_, t)| t.contains_concrete_unknown())
            .collect();
        residuals.sort_by_key(|(id, _)| id.0);
        residuals.into_iter().map(|(_, t)| t).collect()
    }

    /// Iterate over all recorded `(ExprId, Type)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&ExprId, &Type)> {
        self.types.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Span;

    fn span(start: usize, end: usize) -> Span {
        Span::new(start, end, 1, 1)
    }

    #[test]
    fn record_and_lookup_basic() {
        let mut table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2).with_node_id(1));
        assert!(table.is_empty());
        table.record(&expr, Type::Int);
        assert_eq!(table.len(), 1);
        assert_eq!(table.lookup(&expr), Some(&Type::Int));
    }

    #[test]
    fn record_idempotent_overwrites_with_last_write() {
        let mut table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2).with_node_id(1));
        table.record(&expr, Type::Unknown);
        table.record(&expr, Type::Int);
        // Last-write wins when hinted inference revisits an expression.
        assert_eq!(table.lookup(&expr), Some(&Type::Int));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn is_bool_query() {
        let mut table = TypedExprTable::new();
        let bool_expr = Expr::BoolLit(true, span(0, 4).with_node_id(1));
        let int_expr = Expr::IntLit(42, span(5, 7).with_node_id(2));
        table.record(&bool_expr, Type::Bool);
        table.record(&int_expr, Type::Int);
        assert!(table.is_bool(&bool_expr));
        assert!(!table.is_bool(&int_expr));
    }

    #[test]
    fn bare_unknown_is_not_published() {
        let mut table = TypedExprTable::new();
        let e1 = Expr::IntLit(1, span(0, 1).with_node_id(1));
        let e2 = Expr::IntLit(2, span(2, 3).with_node_id(2));
        table.record(&e1, Type::Int);
        assert!(!table.has_residual_unknown());
        table.record(&e2, Type::Unknown);
        assert!(!table.has_residual_unknown());
        assert_eq!(table.lookup(&e2), None);
        table.record(&e1, Type::Unknown);
        assert_eq!(table.lookup(&e1), None);
    }

    #[test]
    fn has_residual_unknown_detects_nested_unknown() {
        let mut table = TypedExprTable::new();
        let e = Expr::ListLit(vec![], span(0, 2).with_node_id(1));
        table.record(
            &e,
            Type::Generic("Result".to_string(), vec![Type::Int, Type::Unknown]),
        );
        assert!(table.has_residual_unknown());
    }

    #[test]
    fn node_id_disambiguates_same_span_same_variant() {
        // Two equivalent expression shapes with the same source span still
        // get distinct ids when they are distinct AST nodes.
        let mut table = TypedExprTable::new();
        let first = Expr::IntLit(0, span(0, 1).with_node_id(1));
        let second = Expr::IntLit(0, span(0, 1).with_node_id(2));
        table.record(&first, Type::Int);
        table.record(&second, Type::Float);
        assert_eq!(table.lookup(&first), Some(&Type::Int));
        assert_eq!(table.lookup(&second), Some(&Type::Float));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn lookup_returns_none_for_unrecorded() {
        let table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2).with_node_id(1));
        assert!(table.lookup(&expr).is_none());
        assert!(!table.is_bool(&expr));
    }

    #[test]
    fn iter_yields_all_recorded() {
        let mut table = TypedExprTable::new();
        let e1 = Expr::IntLit(1, span(0, 1).with_node_id(1));
        let e2 = Expr::StringLit("x".to_string(), span(2, 5).with_node_id(2));
        table.record(&e1, Type::Int);
        table.record(&e2, Type::Str);
        let entries: Vec<_> = table.iter().collect();
        assert_eq!(entries.len(), 2);
    }
}
