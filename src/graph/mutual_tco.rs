//! Tail-only mutual-recursion cycle merging for the C-lowering backends.
//!
//! The Interpreter / JS backends run mutual tail recursion through a runtime
//! trampoline; the native / wasm backends lower calls to plain C calls and
//! would overflow the OS stack. Instead of rejecting every mutual cycle
//! ([E0700]), tail-only cycles are merged here — at the checked-AST level,
//! before lowering — into a single self-tail-recursive dispatcher function:
//!
//! ```text
//! pingA / pingB  (tail-only cycle)
//!     ⇒  __taida_mutual_pingA_pingB(__mtag, __m0_n, __m1_n)
//!            | __mtag == 0 |>  n <= __m0_n        // rebind prologue
//!                              ...pingA body with the tail call
//!                              `pingB(x)` rewritten to
//!                              `__taida_mutual_pingA_pingB(1, __m0_n, x)`
//!            | _ |>           ...pingB body likewise...
//!     ⇒  pingA n: Int = __taida_mutual_pingA_pingB(0, n, 0)   // wrapper
//!     ⇒  pingB n: Int = __taida_mutual_pingA_pingB(1, 0, n)   // wrapper
//! ```
//!
//! The rewritten intra-cycle calls are *self* tail calls of the dispatcher,
//! so the existing self-TCO loop machinery in the emitters applies
//! unchanged. The original functions stay as thin wrappers, so external
//! callers, exports, and value references are unaffected. Each member keeps
//! its own parameter slots in the dispatcher (no positional union), which
//! avoids any cross-member type conflicts; slots not owned by the call
//! target self-forward (in-cycle calls) or receive an inert `0` pad
//! (wrappers) — an arm only ever reads its own rebound slots.
//!
//! ## Mergeable subset
//!
//! A cycle is merged only when every intra-cycle call is:
//! - a direct `FuncCall` with a plain `Ident` callee,
//! - in a *simple* tail position (function-body last statement, or the last
//!   statement of a cond-branch arm chain — NOT pipelines, error-ceiling
//!   handlers, lambdas, or argument positions),
//! - written with full arity (no reliance on default-argument completion),
//! - free of intra-cycle calls nested inside its arguments,
//!
//! and additionally no member body shadows a cycle member's name, all
//! members share the same declared return type, and no member is generic.
//! Cycles outside this subset keep the [E0700] reject (`verify.rs` uses the
//! same predicate, so the checker and the lowering can never disagree).

use crate::graph::extract::GraphExtractor;
use crate::graph::model::GraphView;
use crate::graph::query;
use crate::lexer::Span;
use crate::parser::*;

/// Return the set of mutual cycles in `program` that [`merge_program`] will
/// merge. Used by the verify layer to exempt exactly these cycles from the
/// native [E0700] reject.
pub fn mergeable_tail_cycles(program: &Program) -> Vec<Vec<String>> {
    let func_defs = collect_func_defs(program);
    if func_defs.len() < 2 {
        return Vec::new();
    }
    detect_cycles(program, &func_defs)
        .into_iter()
        .filter(|members| cycle_is_mergeable(members, &func_defs))
        .collect()
}

/// Merge every mergeable tail-only mutual cycle of `program` into a
/// dispatcher + wrappers. Programs without such cycles are returned
/// unchanged (cheap clone of the statement Vec).
pub fn merge_program(program: &Program) -> Program {
    let func_defs = collect_func_defs(program);
    if func_defs.len() < 2 {
        return program.clone();
    }
    let cycles: Vec<Vec<String>> = detect_cycles(program, &func_defs)
        .into_iter()
        .filter(|members| cycle_is_mergeable(members, &func_defs))
        .collect();
    if cycles.is_empty() {
        return program.clone();
    }

    let mut statements = program.statements.clone();
    for members in &cycles {
        merge_one_cycle(&mut statements, members);
    }
    Program { statements }
}

// ── detection ───────────────────────────────────────────────────────

fn collect_func_defs(program: &Program) -> std::collections::HashMap<String, FuncDef> {
    let mut map = std::collections::HashMap::new();
    for stmt in &program.statements {
        if let Statement::FuncDef(fd) = stmt {
            map.entry(fd.name.clone()).or_insert_with(|| fd.clone());
        }
    }
    map
}

