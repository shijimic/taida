//! C27B-018 Option B (@c.27 Round 3 wf018B): codegen lifetime tracking pass.
//!
//! ## 背景
//!
//! Native codegen は短命 binding (`s <= Repeat["x", 512]()` 等) に対し
//! `taida_release` IR を emit せず、関数末尾の一括 Release に頼っていた。
//! しかし関数末尾の Release は以下のケースで実行されない:
//!
//!   1. **末尾再帰 (TCO)**: `iter(n - 1)` が `TailCall` に置換され entry
//!      block にジャンプするため、本体末尾の Release 列を skip する。
//!   2. **CondBranch arm 内 binding**: arm 内で生まれて arm 内で死ぬ binding
//!      は `current_heap_vars` に登録される pass がそもそも辿り着かない
//!      (current_heap_vars は top-level Assignment 経由でしか push されない)。
//!
//! その結果、`iter n = | _ |> s <= Repeat["x", 512](); iter(n - 1)` 形の
//! 1M iter native binary は peak RSS が **533 MB** まで膨らんでいた。
//!
//! ## このパスの役割
//!
//! Lowering 完了後・rc_opt 適用前に `IrFunction.body` (および全 nested
//! `CondArm.body`) を走査し、各 `DefVar(name, value_var)` について:
//!
//!   1. その body 内で `name` が以降に `UseVar` 参照されない、
//!   2. 関数の他 (outer / sibling arm / nested arm) の本体でも参照されない、
//!   3. 関数戻り値式から到達不能、
//!   4. `value_var` が以降の Retain/Release/Call/Pack/List 操作に渡されない、
//!
//! のすべてを満たす場合、`DefVar` 直後に `IrInst::ReleaseAuto(value_var)`
//! を挿入する。`ReleaseAuto` は `taida_release_any` runtime helper を呼び、
//! heap-string (hidden header) と Pack/List/Closure (magic header) を
//! runtime に判定して dispatch する。
//!
//! ## Phase 1 (このパスがカバーする範囲)
//!
//! - 関数 body 直下の linear binding
//! - CondArm body 直下の linear binding (各 arm 独立に処理)
//! - escape 解析: binding が後続のいずれかに該当すると release 抑制:
//!     - 同じ name を `UseVar` で参照する instruction が後続に存在する
//!     - 同じ value_var を引数とする `Retain` / `Release` / `ReleaseAuto`
//!       / `Call` / `CallUser` / `CallIndirect` / `PackSet` / `MakeClosure`
//!       / `Return` / `TailCall` が後続に存在する (= 値が逃げる)
//!     - DefVar より「前」に同名 binding が存在する (rebinding は危険)
//!
//! Phase 2/3 (関数戻り値・分岐合流の精密化) は将来拡張。

use super::ir::*;
use std::collections::HashSet;

/// IrModule 全体に lifetime tracking を適用し、dead binding の
/// last-use 直後に `ReleaseAuto` を挿入する。
pub fn insert_release_for_dead_bindings(module: &mut IrModule) {
    for func in &mut module.functions {
        process_function(func);
    }
}

fn process_function(func: &mut IrFunction) {
    // First, scan the function's top-level body for the function-end
    // Release pattern (`[UseVar(t, name), Release(t)]`) emitted by
    // `lower_func_def` / `lower_program::_taida_main`. Any binding name
    // appearing in this pattern is going to be Release'd at function
    // exit — we MUST NOT insert a ReleaseAuto for that name in any
    // nested arm body, or the value would be double-freed when both
    // paths are reachable. (For TCO loops the function-end Release is
    // unreachable anyway, but conservative skip avoids the edge case
    // where the base case arm bypasses the recursion.)
    let function_end_releases = collect_function_end_release_names(&func.body);

    process_body(&mut func.body, &function_end_releases);
}

/// Walk the top-level body and find names that already have a
/// function-end `[UseVar(_, name), Release(_)]` pair, so the lifetime
/// pass can avoid inserting a duplicate ReleaseAuto for the same value.
fn collect_function_end_release_names(body: &[IrInst]) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for window in body.windows(2) {
        if let (IrInst::UseVar(uv, name), IrInst::Release(rv)) = (&window[0], &window[1])
            && uv == rv
        {
            names.insert(name.clone());
        }
    }
    names
}

