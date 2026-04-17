// C18-3: Ordinal[Enum:Variant()]() で明示的に Enum → Int 変換する
// parity テスト。

Enum => HiveState = :Creating :Running :Stopped

// ── Case 1: 各 variant の ordinal ──────────────────────
stdout(Ordinal[HiveState:Creating()]().toString())
stdout(Ordinal[HiveState:Running()]().toString())
stdout(Ordinal[HiveState:Stopped()]().toString())

// ── Case 2: 変数経由でも同じ ordinal を返す ────────────
s <= HiveState:Running()
stdout(Ordinal[s]().toString())

// ── Case 3: Int と組み合わせた比較（#4 の正規回避経路）──
// 既存の Int カラム (legacy) との比較は Ordinal[] 経由で行う。
n <= Ordinal[HiveState:Stopped()]()
stdout((n > 0).toString())
stdout((n == 2).toString())