/// Distinct user-function cycles (size ≥ 2), deduplicated by member set —
/// the same shape the verify checks use.
fn detect_cycles(
    program: &Program,
    func_defs: &std::collections::HashMap<String, FuncDef>,
) -> Vec<Vec<String>> {
    let mut extractor = GraphExtractor::new("<mutual-tco>");
    let graph = extractor.extract(program, GraphView::Call);
    let cycles = match query::find_cycles(&graph) {
        query::QueryResult::Cycles(c) => c,
        _ => return Vec::new(),
    };
    let mut sets: Vec<std::collections::BTreeSet<String>> = Vec::new();
    for cycle in &cycles {
        let distinct: std::collections::BTreeSet<String> = cycle
            .iter()
            .filter(|lbl| func_defs.contains_key(lbl.as_str()))
            .cloned()
            .collect();
        if distinct.len() < 2 {
            continue;
        }
        sets.push(distinct);
    }
    // Review C-2: `find_cycles` enumerates DFS back-edge cycles, not SCCs —
    // overlapping cycles (`a↔b` and `a↔c`) arrive as separate sets. Merging
    // them into separate dispatchers would leave the dispatchers calling
    // each other through plain wrapper calls, growing the stack per
    // iteration. Union every overlapping pair until a fixpoint so each
    // strongly-connected family becomes exactly one dispatcher.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..sets.len() {
            for j in (i + 1)..sets.len() {
                if !sets[i].is_disjoint(&sets[j]) {
                    let absorbed = sets.remove(j);
                    sets[i].extend(absorbed);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for set in sets {
        let members: Vec<String> = set.into_iter().collect();
        if seen.insert(members.join("|")) {
            out.push(members);
        }
    }
    out
}

fn cycle_is_mergeable(
    members: &[String],
    func_defs: &std::collections::HashMap<String, FuncDef>,
) -> bool {
    let member_set: std::collections::HashSet<&str> = members.iter().map(|s| s.as_str()).collect();

    // Shared declared return type, no generics, no defaulted params used as
    // pads (we always pass full arity, so defaults themselves are fine).
    let first_ret = match func_defs.get(&members[0]) {
        Some(fd) => &fd.return_type,
        None => return false,
    };
    for name in members {
        let Some(fd) = func_defs.get(name) else {
            return false;
        };
        if !fd.type_params.is_empty() {
            return false;
        }
        if &fd.return_type != first_ret {
            return false;
        }
        if shadows_any_member(&fd.body, &member_set) {
            return false;
        }
        if !member_body_is_mergeable(&fd.body, &member_set, func_defs) {
            return false;
        }
    }
    true
}

/// Walk a member body the same way the rewriter does: the last statement is
/// in simple tail position; everything else must be free of intra-cycle
/// calls.
fn member_body_is_mergeable(
    body: &[Statement],
    members: &std::collections::HashSet<&str>,
    func_defs: &std::collections::HashMap<String, FuncDef>,
) -> bool {
    let Some((last, init)) = body.split_last() else {
        return true;
    };
    for stmt in init {
        if stmt_contains_member_call(stmt, members) {
            return false;
        }
    }
    match last {
        Statement::Expr(expr) => tail_expr_is_mergeable(expr, members, func_defs),
        other => !stmt_contains_member_call(other, members),
    }
}

fn tail_expr_is_mergeable(
    expr: &Expr,
    members: &std::collections::HashSet<&str>,
    func_defs: &std::collections::HashMap<String, FuncDef>,
) -> bool {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && members.contains(name.as_str())
            {
                // The rewrite site: full arity, no nested intra-cycle calls.
                let arity = func_defs.get(name.as_str()).map(|fd| fd.params.len());
                if arity != Some(args.len()) {
                    return false;
                }
                return !args.iter().any(|a| expr_contains_member_call(a, members));
            }
            // Foreign call in tail position: its arguments must not smuggle
            // intra-cycle calls (those would be non-tail).
            !expr_contains_member_call(expr, members)
        }
        Expr::CondBranch(arms, _) => arms.iter().all(|arm| {
            arm.condition
                .as_ref()
                .map(|c| !expr_contains_member_call(c, members))
                .unwrap_or(true)
                && member_body_is_mergeable(&arm.body, members, func_defs)
        }),
        other => !expr_contains_member_call(other, members),
    }
}

