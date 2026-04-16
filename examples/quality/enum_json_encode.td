// C18-2: jsonEncode が Enum を variant 名 Str で出力する parity テスト。
// C16 の JSON[raw, Schema]() (decode) と対称な round-trip を保証する。

Enum => HiveState = :Creating :Running :Stopped

// ── Case 1: 単一フィールド ────────────────────────────
rec1 <= @(state <= HiveState:Running())
stdout(jsonEncode(rec1))

// ── Case 2: 別 variant (境界: ordinal 0 / 最初のバリアント) ──
rec2 <= @(state <= HiveState:Creating())
stdout(jsonEncode(rec2))

// ── Case 3: 末尾 variant (境界: ordinal = len-1) ──────
rec3 <= @(state <= HiveState:Stopped())
stdout(jsonEncode(rec3))

// ── Case 4: 複数フィールド + Enum と他型の混在 ────────
Status = @(state: HiveState, count: Int)
s4 <= Status(state <= HiveState:Running(), count <= 42)
stdout(jsonEncode(s4))

// ── Case 5: Enum フィールドを含む TypeDef の List ─────
// 実アプリで一番多い形: 同じ TypeDef の要素を列にして encode する。
// TypeDef のフィールド型情報が下流に伝播するので、3 バックエンドで
// variant 名 Str 出力の整合が取れる。
// Note: count は 0 を避けて 1.. を使う。Native backend の既存制約で
// List 要素内 BuchiPack フィールドが 0 (taida_val 0) のとき、encoder
// が Unit と誤判定して `{}` を出力する (C18 scope 外の pre-existing
// issue、すべての primitive Int フィールドに影響する一般的な問題)。
states <= @[
  Status(state <= HiveState:Creating(), count <= 10),
  Status(state <= HiveState:Running(), count <= 11),
  Status(state <= HiveState:Stopped(), count <= 12)
]
stdout(jsonEncode(states))