/// `body` を処理し、dead binding に ReleaseAuto を挿入する。
/// `function_end_releases` は、この関数の末尾 Release 列で既に Release
/// される binding 名の集合。double-release 防止のため、これに含まれる
/// 名前への ReleaseAuto 挿入はスキップする。
fn process_body(body: &mut Vec<IrInst>, function_end_releases: &std::collections::HashSet<String>) {
    // First, recurse into nested CondBranch arms — they are processed
    // independently. We do this BEFORE inserting any new release in this
    // body so that nested-body changes don't shift indices we compute
    // in the outer pass.
    for inst in body.iter_mut() {
        if let IrInst::CondBranch(_, arms) = inst {
            for arm in arms.iter_mut() {
                process_body(&mut arm.body, function_end_releases);
            }
        }
    }

    // Walk top-down and collect (insert_after_idx, value_var) pairs.
    // We insert from the END so earlier indices remain valid.
    let mut insertions: Vec<(usize, IrVar)> = Vec::new();
    // Track which names have already been seen as DefVar earlier in this
    // body — if a name is rebound later, the first binding's lifetime
    // ends at the second DefVar (which counts as a "later reference"
    // in our analysis below).
    let mut seen_defs: HashSet<String> = HashSet::new();
    for (i, inst) in body.iter().enumerate() {
        let IrInst::DefVar(name, value_var) = inst else {
            continue;
        };
        // Skip names that the function-end Release pass already
        // handles. Inserting a ReleaseAuto here would double-free when
        // both code paths are reachable.
        if function_end_releases.contains(name) {
            seen_defs.insert(name.clone());
            continue;
        }
        // Conservative: only emit for "fresh" names (no earlier DefVar
        // with same name) to avoid double-release on rebinding paths.
        if seen_defs.contains(name) {
            seen_defs.insert(name.clone());
            continue;
        }
        seen_defs.insert(name.clone());

        // Search the rest of this body for any reference to `name` or
        // to `value_var`. If none → safe to release at i+1.
        if !is_referenced_after(body, i, name, *value_var) && !name_referenced_before(body, i, name)
        {
            insertions.push((i + 1, *value_var));
        }
    }

    // Apply insertions in reverse order so earlier indices stay valid.
    for (idx, var) in insertions.into_iter().rev() {
        body.insert(idx, IrInst::ReleaseAuto(var));
    }
}

/// `body[start_idx + 1 ..]` および各 nested CondArm.body 内に
/// `name` への UseVar 参照、または `value_var` 自体を引数に取る命令が
/// 存在するかを返す。
fn is_referenced_after(body: &[IrInst], start_idx: usize, name: &str, value_var: IrVar) -> bool {
    for inst in body.iter().skip(start_idx + 1) {
        if inst_references(inst, name, value_var) {
            return true;
        }
    }
    false
}