/// True if the body introduces a binding (assignment target, unmold target,
/// lambda param, nested function) that shadows a cycle member's name — the
/// name-based call rewrite would misfire, so such cycles stay rejected.
fn shadows_any_member(body: &[Statement], members: &std::collections::HashSet<&str>) -> bool {
    fn stmt_shadows(stmt: &Statement, members: &std::collections::HashSet<&str>) -> bool {
        match stmt {
            Statement::Assignment(a) => {
                members.contains(a.target.as_str()) || expr_shadows(&a.value, members)
            }
            Statement::UnmoldForward(u) => {
                members.contains(u.target.as_str()) || expr_shadows(&u.source, members)
            }
            Statement::UnmoldBackward(u) => {
                members.contains(u.target.as_str()) || expr_shadows(&u.source, members)
            }
            Statement::FuncDef(fd) => {
                members.contains(fd.name.as_str())
                    || fd.params.iter().any(|p| members.contains(p.name.as_str()))
                    || fd.body.iter().any(|s| stmt_shadows(s, members))
            }
            Statement::Expr(e) => expr_shadows(e, members),
            Statement::ErrorCeiling(ec) => ec.handler_body.iter().any(|s| stmt_shadows(s, members)),
            _ => false,
        }
    }
    fn expr_shadows(expr: &Expr, members: &std::collections::HashSet<&str>) -> bool {
        match expr {
            Expr::Lambda(params, body, _) => {
                params.iter().any(|p| members.contains(p.name.as_str()))
                    || expr_shadows(body, members)
            }
            Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
                arm.condition
                    .as_ref()
                    .is_some_and(|c| expr_shadows(c, members))
                    || arm.body.iter().any(|s| stmt_shadows(s, members))
            }),
            Expr::Pipeline(steps, _) | Expr::ListLit(steps, _) => {
                steps.iter().any(|s| expr_shadows(s, members))
            }
            Expr::FuncCall(callee, args, _) => {
                expr_shadows(callee, members) || args.iter().any(|a| expr_shadows(a, members))
            }
            Expr::MethodCall(obj, _, args, _) => {
                expr_shadows(obj, members) || args.iter().any(|a| expr_shadows(a, members))
            }
            Expr::BinaryOp(l, _, r, _) => expr_shadows(l, members) || expr_shadows(r, members),
            Expr::UnaryOp(_, inner, _) | Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                expr_shadows(inner, members)
            }
            Expr::MoldInst(_, targs, fields, _) => {
                targs.iter().any(|a| expr_shadows(a, members))
                    || fields.iter().any(|f| expr_shadows(&f.value, members))
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                fields.iter().any(|f| expr_shadows(&f.value, members))
            }
            Expr::FieldAccess(obj, _, _) => expr_shadows(obj, members),
            _ => false,
        }
    }
    body.iter().any(|s| stmt_shadows(s, members))
}

fn stmt_contains_member_call(stmt: &Statement, members: &std::collections::HashSet<&str>) -> bool {
    match stmt {
        Statement::Expr(e) => expr_contains_member_call(e, members),
        Statement::Assignment(a) => expr_contains_member_call(&a.value, members),
        Statement::UnmoldForward(u) => expr_contains_member_call(&u.source, members),
        Statement::UnmoldBackward(u) => expr_contains_member_call(&u.source, members),
        Statement::FuncDef(fd) => fd
            .body
            .iter()
            .any(|s| stmt_contains_member_call(s, members)),
        Statement::ErrorCeiling(ec) => ec
            .handler_body
            .iter()
            .any(|s| stmt_contains_member_call(s, members)),
        _ => false,
    }
}

