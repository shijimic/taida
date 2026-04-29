# 操作モールド

> **PHILOSOPHY.md -- III.** カタめたいなら、鋳型を作りましょう

> **本章は E30 (gen-E 破壊的変更) で再構成されました。** 旧「Mold 基底クラス」(`Mold[T] => MyMold[T] = @(...)`) の **型定義部分はクラスライク型定義の単一構文** に統合され、[クラスライク型定義 / モールド系統](04_class_like.md#モールド系統-操作モールド) で説明されます。本章では値を流し込んで取り出す **操作モールド** (Upper / Filter / Sort 等) を中心に扱います。旧構文からの移行は [migration_e30.md](migration_e30.md) を参照してください。

---

## 操作モールドとは

型パラメータ化が必要になったら、鋳型（Mold）を作ります。値を鋳型に流し込み（モールディング）、必要なときに取り出します（アンモールディング）。

操作はモールドで、状態チェックはメソッドで -- これが Taida の原則です。

```taida
// クラスライク型として鋳型を定義する (詳細: 04_class_like.md)
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)

// 値を流し込みます
boxed <= Result[42, _ = true]()

// 取り出します
boxed ]=> value  // 42
```

> 鋳型自体の定義 (`Mold[T] => Foo[T] = @(...)`) は [クラスライク型定義](04_class_like.md) の単一構文の一例です。本章では既に定義された鋳型を **使う** 側 (溶融・取り出し操作) に集中します。

---

## モールドの解剖

クラスライク型としての鋳型の構造を簡単に振り返ります (詳細は [04_class_like.md](04_class_like.md) を参照):

```taida
// 基本形式 (クラスライク型定義)
Mold[T] => MyMold[T] = @(
  filling: T // [T]に代入された値が格納される
  solidify _ => :V  // Is-A（何として固まるか）を決める
  unmold _ => :U    // 取り出し値（]=> / <=[ / .unmold()）を決める

  // 追加フィールドを定義（プロパティ、メソッド）
)
```

ヘッダー (`Mold[T] => MyMold[T]`) は親型 arity 一致が要件です ([04_class_like.md](04_class_like.md#親型の型引数と-arity-一致))。`Mold[...]` の親側は常に 1 slot のままで、追加 slot は子側 `MyMold[T, U]` 等で書きます。

ヘッダースロットの意味:

- 1つ目のスロットは常に `filling` に対応します
- 2つ目以降のスロットは、`@(...)` 内の「デフォルト値なしフィールド」に宣言順で対応します
- この対応は `T` だけでなく `:Int` のような具象型スロットでも同じです
- つまり、具象型スロットを途中に置いた場合も、そのスロットは1つの束縛先フィールドを消費します

```taida
Mold[:Int] => IntBox = @()
ok_box <= IntBox[1]()
// IntBox["x"]() はコンパイルエラー: 1つ目のスロットは具象型 Int

Mold[:Int] => IntPair[:Int, T] = @(
  second: T
)
pair <= IntPair[1, "x"]()

Mold[:Int] => Broken[:Int, T] = @()
// コンパイルエラー: 2つ目のヘッダースロットに対応するフィールドが無い
```

### solidify / unmold フック

`Mold` は 2 つのフックで挙動が決まります。

| フック | デフォルト | 意味 |
|---|---|---|
| `solidify` | `self` を返す | モールドが何の型として固まるか（Is-A） |
| `unmold` | `filling` を返す | 固まった値から何を取り出すか |

`Name[args]()` の評価は次の順序です。

1. `[]` を `filling` とデフォルト値なしフィールドへ順に束縛
2. `()` をデフォルト値ありフィールドへ束縛
3. `solidify` を評価して値 `V` を得る
4. `Name[args]()` の式型は `V`

演算子の意味:

- `Name[args]() => x`: `solidify` の結果を `x` に代入
- `Name[args]() ]=> x`: `solidify` 結果に対して `unmold` を実行して `x` に代入

### `filling` と引数バインド規則

`filling` は常に 1 つ目の `[]` 位置引数です。2つ目以降は `@(...)` のフィールド定義から自動的に割り当てられます。

| フィールド種別 | 入力 | 省略 |
|---|---|---|
| `filling` | 1つ目の `[]` | 不可 |
| デフォルト値なしフィールド（`filling` 以外） | 2つ目以降の `[]`（宣言順） | 不可 |
| デフォルト値ありフィールド | `()` 名前付き設定 | 可 |

規則違反はコンパイルエラーです（不足・過多・`[]`/`()` の取り違え・未定義オプション）。
また、カスタムモールド定義時に追加型引数の束縛先が無い場合もコンパイルエラーです。
さらに、通常フィールドは `field: Type` または `field <= value` のどちらかが必要です（`field` 単独は不可）。

```taida
Mold[T] => Div[T, U] = @(
  divisor: U
  solidify _ =
    | divisor == 0 |> Lax[T](hasValue <= false)
    | _ |> Lax[T](hasValue <= true)
  => :Lax[T]
)

Div[10, 3]()  // filling=10, divisor=3
Div[10]()     // コンパイルエラー: divisor が不足

Mold[T] => Broken[T, U] = @(
  solidify _ = filling
  => :T
)
// コンパイルエラー: U の束縛先が無い

Mold[:Int] => AlsoBroken[:Int, U] = @()
// コンパイルエラー: 具象型スロットもフィールドスロットを消費するため、U の束縛先が無い

Mold[T] => BrokenField[T] = @(
  count
)
// コンパイルエラー: count は型注釈かデフォルト値が必要
```

### インスタンス化

実際の値を型引数として渡すと、型が自動推論されます:

```taida
pilot <= @(name <= "Kaoru", age <= 14)
pilotBox <= Lax[pilot]()
// pilotBox: Lax[@(name: Str, age: Int)]

number <= 42
numberBox <= Lax[number]()
// numberBox: Lax[Int]
```

### 明示的な型指定

```taida
explicitBox: Lax[Str] <= Lax["test"]()
emptyLax: Lax[Int] <= Div[10, 0]()
```

---

## `[]` と `()` の役割分担

モールドは、`[]` と `()` で引数の役割を明確に分けています。

| | `[]` 位置引数 | `()` 名前付き設定 |
|---|---|---|
| **役割** | 必須引数（何を / 何で） | オプション設定（どうやって） |
| **名前** | なし（位置で区別） | あり（名前で区別） |
| **省略** | 不可 | 可（デフォルト値あり） |
| **順序** | 固定（`[何を, 何で]`） | 任意 |

```taida
// [] = 必須引数: str と old と new がなければ置換不可能
// () = オプション: all のデフォルトは false（最初の1つだけ）
Replace["hello world", "world", "taida"](all <= true)

// [] = 必須引数: list がなければソート不可能
// () = オプション: reverse と by にはデフォルトがある
Sort[@[3, 1, 2]](reverse <= true)
```

---

## 文字列モールド

文字列の変換・加工はモールドで行います。状態チェック（`length()`, `contains()`, `startsWith()` 等）はメソッドとして存在します。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `Upper[str]()` | str | - | Str | 大文字変換 |
| `Lower[str]()` | str | - | Str | 小文字変換 |
| `Trim[str]()` | str | start, end | Str | 空白除去 |
| `Split[str, delim]()` | str, delim | - | @[Str] | 分割 |
| `Replace[str, old, new]()` | str, old, new | all | Str | 置換 |
| `Slice[str]()` | str | start, end | Str | 範囲抽出 |
| `CharAt[str, idx]()` | str, idx | - | Lax[Str] | 指定位置の文字 |
| `Repeat[str, n]()` | str, n | - | Str | 繰り返し |
| `Reverse[str]()` | str | - | Str | 逆順 |
| `Pad[str, len]()` | str, len | side, char | Str | パディング |

### 使用例

```taida
Upper["hello"]()                              // "HELLO"
Lower["HELLO"]()                              // "hello"
Trim["  hello  "]()                           // "hello"
Trim["  hello  "](end <= false)               // "hello  "（先頭のみ）
Split["a,b,c", ","]()                         // @["a", "b", "c"]
Replace["hello world", "o", "0"]()            // "hell0 world"（最初の1つ）
Replace["hello world", "o", "0"](all <= true) // "hell0 w0rld"（全部）
Slice["hello"](start <= 1, end <= 3)          // "el"
CharAt["hello", 0]() ]=> ch                   // ch: "h" (Lax[Str]を返す)
Repeat["ha", 3]()                             // "hahaha"
Reverse["hello"]()                            // "olleh"
Pad["42", 5](side <= "start", char <= "0")    // "00042"
```

---

## 数値モールド

数値の変換・加工はモールドで行います。状態チェック（`isNaN()`, `isZero()`, `isPositive()` 等）はメソッドとして存在します。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `ToFixed[num, digits]()` | num, digits | - | Str | 小数点固定文字列 |
| `Abs[num]()` | num | - | Num | 絶対値 |
| `Floor[num]()` | num | - | Int | 切り捨て |
| `Ceil[num]()` | num | - | Int | 切り上げ |
| `Round[num]()` | num | - | Int | 四捨五入 |
| `Truncate[num]()` | num | - | Int | 0方向切り捨て |
| `Clamp[num, min, max]()` | num, min, max | - | Num | 範囲制限 |
| `BitAnd[a, b]()` | a, b | - | Int | ビットAND |
| `BitOr[a, b]()` | a, b | - | Int | ビットOR |
| `BitXor[a, b]()` | a, b | - | Int | ビットXOR |
| `BitNot[x]()` | x | - | Int | ビットNOT |
| `ShiftL[x, n]()` | x, n | - | Lax[Int] | 左シフト（`n` が 0..63 のとき成功） |
| `ShiftR[x, n]()` | x, n | - | Lax[Int] | 算術右シフト（`n` が 0..63 のとき成功） |
| `ShiftRU[x, n]()` | x, n | - | Lax[Int] | 論理右シフト（`n` が 0..63 のとき成功） |
| `ToRadix[int, base]()` | int, base | - | Lax[Str] | 基数変換（`base` は 2..36） |
| `Int[str, base]()` | str, base | - | Lax[Int] | 指定基数で文字列を整数化（`base` は 2..36） |

### 使用例

```taida
ToFixed[3.14159, 2]()         // "3.14"
Abs[-5]()                     // 5
Floor[3.7]()                  // 3
Ceil[3.2]()                   // 4
Round[3.5]()                  // 4
Truncate[-3.7]()              // -3
Clamp[15, 0, 10]()            // 10
```

> ビット演算用の新しい演算子は追加しません。`Bit*` / `Shift*` / `ToRadix` / `Int[str, base]` をモールドで使います。

### バイト列・Unicode 変換モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `UInt8[x]()` | x | - | Lax[Int] | `0..255` への範囲制約付き変換 |
| `Bytes[x]()` | x | `fill <= Int` | Lax[Bytes] | `Int/Str/@[Int]/Bytes` から Bytes へ変換 |
| `ByteSet[bytes, idx, value]()` | bytes, idx, value | - | Lax[Bytes] | 指定位置を更新した新しい Bytes を返す |
| `BytesToList[bytes]()` | bytes | - | @[Int] | Bytes を整数リストへ展開 |
| `Char[x]()` | x | - | Lax[Str] | `Int` または `Str` から1文字へ変換 |
| `CodePoint[str]()` | str | - | Lax[Int] | 1文字 `Str` からコードポイント取得 |
| `Utf8Encode[str]()` | str | - | Lax[Bytes] | UTF-8 バイト列へ変換 |
| `Utf8Decode[bytes]()` | bytes | - | Lax[Str] | UTF-8 バイト列を文字列へ復元（不正列は failure） |

```taida
Bytes[4](fill <= 65) ]=> b        // Bytes[@[65, 65, 65, 65]]
ByteSet[b, 1, 66]() ]=> b2        // Bytes[@[65, 66, 65, 65]]
BytesToList[b2]()                  // @[65, 66, 65, 65]

Char[65]() ]=> c                   // "A"
CodePoint["A"]() ]=> cp            // 65

Utf8Encode["pong"]() ]=> raw       // Bytes[@[112,111,110,103]]
Utf8Decode[raw]() ]=> text         // "pong"
```

---

## リストモールド

リストの操作はモールドで行います。状態チェック（`length()`, `isEmpty()`, `contains()`, `any()`, `all()`, `none()` 等）と安全アクセス（`get()`, `first()`, `last()`, `max()`, `min()`）はメソッドとして存在します。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `Reverse[list]()` | list | - | @[T] | 逆順 |
| `Concat[list, other]()` | list, other | - | @[T] | 結合 |
| `Append[list, val]()` | list, val | - | @[T] | 末尾追加 |
| `Prepend[list, val]()` | list, val | - | @[T] | 先頭追加 |
| `Join[list, sep]()` | list, sep | - | Str | 文字列結合 |
| `Sum[list]()` | list | - | Num | 合計 |
| `Sort[list]()` | list | reverse, by | @[T] | ソート |
| `Unique[list]()` | list | by | @[T] | 重複除去 |
| `Flatten[list]()` | list | - | @[U] | フラット化 |
| `Find[list, fn]()` | list, fn | - | Lax[T] | 条件検索 |
| `FindIndex[list, fn]()` | list, fn | - | Int | 条件位置検索 |
| `Count[list, fn]()` | list, fn | - | Int | 条件カウント |
| `Take[list, n]()` | list, n | - | @[T] | 先頭n個 |
| `Drop[list, n]()` | list, n | - | @[T] | 先頭n個スキップ |
| `TakeWhile[list, fn]()` | list, fn | - | @[T] | 条件満たす間取得 |
| `DropWhile[list, fn]()` | list, fn | - | @[T] | 条件満たす間スキップ |
| `Zip[list, other]()` | list, other | - | @[BuchiPack] | ペア化 |
| `Enumerate[list]()` | list | - | @[BuchiPack] | インデックス付与 |

### 高階関数モールド（HOF）

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `Filter[list, fn]()` | list, fn | - | @[T] | 条件フィルタ |
| `Map[list, fn]()` | list, fn | - | @[U] | 変換 |
| `Fold[list, init, fn]()` | list, init, fn | - | A | 左畳み込み |
| `Foldr[list, init, fn]()` | list, init, fn | - | A | 右畳み込み |

### 統合モールドの使い方

Sort と Unique はオプション設定で挙動を制御できます:

```taida
// Sort: デフォルトは昇順
Sort[@[3, 1, 2]]()                                // @[1, 2, 3]
Sort[@[3, 1, 2]](reverse <= true)                 // @[3, 2, 1]
Sort[pilots](by <= _ p = p.sync_rate)               // キー関数ソート
Sort[pilots](by <= _ p = p.name, reverse <= true)  // キー関数降順

// Unique: デフォルトは値の等価比較
Unique[@[1, 2, 2, 3, 3]]()                        // @[1, 2, 3]
Unique[items](by <= _ x = x.id)                    // idフィールドで重複判定
```

### 使用例

```taida
// パイロットデータの処理
pilots <= @[
  @(name <= "Shinji", sync_rate <= 95, active <= true),
  @(name <= "Asuka", sync_rate <= 41, active <= true),
  @(name <= "Rei", sync_rate <= 65, active <= false)
]

// フィルタ → マップ
pilots => Filter[_, _ p = p.active]() => Map[_, _ p = p.name]() => activeNames
// activeNames: @["Shinji", "Asuka"]

// 集約
Fold[orders, 0, _ total order = total + (order.quantity * order.price)]() ]=> totalRevenue

// ソートとフィルタの組み合わせ
pilots => Filter[_, _ p = p.active]() => Sort[_](by <= _ p = p.sync_rate, reverse <= true) => topPilots
```

---

## 演算モールド: Div / Mod

除算と剰余は Div / Mod モールドで行います。`/` と `%` 演算子は Taida にはありません。結果は Lax で返ります。

```taida
// 除算
Div[10, 3]() ]=> quotient    // 3
Div[10, 0]() ]=> quotient    // 0（ゼロ除算: デフォルト値）
Div[10, 0]().hasValue         // false

// 剰余
Mod[10, 3]() ]=> remainder   // 1
Mod[10, 0]() ]=> remainder   // 0（ゼロ除算: デフォルト値）
Mod[10, 0]().hasValue         // false
```

ゼロ除算はエラーではなく、`Lax(hasValue=false)` が返ります。unmold するとデフォルト値（Int なら 0、Float なら 0.0）が返ります。

---

## 条件モールド: If

`If[condition, then_value, else_value]()` は 2 分岐の条件式をモールドとして表現します。

```taida
// 基本的な条件分岐
stdout(If[x > 0, "positive", "negative"]())

// パイプラインでの使用（clamp パターン）
150 => If[_ > 100, 100, _]() => clamped   // 100
50 => If[_ > 100, 100, _]() => clamped    // 50
```

特徴:

- **短絡評価**: 選択されなかった枝は評価しません。`If[true, "ok", throw "error"]()` でエラーは発生しません
- **パイプライン対応**: `_` で前段の値を参照できます（`_ > 3` のようにネストした `_` も可能）
- **ネスト可能**: `If[cond, If[cond2, a, b](), c]()` のように入れ子にできます
- **`| |>` との使い分け**: `If` は 2 分岐向き、`| |>` は複数分岐向き

```taida
// ネスト
stdout(If[true, If[false, 1, 2](), 3]())   // 2

// パイプラインで比較
5 => If[_ > 3, "big", "small"]() => result  // "big"
```

---

## 型比較モールド: TypeIs / TypeExtends

型の比較をモールドで行います。

### TypeIs -- 実行時の型チェック

`TypeIs[value, :TypeName]()` は値の実行時の型が指定した型と一致するかを返します。

```taida
stdout(TypeIs[42, :Int]())        // true
stdout(TypeIs["hello", :Int]())   // false
stdout(TypeIs["hello", :Str]())   // true
stdout(TypeIs[true, :Bool]())     // true
stdout(TypeIs[3.14, :Float]())    // true
stdout(TypeIs[42, :Num]())        // true（Int は Num の一種）
```

Enum variant の判定もできます:

```taida
Enum => Status = :Ok :Fail :Retry
x <= Status:Ok()
stdout(TypeIs[x, Status:Ok]())    // true
stdout(TypeIs[x, Status:Fail]())  // false
```

### TypeExtends -- 型の継承関係チェック

`TypeExtends[:TypeA, :TypeB]()` は TypeA が TypeB と同じか、TypeB のサブタイプかを返します。

```taida
stdout(TypeExtends[:Int, :Num]())     // true（Int は Num のサブタイプ）
stdout(TypeExtends[:Str, :Int]())     // false（Str は Int と無関係）

Animal = @(name: Str)
Animal => Dog = @(breed: Str)
stdout(TypeExtends[:Dog, :Animal]())  // true（Dog は Animal を継承）
stdout(TypeExtends[:Animal, :Dog]())  // false（逆方向は不可）
```

対応する型リテラル（B11 スコープ）:

- プリミティブ: `:Int`, `:Float`, `:Num`, `:Bool`, `:Str`, `:Bytes`, `:Error`
- ユーザー定義型: `:TypeName`
- Enum variant: `EnumName:Variant`（TypeIs のみ）

B11 スコープ外: inline BuchiPack 型リテラル、関数型リテラル、汎用 generic literal

---

## 型変換モールド

値の型変換は型変換モールドで行います。結果は Lax で返ります。
これは `solidify` オーバーライドで表現される仕様であり、型変換モールド専用のコンパイラ特別扱いは不要です。

| モールド | 説明 | 成功例 | 失敗例 |
|---------|------|--------|--------|
| `Int[x]()` | 整数に変換 | `Int["123"]()` → 123 | `Int["abc"]()` → 0 |
| `Int[str, base]()` | 指定基数で整数に変換 | `Int["ff", 16]()` → 255 | `Int["gg", 16]()` → 0 |
| `Float[x]()` | 浮動小数点に変換 | `Float["3.14"]()` → 3.14 | `Float["abc"]()` → 0.0 |
| `Str[x]()` | 文字列に変換 | `Str[42]()` → "42" | - |
| `Bool[x]()` | 真偽値に変換 | `Bool[1]()` → true | `Bool[0]()` → false |
| `Ordinal[e]()` | Enum → Int（宣言 ordinal） | `Ordinal[Color:Red()]()` → 0 | 非 Enum は runtime error |

```taida
// 文字列 → 整数
Int["123"]() ]=> num     // 123
Int["abc"]() ]=> num     // 0（変換失敗: デフォルト値）
Int["+5"]() ]=> num      // 5（符号付き文字列も受理）

// 文字列 → 整数（基数指定）
Int["ff", 16]() ]=> hex  // 255
Int["1010", 2]() ]=> bin // 10

// 文字列 → 浮動小数点
Float["3.14"]() ]=> val  // 3.14
Float["abc"]() ]=> val   // 0.0（変換失敗: デフォルト値）

// 数値 → 文字列
Str[42]() ]=> text       // "42"

// 値 → 真偽値
Bool[1]() ]=> flag       // true
Bool[0]() ]=> flag       // false

// Enum → Int（明示的な ordinal 変換）
Enum => Color = :Red :Green :Blue
n <= Ordinal[Color:Green()]()  // 1
```

### `Ordinal[]` -- Enum から Int への唯一の正規経路

`Ordinal[<enum_value>]()` は Enum 値を宣言順の ordinal Int に変換します (C18-3)。`.toString()` を `Int[]` で parse する workaround は fragile で推奨されません。

```taida
Enum => HiveState = :Creating :Running :Stopped

// 基本形
Ordinal[HiveState:Running()]() ]=> n
stdout(n.toString())  // 1

// 変数経由
s <= HiveState:Stopped()
Ordinal[s]() ]=> m
stdout(m.toString())  // 2

// Int カラム / legacy DB との比較で使う
isLate <= Ordinal[s]() > 0
```

**失敗条件**:
- 非 Enum の値を渡すと runtime error: `Ordinal: argument must be an Enum value`
- 引数なし (`Ordinal[]()`) も runtime error

**用途**:
- 既存の Int カラム / binary wire フォーマットとの互換
- 同一 Enum の ordering 比較 (C18-4) 以外で Int 比較が必要な場合
- variant 宣言順が semantic で、将来の並び替えに備えて explicit に固定したい場合

逆方向（`Int → Enum`）の `FromOrdinal[Color, 1]()` は別 track で検討中です（C18 scope 外）。

---

## Lax[T] -- 必ず値を返すモールド型

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

Lax[T] は「操作が失敗しても必ず値を返す」モールド型です。失敗時は型 T のデフォルト値にフォールバックします。

### 概念

```taida
// クラスライク型として定義 (詳細は 04_class_like.md)
Mold[T] => Lax[T] = @(hasValue: Bool)

// 成功: hasValue = true, unmold で値が取り出せます
// 失敗: hasValue = false, unmold でデフォルト値が返ります
```

### IS-A Lax のモールド

| 操作 | 説明 |
|------|------|
| `Div[x, y]()` | 除算（ゼロ除算で hasValue=false） |
| `Mod[x, y]()` | 剰余（ゼロ除算で hasValue=false） |
| `Int[x]()` / `Float[x]()` / `Str[x]()` / `Bool[x]()` | 型変換（変換失敗で hasValue=false） |
| `JSON[raw, Schema]()` | JSON パース（パース失敗で hasValue=false） |
| `.get(idx)` | インデックスアクセス（範囲外で hasValue=false） |
| `.first()` / `.last()` | 先頭/末尾アクセス（空リストで hasValue=false） |
| `.max()` / `.min()` | 最大/最小値（空リストで hasValue=false） |
| `Find[list, fn]()` | 条件検索（見つからないとき hasValue=false） |

### Lax メソッド

| メソッド | 説明 | 戻り値 |
|---------|------|--------|
| `hasValue` | 値を持つかどうか（フィールドアクセス） | Bool |
| `isEmpty()` | 値を持たないかどうか | Bool |
| `getOrDefault(default)` | 値があれば値を、なければ指定したデフォルト値を返します | T |
| `map(fn)` | 値があれば fn を適用した新しい Lax を返します | Lax[U] |
| `flatMap(fn)` | 値があれば fn を適用し、fn が返す Lax をそのまま返します | Lax[U] |
| `toString()` | 文字列表現を返します | Str |
| `unmold()` | 値を取り出します（失敗時はデフォルト値） | T |

```taida
lax <= Div[10, 3]()
lax.hasValue              // true
lax.isEmpty()             // false
lax.getOrDefault(99)      // 3
lax.map(_ x = x * 2) ]=> doubled  // 6
lax.toString()            // "Lax(3)"

empty <= Div[10, 0]()
empty.hasValue            // false
empty.isEmpty()           // true
empty.getOrDefault(99)    // 99
empty.map(_ x = x * 2) ]=> doubled  // 0（空のまま）
empty.toString()          // "Lax(default: 0)"
```

---

## Result[T, P] -- 述語付き操作モールド

成功/失敗を**述語（P: :T => :Bool）**で判定するモールド型です。`]=>` でアンモールディングすると述語が評価され、成功なら値 T を返し、失敗なら throw が発動します。

```taida
// クラスライク型として定義 (詳細は 04_class_like.md)
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)
// P は :T => :Bool（成功条件を定義する述語）

// 使用例
Error => ValidationError = @(field: Str)

// true = 常に真の述語（常に成功）
ok <= Result[42, _ = true]()
ok ]=> value  // 42（述語 true → 成功）

// false = 常に偽の述語（常に失敗）
err <= Result[0, _ = false](throw <= ValidationError(type <= "ValidationError", message <= "fail", field <= "age"))
err ]=> value  // throw が発動（エラー天井で捕捉可能）

// ラムダ述語によるバリデーション
age <= 15
checked <= Result[age, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age"))
checked ]=> value  // age=15 → 述語が false → throw 発動
```

### => と ]=> の違い

```taida
Result[x, pred](throw <= err) => r   // r: Result[T, P]（default solidify = self）
Result[x, pred](throw <= err) ]=> r  // r: T（unmold 時に述語を評価 → 真なら値、偽なら throw）
```

### 関数の戻り値として

```taida
validateAge age: Int =
  Result[age, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age"))
=> :Result[Int, _]  // _ = 述語の型を推論
```

### モナディック操作はメソッドで

Result, Lax の `map`, `flatMap`, `mapError` はメソッドとして残っています。これらはモナディック操作であり、型ごとに固有の振る舞いを持つためです。

---

## Gorillax[T] -- 覚悟のモールド型

Lax[T] が「失敗してもデフォルト値で続行」なのに対し、Gorillax[T] は「失敗したらゴリラがプログラムを止める」モールド型です。安全を保証しない外部操作（npm パッケージの利用など）に使います。

```taida
// クラスライク型として定義 (詳細は 04_class_like.md)
Mold[T] => Gorillax[T] = @(hasValue: Bool)

// Lax: 失敗 → デフォルト値（プログラム続行）
// Gorillax: 失敗 → ゴリラ（プログラム終了）
```

### 直接生成

```taida
Gorillax[42]() => g         // g: Gorillax[Int], hasValue = true
g ]=> value                  // 42（成功: 値が取り出せます）
```

### Cage[T, F] -- JS Molten 専用の操作檻

Cage は JS / npm 連携の `Molten => JS` 分岐に対して操作を実行し、結果を Gorillax で包むモールドです。Cage 内でのみ JS 溶鉄への操作が許可されます。

**Cage は `Molten => JS` 専用です。** 第一引数は JS 分岐の Molten 値だけを受け付けます。Taida の型安全な値、JSON 分岐、`TemperedMolten[T]` は Cage に渡せません。

```taida
Cage[molten, function]() => gorillax  // gorillax: Gorillax[U]
// molten: Molten => JS
// function: :Molten => :U（JS 溶鉄に対する操作）
```

### 使用例

```taida
>>> npm:lodash => @(lodash)  // lodash: Molten（溶鉄状態、直接使用不可）

items: @[Int] <= @[1, 2, 3, 4, 5]

// Cage 内でのみ Molten への操作が許可されます
Cage[lodash, _ lo = lo.sum(items)] => rilax  // rilax: Gorillax[Int]

// 成功時: 値が取り出せます
rilax ]=> total              // total = 15

// 失敗時: ゴリラがプログラムを停止させます
// rilax ]=> total           // ><（ゴリラ出現）

// Taida の値は Cage に渡せません
// Cage[42, _ x = x + 1]()  // コンパイルエラー: Cage requires Molten type
```

### JSON と Molten の関係

JSON は `Molten => JSON` の分岐です。JS / npm 連携の値は `Molten => JS` の分岐です。どちらも外部由来の不透明な値ですが、鋳造経路は分かれます。

JSON には専用のモールド `JSON[raw, Schema]()` があります。JSON を Cage に渡すことはできません。

| 状況 | 使うべきもの |
|------|------------|
| JSON 文字列を型付き値に変換 | `JSON[raw, Schema]()` |
| npm パッケージの Molten を操作 | `Cage[molten, fn]()` |
| JSON を Cage に渡す | 不可（`JSON[raw, Schema]()` を使うこと） |

### .relax() -- RelaxedGorillax[T]

`.relax()` を呼ぶと、ゴリラが `RelaxedGorillaEscaped` エラーに変わります。このエラーは `|==` エラー天井で捕捉可能です。

```taida
rilax.relax() => relaxed     // relaxed: RelaxedGorillax[Int]

|== error: RelaxedGorillaEscaped =
  | _ |> 0
=> :Int

relaxed ]=> value            // 失敗時: throw（|== で捕捉）
```

### Gorillax メソッド

| メソッド | 説明 | 戻り値 |
|---------|------|--------|
| `hasValue` | 値を持つかどうか（フィールドアクセス） | Bool |
| `isEmpty()` | 値を持たないかどうか | Bool |
| `relax()` | RelaxedGorillax に変換（throwable にする） | RelaxedGorillax[T] |
| `toString()` | 文字列表現を返します | Str |

### RelaxedGorillax メソッド

| メソッド | 説明 | 戻り値 |
|---------|------|--------|
| `hasValue` | 値を持つかどうか（フィールドアクセス） | Bool |
| `isEmpty()` | 値を持たないかどうか | Bool |
| `toString()` | 文字列表現を返します | Str |

### 使い分け: Lax vs Gorillax

| 状況 | 使うべきもの |
|------|------------|
| ゼロ除算、範囲外アクセス、型変換 | Lax（デフォルト値で安全に続行） |
| 外部パッケージの Molten 操作（npm 等） | Cage → Gorillax（失敗は致命的） |
| 外部操作の失敗をキャッチしたい | `.relax()` → RelaxedGorillax |

---

## JS 補助モールド（JS バックエンド専用）

npm パッケージから得られる Molten 値に対して、JavaScript 固有の操作を行うモールドです。これらは全て **Molten を受け取り Molten を返します**（溶鉄から溶鉄）。最終的に Taida の型世界に持ち込むには Cage を経由します。

**重要: これらのモールドは JS トランスパイラでのみ動作します。** インタプリタおよび Native バックエンドでは「JS バックエンド専用です」コンパイルエラーになります。ポータブルなコード（複数バックエンドで動作させるコード）では使わないでください。

**3バックエンド・パリティの対象外です。** JSNew, JSSet, JSBind, JSSpread は JS 連携層に属し、JS トランスパイラ固有の機能です。インタプリタや Native バックエンドに同等の実装は提供されません。

### JSNew[constructor, args] -- コンストラクタ呼び出し

JavaScript の `new` キーワードに相当する操作です。Molten のコンストラクタを呼び出し、新しい Molten インスタンスを返します。

```taida
>>> npm:express => @(express)

// new express.Router() に相当
JSNew[express.Router, @()]() => router  // router: Molten
```

| `[]` 必須 | 説明 |
|----------|------|
| `constructor` | Molten のコンストラクタ関数 |
| `args` | コンストラクタ引数（ぶちパック） |

**戻り値**: `Molten`

### JSSet[obj, key, value] -- プロパティ設定

Molten オブジェクトのプロパティに値を設定します。JavaScript の `obj[key] = value` に相当する破壊的操作です。**同一の Molten 参照を返します**（JavaScript の破壊的代入のセマンティクスがそのまま適用されます）。

```taida
>>> npm:express => @(express)

// Cage 内で Molten の関数を呼び出し、JS 補助モールドで操作します
Cage[express, _ e = e()]() ]=> app    // app: Molten（express() の結果）
JSSet[app, "port", 3000]() => app2    // app2: Molten（app と同一参照）
```

| `[]` 必須 | 説明 |
|----------|------|
| `obj` | 対象の Molten オブジェクト |
| `key` | プロパティ名（Str） |
| `value` | 設定する値（Any） |

**戻り値**: `Molten`（同一参照。JavaScript の破壊的代入に相当）

### JSBind[obj, method] -- this バインド

Molten オブジェクトのメソッドに `this` をバインドします。JavaScript の `obj.method.bind(obj)` に相当します。

```taida
>>> npm:someLib => @(lib)

handler <= JSBind[lib, "handleRequest"]() // handler: Molten
```

| `[]` 必須 | 説明 |
|----------|------|
| `obj` | 対象の Molten オブジェクト |
| `method` | メソッド名（Str） |

**戻り値**: `Molten`

### JSSpread[target, source] -- スプレッド展開

Molten オブジェクトにスプレッド展開でプロパティをマージします。JavaScript の `{...target, ...source}` に相当します。

```taida
>>> npm:config => @(defaults)

overrides <= @(port <= 8080, debug <= true)
merged <= JSSpread[defaults, overrides]()  // merged: Molten
```

| `[]` 必須 | 説明 |
|----------|------|
| `target` | マージ先の Molten オブジェクト |
| `source` | マージ元の値（Any） |

**戻り値**: `Molten`

### 型の流れ: npm から値の取り出しまで

npm パッケージを使う際の典型的な型の流れは以下の通りです:

```
npm import (Molten) → JSNew 等 (Molten→Molten) → Cage (Molten→Gorillax) → ]=> (値)
```

```taida
>>> npm:express => @(express)           // express: Molten

// Cage 内で Molten の関数を呼び出します
Cage[express, _ e = e()]() ]=> app       // app: Molten（express() の結果）
JSNew[express.Router, @()]() => router   // router: Molten（new 呼び出し）

// Cage で Molten の操作を実行し、Gorillax で受け取ります
Cage[app, _ a = a.get("/")]() => result  // result: Gorillax[Molten]
result ]=> handler                       // handler: Molten（またはゴリラ）
```

Molten への直接的な関数呼び出しは Cage 内でのみ許可されます。JS 補助モールド（JSNew, JSSet, JSBind, JSSpread）は Taida のモールド構文なので Cage 外でも使用可能ですが、Molten のメソッド呼び出しや関数呼び出しは必ず Cage を経由してください。

---

## アンモールディング

モールディング型から値を取り出すには3つの方法があります。
`Name[args]()` に対する `]=>` / `<=[` / `.unmold()` は、`solidify` 済みの値に対して実行されます。

### `]=>` 演算子

```taida
lax <= Div[10, 3]()
lax ]=> value  // value = 3
```

### `<=[` 演算子（逆向き）

```taida
value <=[ lax  // value = 3
```

### `.unmold()` メソッド

```taida
value <= lax.unmold()  // value = 3
```

### 使い分け

```taida
// 代入したい場合は演算子を使います
lax ]=> value

// 式の中で使いたい場合はメソッドを使います
result <= lax.unmold() + 10
```

---

## パイプラインでのモールド使用

パイプライン `=>` の中では、`_` が前ステップの値を受け取ります。モールドの `[]` 内でも使用可能です。

### 正方向パイプライン

```taida
"  Hello, World!  " => Trim[_]() => Upper[_]() => Replace[_, ",", ""]() => Split[_, " "]() => result
```

> **注意**: `<=` のチェーンによる逆方向パイプラインは現在サポートされていません。順方向パイプライン `=>` または中間変数を使用してください。

### リストのパイプライン

```taida
numbers => Filter[_, _ x = Mod[x, 2]() ]=> r; r == 0]() => Map[_, _ x = x * 2]() => result
```

### 直接呼び出し

```taida
Filter[list, isEven]() ]=> result
```

### 数値のパイプライン

```taida
value => Abs[_]() => Clamp[_, 0, 100]() => Round[_]() => result
```

---

## ユーザー定義モールド

`Mold[T]` を継承して独自のモールディング型を定義できます。これは [クラスライク型定義](04_class_like.md) の単一構文の一例です。

```taida
Mold[:@(x: Int, y: Int)] => Container = @(
  count: Int
  name: Str
  unmold _ =
    filling
  => @(x: Int, y: Int) // unmoldのカスタム定義（_ = :T）
)

data <= @(x <= 1, y <= 2)
box <= Container[data, 1, "my-container"]()
box ]=> extracted  // @(x <= 1, y <= 2)
box.count          // 1
box.name           // "my-container"
```

### solidify オーバーライド（自型以外に固める）

`solidify` をオーバーライドすると、カスタムモールドでも自型以外を返せます。

```taida
Mold[T] => TryInt[T] = @(
  solidify _ =
    Int[filling]()
  => :Lax[Int]
)

TryInt["123"]() => boxed   // boxed: Lax[Int]
TryInt["123"]() ]=> value  // value: Int
```

### メソッドの定義

モールディング型内にメソッドを定義できます:

```taida
Mold[T] => Container[T] = @(
  label: Str <= ""

  describe =
    `Container(${label}): ${unmold().toString()}`
  => :Str

  mapValue fn: T => :U =
    Container[fn(unmold())](label <= label)
  => :Container[U]
)
```

> declare-only 関数フィールド (`fn: T => :U` 本体なし) は E30 で全クラスライク系統 (Mold / Error 含む) に許可されました。defaultFn による自動充足の挙動については [クラスライク型定義](04_class_like.md#declare-only-関数フィールド) を参照してください。

---

## まとめ

| 概念 | 構文 |
|------|------|
| 鋳型の定義 | `Mold[T] => TypeName[T] = @(...)` ([04_class_like.md](04_class_like.md)) |
| 中身 | `filling: T` |
| Is-A 決定 | `solidify _ => :V`（省略時: self） |
| 取り出し定義 | `unmold _ => :U`（省略時: filling） |
| インスタンス化 | `Name[a, b, ...](opt <= ...)`（`a` は filling、`b...` はデフォルト値なしフィールド宣言順） |
| アンモールディング | `]=>`, `<=[`, `.unmold()` |
| `[]` 位置引数 | 必須引数（何を / 何で） |
| `()` 名前付き設定 | オプション設定（どうやって） |
| 文字列モールド | Upper, Lower, Trim, Split, Replace, Slice, CharAt, Repeat, Reverse, Pad |
| 数値モールド | ToFixed, Abs, Floor, Ceil, Round, Truncate, Clamp |
| リストモールド | Reverse, Concat, Append, Prepend, Join, Sum, Sort, Unique, Flatten, Find, FindIndex, Count, Take, Drop, TakeWhile, DropWhile, Zip, Enumerate |
| HOF モールド | Filter, Map, Fold, Foldr |
| 演算モールド | Div[x, y]() → Lax, Mod[x, y]() → Lax |
| 型変換モールド | Int[x]() → Lax, Float[x]() → Lax, Str[x]() → Lax, Bool[x]() → Lax |
| Lax[T] | 必ず値を返すモールド型。Div/Mod/get/first/last/JSON の戻り値 |
| Result[T, P] | 述語付き操作モールド（P: :T => :Bool で成功/失敗を判定） |
| Gorillax[T] | 覚悟のモールド型。unmold 失敗時はゴリラ（プログラム終了） |
| RelaxedGorillax[T] | `.relax()` で生成。unmold 失敗時は RelaxedGorillaEscaped を throw |
| Cage[T, F] | Molten 専用の操作檻。`F(Molten)` を実行し Gorillax[U] を返す |
| JSNew[constructor, args] | JS の `new` 呼び出し。Molten→Molten（JS バックエンド専用） |
| JSSet[obj, key, value] | JS プロパティ設定。Molten→Molten（JS バックエンド専用） |
| JSBind[obj, method] | JS の `this` バインド。Molten→Molten（JS バックエンド専用） |
| JSSpread[target, source] | JS スプレッド展開。Molten→Molten（JS バックエンド専用） |
| パイプライン | `=>` / `<=` の中で `_` が前ステップの値を受け取る |

旧 D 世代までの「Mold 基底クラス」「Mold 継承定義」記述は [クラスライク型定義 / モールド系統](04_class_like.md#モールド系統-操作モールド) に統合されました。