/// `body[..before_idx]` 内に `name` を DefVar / UseVar していたかを返す。
/// rebinding/早期 capture 検出に使う。
fn name_referenced_before(body: &[IrInst], before_idx: usize, name: &str) -> bool {
    for inst in body.iter().take(before_idx) {
        match inst {
            IrInst::DefVar(n, _) | IrInst::UseVar(_, n) if n == name => return true,
            IrInst::MakeClosure(_, _, captures) if captures.iter().any(|c| c == name) => {
                return true;
            }
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    if name_referenced_before(&arm.body, arm.body.len(), name) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// `inst` が `name` を読む or `value_var` を引数に取るかを返す。
/// nested CondBranch arms も再帰的にスキャンする。
fn inst_references(inst: &IrInst, name: &str, value_var: IrVar) -> bool {
    match inst {
        IrInst::DefVar(_, src) => *src == value_var,
        IrInst::UseVar(_, n) => n == name,
        IrInst::MakeClosure(_, _, captures) => captures.iter().any(|c| c == name),
        IrInst::Retain(v) | IrInst::Release(v) | IrInst::ReleaseAuto(v) | IrInst::Return(v) => {
            *v == value_var
        }
        IrInst::Call(_, _, args) | IrInst::CallUser(_, _, args) | IrInst::TailCall(args) => {
            args.contains(&value_var)
        }
        IrInst::CallIndirect(_, fn_var, args) => *fn_var == value_var || args.contains(&value_var),
        IrInst::PackNew(_, _) => false,
        IrInst::PackSet(p, _, v) => *p == value_var || *v == value_var,
        IrInst::PackSetTag(p, _, _) => *p == value_var,
        IrInst::PackGet(_, p, _) => *p == value_var,
        IrInst::FuncAddr(_, _) => false,
        IrInst::CondBranch(result, arms) => {
            // The CondBranch's result var carries a value that may or
            // may not be ours; arms compute it. If any arm's result is
            // value_var (= we're returning the binding through a
            // branch) or any arm body references the name/value, we
            // count this as a reference.
            if *result == value_var {
                return true;
            }
            for arm in arms {
                if let Some(c) = arm.condition
                    && c == value_var
                {
                    return true;
                }
                if arm.result == value_var {
                    return true;
                }
                for inner in &arm.body {
                    if inst_references(inner, name, value_var) {
                        return true;
                    }
                }
            }
            false
        }
        IrInst::GlobalSet(_, v) => *v == value_var,
        IrInst::GlobalGet(_, _) => false,
        IrInst::ConstInt(_, _)
        | IrInst::ConstFloat(_, _)
        | IrInst::ConstStr(_, _)
        | IrInst::ConstBool(_, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_count(body: &[IrInst]) -> usize {
        let mut n = 0;
        for inst in body {
            if matches!(inst, IrInst::ReleaseAuto(_)) {
                n += 1;
            }
            if let IrInst::CondBranch(_, arms) = inst {
                for arm in arms {
                    n += release_count(&arm.body);
                }
            }
        }
        n
    }

    /// Linear case: `s <= expr; iter(n)` — `s` is dead after DefVar.
    #[test]
    fn dead_binding_in_linear_body_gets_release() {
        let mut func = IrFunction::new("iter".to_string());
        // simulate: v0 = call(taida_str_repeat, [arg, n]); DefVar("s", v0); v1 = call(iter, [arg]); Return(v1)
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body
            .push(IrInst::CallUser(1, "iter".to_string(), vec![]));
        func.body.push(IrInst::Return(1));

        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            1,
            "dead binding should get one ReleaseAuto"
        );
        // The release should sit at index 2 (right after DefVar).
        assert!(matches!(func.body[2], IrInst::ReleaseAuto(0)));
    }

    /// Used after DefVar: should NOT release.
    #[test]
    fn used_binding_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::UseVar(1, "s".to_string()));
        func.body.push(IrInst::Return(1));
        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "used binding must not be released"
        );
    }

    /// Returned binding (escapes via Return): should NOT release.
    #[test]
    fn returned_value_var_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::Return(0));
        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "value escaping via Return must not be released"
        );
    }

    /// CondArm linear binding: `iter` recursive case.
    #[test]
    fn cond_arm_linear_binding_gets_release() {
        let mut func = IrFunction::new("iter".to_string());
        // Outer body: just CondBranch(result, [arm0, arm1])
        // arm0: ConstInt(10, 0), result=10
        // arm1: Call(20, "taida_str_repeat", []), DefVar("s", 20),
        //       Call(21, "taida_int_sub", []), TailCall([21]), result=21
        let arm0 = CondArm {
            condition: Some(99),
            body: vec![IrInst::ConstInt(10, 0)],
            result: 10,
        };
        let arm1_body = vec![
            IrInst::Call(20, "taida_str_repeat".to_string(), vec![]),
            IrInst::DefVar("s".to_string(), 20),
            IrInst::Call(21, "taida_int_sub".to_string(), vec![]),
            IrInst::TailCall(vec![21]),
        ];
        let arm1 = CondArm {
            condition: None,
            body: arm1_body,
            result: 21,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm0, arm1]));

        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            1,
            "arm-local dead binding should get a ReleaseAuto"
        );

        // verify it sits inside arm1.body, after DefVar
        if let IrInst::CondBranch(_, arms) = &func.body[0] {
            let arm1 = &arms[1];
            // arm1 body now should be:
            //   Call, DefVar, ReleaseAuto, Call, TailCall
            assert!(
                matches!(arm1.body[2], IrInst::ReleaseAuto(20)),
                "release should be inserted right after DefVar in arm body, got {:?}",
                arm1.body
            );
        } else {
            panic!("expected CondBranch");
        }
    }

    /// Captured into closure: should NOT release.
    #[test]
    fn captured_binding_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::MakeClosure(
            1,
            "lambda_0".to_string(),
            vec!["s".to_string()],
        ));
        func.body.push(IrInst::Return(1));
        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "captured binding must not be released"
        );
    }

    /// Function-end Release path already handles `name` — must NOT
    /// insert a ReleaseAuto inside an arm body for the same name (which
    /// would cause double-release on the non-TCO base-case path).
    #[test]
    fn function_end_release_prevents_double_release() {
        let mut func = IrFunction::new("foo".to_string());
        // Outer body: CondBranch(0, [arm1, arm2]); UseVar(t, "p"); Release(t); Return(0)
        // arm2 body has `p <= @(...)` (PackNew + DefVar), result=0
        let arm1 = CondArm {
            condition: Some(99),
            body: vec![IrInst::ConstInt(10, 1)],
            result: 10,
        };
        let arm2_body = vec![
            IrInst::PackNew(20, 1),
            IrInst::DefVar("p".to_string(), 20),
            IrInst::ConstInt(21, 0),
        ];
        let arm2 = CondArm {
            condition: None,
            body: arm2_body,
            result: 21,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm1, arm2]));
        // Function-end Release: UseVar(t, "p"); Release(t)
        func.body.push(IrInst::UseVar(30, "p".to_string()));
        func.body.push(IrInst::Release(30));
        func.body.push(IrInst::ConstInt(31, 0));
        func.body.push(IrInst::Return(31));

        process_function(&mut func);
        // Must be ZERO ReleaseAuto: function-end Release covers it.
        assert_eq!(
            release_count(&func.body),
            0,
            "ReleaseAuto must be skipped for names already in the function-end Release pattern"
        );
    }

    /// Stored into Pack via PackSet: should NOT release.
    #[test]
    fn pack_set_value_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::PackNew(1, 1));
        func.body.push(IrInst::PackSet(1, 0, 0));
        func.body.push(IrInst::Return(1));
        process_function(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "value stored into Pack must not be released"
        );
    }
}