fn expr_contains_member_call(expr: &Expr, members: &std::collections::HashSet<&str>) -> bool {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && members.contains(name.as_str())
            {
                return true;
            }
            expr_contains_member_call(callee, members)
                || args.iter().any(|a| expr_contains_member_call(a, members))
        }
        // A bare reference to a member (function value) is not a call, but
        // once the member becomes a wrapper the reference still works — no
        // need to reject it here. Only calls matter for tail analysis.
        Expr::MethodCall(obj, _, args, _) => {
            expr_contains_member_call(obj, members)
                || args.iter().any(|a| expr_contains_member_call(a, members))
        }
        Expr::BinaryOp(l, _, r, _) => {
            expr_contains_member_call(l, members) || expr_contains_member_call(r, members)
        }
        Expr::UnaryOp(_, inner, _) | Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
            expr_contains_member_call(inner, members)
        }
        Expr::Lambda(_, body, _) => expr_contains_member_call(body, members),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| expr_contains_member_call(c, members))
                || arm
                    .body
                    .iter()
                    .any(|s| stmt_contains_member_call(s, members))
        }),
        Expr::Pipeline(items, _) | Expr::ListLit(items, _) => {
            items.iter().any(|i| expr_contains_member_call(i, members))
        }
        Expr::MoldInst(_, targs, fields, _) => {
            targs.iter().any(|a| expr_contains_member_call(a, members))
                || fields
                    .iter()
                    .any(|f| expr_contains_member_call(&f.value, members))
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| expr_contains_member_call(&f.value, members)),
        Expr::FieldAccess(obj, _, _) => expr_contains_member_call(obj, members),
        _ => false,
    }
}

// ── transform ───────────────────────────────────────────────────────

struct CycleLayout {
    merged_name: String,
    /// Member order = tag order (sorted member names).
    members: Vec<FuncDef>,
    /// Slot parameter names: `slot_names[i][k]` is member i's k-th slot.
    slot_names: Vec<Vec<String>>,
}

impl CycleLayout {
    fn tag_of(&self, name: &str) -> usize {
        self.members
            .iter()
            .position(|fd| fd.name == name)
            .expect("cycle member")
    }
}

fn merge_one_cycle(statements: &mut Vec<Statement>, member_names: &[String]) {
    let mut members: Vec<FuncDef> = Vec::new();
    for stmt in statements.iter() {
        if let Statement::FuncDef(fd) = stmt
            && member_names.iter().any(|m| m == &fd.name)
            && !members.iter().any(|f: &FuncDef| f.name == fd.name)
        {
            members.push(fd.clone());
        }
    }
    if members.len() != member_names.len() {
        return; // a member vanished — leave the program untouched
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));

    let merged_name = format!(
        "__taidaMutual{}",
        members
            .iter()
            .map(|fd| {
                let mut n = fd.name.clone();
                if let Some(first) = n.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                n
            })
            .collect::<Vec<_>>()
            .join("")
    );
    let slot_names: Vec<Vec<String>> = members
        .iter()
        .enumerate()
        .map(|(i, fd)| {
            fd.params
                .iter()
                .map(|p| format!("__m{}_{}", i, p.name))
                .collect()
        })
        .collect();
    let layout = CycleLayout {
        merged_name,
        members,
        slot_names,
    };

    let dummy = Span::new(0, 0, 0, 0);

    // Dispatcher params: tag + every member's slots (annotations preserved).
    let mut merged_params = vec![Param {
        name: "__mtag".to_string(),
        type_annotation: Some(TypeExpr::Named("Int".to_string())),
        default_value: None,
        span: dummy.clone(),
    }];
    for (i, fd) in layout.members.iter().enumerate() {
        for (k, p) in fd.params.iter().enumerate() {
            merged_params.push(Param {
                name: layout.slot_names[i][k].clone(),
                type_annotation: p.type_annotation.clone(),
                default_value: None,
                span: dummy.clone(),
            });
        }
    }

    // Dispatcher body: one cond arm per member (last member is the default
    // arm so every tag value is covered).
    let mut arms: Vec<CondArm> = Vec::new();
    for (i, fd) in layout.members.iter().enumerate() {
        let mut arm_body: Vec<Statement> = Vec::new();
        // Rebind prologue: the member body keeps using its original
        // parameter names, shadowing nothing (shadowing cycles were
        // rejected by `shadows_any_member`).
        for (k, p) in fd.params.iter().enumerate() {
            arm_body.push(Statement::Assignment(Assignment {
                target: p.name.clone(),
                type_annotation: p.type_annotation.clone(),
                value: Expr::Ident(layout.slot_names[i][k].clone(), dummy.clone()),
                doc_comments: Vec::new(),
                span: dummy.clone(),
                forward: false,
            }));
        }
        arm_body.extend(rewrite_body(&fd.body, &layout, &dummy));
        arms.push(CondArm {
            condition: if i + 1 == layout.members.len() {
                None
            } else {
                Some(Expr::BinaryOp(
                    Box::new(Expr::Ident("__mtag".to_string(), dummy.clone())),
                    BinOp::Eq,
                    Box::new(Expr::IntLit(i as i64, dummy.clone())),
                    dummy.clone(),
                ))
            },
            body: arm_body,
            span: dummy.clone(),
        });
    }

    let merged_def = FuncDef {
        name: layout.merged_name.clone(),
        type_params: Vec::new(),
        params: merged_params,
        body: vec![Statement::Expr(Expr::CondBranch(arms, dummy.clone()))],
        return_type: layout.members[0].return_type.clone(),
        doc_comments: Vec::new(),
        span: layout.members[0].span.clone(),
    };

    // Replace each member with a thin wrapper; the dispatcher is inserted
    // before the first member after this loop.
    for stmt in statements.iter_mut() {
        let Statement::FuncDef(fd) = stmt else {
            continue;
        };
        let Some(tag) = layout
            .members
            .iter()
            .position(|m| m.name == fd.name)
            .filter(|_| member_names.iter().any(|m| m == &fd.name))
        else {
            continue;
        };
        let mut call_args: Vec<Expr> = vec![Expr::IntLit(tag as i64, dummy.clone())];
        for (i, member) in layout.members.iter().enumerate() {
            for (k, p) in member.params.iter().enumerate() {
                if i == tag {
                    call_args.push(Expr::Ident(p.name.clone(), dummy.clone()));
                } else {
                    // Inert pad: the slot is only read by its owning arm,
                    // which a foreign-tag dispatch never executes.
                    let _ = k;
                    call_args.push(Expr::IntLit(0, dummy.clone()));
                }
            }
        }
        let wrapper_body = vec![Statement::Expr(Expr::FuncCall(
            Box::new(Expr::Ident(layout.merged_name.clone(), dummy.clone())),
            call_args,
            dummy.clone(),
        ))];
        let mut wrapper = fd.clone();
        wrapper.body = wrapper_body;
        *fd = wrapper;
    }

    // Insert the dispatcher before the first wrapper.
    if let Some(first_idx) = statements.iter().position(
        |s| matches!(s, Statement::FuncDef(fd) if layout.members.iter().any(|m| m.name == fd.name)),
    ) {
        statements.insert(first_idx, Statement::FuncDef(merged_def));
    } else {
        statements.push(Statement::FuncDef(merged_def));
    }
}

