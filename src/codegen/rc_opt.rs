/// RC 最適化パス — 不要な Retain/Release 操作を除去する。
///
/// Taida は全値イミュータブルなため、Lobster 言語の事例（95% RC 除去）
/// に匹敵する最適化が可能。
///
/// ## 最適化パターン
///
/// 1. **Return 直前の Release 除去**: 関数が値を返す直前に同じ変数を
/// Release するのは無意味（callee → caller の所有権移転）
///
/// 2. **未使用 Release 除去**: Release 対象の変数がヒープでない場合
/// （スカラー値 0 など）、Release は不要
///
/// 3. **連続 Retain-Release 除去**: 同一変数に対する Retain 直後の Release
/// は相殺されるため除去可能
use super::ir::*;

/// IrModule 全体に RC 最適化を適用
pub fn optimize(module: &mut IrModule) {
    for func in &mut module.functions {
        optimize_function(func);
    }
}

fn optimize_function(func: &mut IrFunction) {
    // Phase 1: Return 直前の同一変数 Release を除去
    remove_release_before_return(&mut func.body);

    // Phase 2: 連続 Retain-Release ペアを除去
    remove_retain_release_pairs(&mut func.body);

    // Phase 3: CondBranch 内部も再帰的に最適化
    optimize_cond_branches(&mut func.body);
}

/// Return(var) の直前にある Release(var) を除去
/// （所有権が caller に移転するため Release は不要）
#[allow(clippy::ptr_arg)]
fn remove_release_before_return(insts: &mut Vec<IrInst>) {
    let len = insts.len();
    if len < 3 {
        return;
    }

    // Return の前の Release を逆順でチェック
    // パターン: UseVar(tmp, name) → Release(tmp) → ... → Return(ret)
    // Return の直前にある Release を除去したい

    let Some(IrInst::Return(returned)) = insts.last() else {
        return;
    };

    let release_idx = len - 2;
    let use_idx = len - 3;
    if let (IrInst::Release(rel_var), IrInst::UseVar(use_var, _)) =
        (&insts[release_idx], &insts[use_idx])
        && rel_var == use_var
        && rel_var == returned
    {
        insts.remove(release_idx);
    }
}

/// 連続する Retain(v) → Release(v) ペアを除去
fn remove_retain_release_pairs(insts: &mut Vec<IrInst>) {
    let mut i = 0;
    while i + 1 < insts.len() {
        let is_pair = matches!(
            (&insts[i], &insts[i + 1]),
            (IrInst::Retain(a), IrInst::Release(b)) if a == b
        );
        if is_pair {
            insts.remove(i + 1);
            insts.remove(i);
            // インデックスを戻さない（次のペアがある可能性）
        } else {
            i += 1;
        }
    }
}

/// CondBranch 内部の命令列を再帰的に最適化
fn optimize_cond_branches(insts: &mut Vec<IrInst>) {
    for inst in insts {
        if let IrInst::CondBranch(_, arms) = inst {
            for arm in arms {
                remove_retain_release_pairs(&mut arm.body);
                optimize_cond_branches(&mut arm.body);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returning_a_scalar_keeps_heap_cleanup() {
        let mut body = vec![
            IrInst::ConstInt(0, 7),
            IrInst::UseVar(1, "scratch".into()),
            IrInst::Release(1),
            IrInst::Return(0),
        ];
        remove_release_before_return(&mut body);
        assert!(body.iter().any(|i| matches!(i, IrInst::Release(1))));
        let mut transferred = vec![
            IrInst::UseVar(1, "result".into()),
            IrInst::Release(1),
            IrInst::Return(1),
        ];
        remove_release_before_return(&mut transferred);
        assert!(!transferred.iter().any(|i| matches!(i, IrInst::Release(_))));
    }
}
