// C18-4: Enum 同士の ordering 比較 (>=, <, >) parity テスト。
// 同一 Enum のみ許可される。異 Enum / Enum ↔ Int は依然 [E1605]
// (別テスト types/checker_tests.rs::test_cross_enum_comparison_rejected
// 相当でカバー)。

Enum => HiveState = :Creating :Running :Stopped

c <= HiveState:Creating()
r <= HiveState:Running()
s <= HiveState:Stopped()

// ── Case 1: 基本の ordinal ベース比較 ─────────────────
stdout((c < r).toString())
stdout((r > c).toString())
stdout((s > r).toString())
stdout((r > s).toString())

// ── Case 2: 同値は >= が true、> は false ─────────────
stdout((c >= c).toString())
stdout((c > c).toString())

// ── Case 3: pipeline 相当の条件分岐で使う ────────────
ready x =
  | x >= HiveState:Running() |> "yes"
  | _ |> "no"
=> :Str

stdout(ready(HiveState:Creating()))
stdout(ready(HiveState:Running()))
stdout(ready(HiveState:Stopped()))