/// Rewrite intra-cycle tail calls in a member body copy. Mirrors the
/// traversal of [`member_body_is_mergeable`] exactly.
fn rewrite_body(body: &[Statement], layout: &CycleLayout, dummy: &Span) -> Vec<Statement> {
    let mut out: Vec<Statement> = body.to_vec();
    if let Some(last) = out.last_mut()
        && let Statement::Expr(expr) = last
    {
        let rewritten = rewrite_tail_expr(expr, layout, dummy);
        *last = Statement::Expr(rewritten);
    }
    out
}

fn rewrite_tail_expr(expr: &Expr, layout: &CycleLayout, dummy: &Span) -> Expr {
    match expr {
        Expr::FuncCall(callee, args, span) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && layout.members.iter().any(|m| &m.name == name)
            {
                let target_tag = layout.tag_of(name);
                let mut new_args: Vec<Expr> = vec![Expr::IntLit(target_tag as i64, dummy.clone())];
                for (i, member) in layout.members.iter().enumerate() {
                    if i == target_tag {
                        new_args.extend(args.iter().cloned());
                    } else {
                        // Self-forward foreign slots: always well-typed, and
                        // never read by the target arm.
                        for slot in &layout.slot_names[i] {
                            new_args.push(Expr::Ident(slot.clone(), dummy.clone()));
                        }
                    }
                    let _ = member;
                }
                return Expr::FuncCall(
                    Box::new(Expr::Ident(layout.merged_name.clone(), dummy.clone())),
                    new_args,
                    span.clone(),
                );
            }
            expr.clone()
        }
        Expr::CondBranch(arms, span) => Expr::CondBranch(
            arms.iter()
                .map(|arm| CondArm {
                    condition: arm.condition.clone(),
                    body: rewrite_body(&arm.body, layout, dummy),
                    span: arm.span.clone(),
                })
                .collect(),
            span.clone(),
        ),
        other => other.clone(),
    }
}
